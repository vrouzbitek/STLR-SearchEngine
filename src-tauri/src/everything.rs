//! Everything (voidtools) IPC over WM_COPYDATA — no SDK DLL needed.
//! Query: EVERYTHING_IPC_QUERYW sent to the Everything taskbar window.
//! Reply: EVERYTHING_IPC_LISTW sent back (WM_COPYDATA) to a message-only window we own.
use crate::{score, Hit};
use std::cell::RefCell;
use std::sync::Once;
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const COPYDATA_QUERYW: usize = 2;
const REPLY_ID: u32 = 0x5354_4C52; // arbitrary tag so we only accept our own reply
const IPC_FOLDER: u32 = 1;
const IPC_DRIVE: u32 = 2;
const FETCH: u32 = 64; // fetch more than we show, then re-rank locally
const TIMEOUT: Duration = Duration::from_millis(400);
const CLASS: PCWSTR = w!("STLR_EVERYTHING_REPLY");

thread_local! {
    static REPLY: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        let cds = unsafe { &*(lp.0 as *const COPYDATASTRUCT) };
        if cds.dwData == REPLY_ID as usize && !cds.lpData.is_null() {
            let bytes = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }.to_vec();
            REPLY.with(|r| *r.borrow_mut() = Some(bytes));
            return LRESULT(1);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn everything_window() -> Option<HWND> {
    unsafe {
        FindWindowW(w!("EVERYTHING_TASKBAR_NOTIFICATION"), PCWSTR::null())
            .or_else(|_| FindWindowW(w!("EVERYTHING_TASKBAR_NOTIFICATION_(1.5a)"), PCWSTR::null()))
            .ok()
            .filter(|h| !h.is_invalid())
    }
}

/// Returns [] when Everything isn't running or doesn't answer in time — never errors.
pub fn search(query: &str, max: usize) -> Vec<Hit> {
    let query = query.trim();
    if query.is_empty() {
        return vec![];
    }
    let Some(target) = everything_window() else { return vec![] };
    let Some(bytes) = (unsafe { round_trip(target, query) }) else { return vec![] };
    rank(parse(&bytes), query, max)
}

unsafe fn round_trip(target: HWND, query: &str) -> Option<Vec<u8>> {
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        static REG: Once = Once::new();
        REG.call_once(|| {
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: CLASS, ..Default::default() };
            RegisterClassW(&wc);
        });
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0), CLASS, PCWSTR::null(), WINDOW_STYLE(0),
            0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst.into()), None,
        )
        .ok()?;

        // EVERYTHING_IPC_QUERYW: 5 DWORDs + null-terminated UTF-16 search string.
        let mut buf = Vec::<u8>::new();
        for v in [hwnd.0 as usize as u32, REPLY_ID, 0, 0, FETCH] {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        for u in query.encode_utf16().chain([0]) {
            buf.extend_from_slice(&u.to_le_bytes());
        }
        let cds = COPYDATASTRUCT { dwData: COPYDATA_QUERYW, cbData: buf.len() as u32, lpData: buf.as_ptr() as *mut _ };

        REPLY.with(|r| *r.borrow_mut() = None);
        let accepted = SendMessageW(target, WM_COPYDATA, Some(WPARAM(hwnd.0 as usize)), Some(LPARAM(&cds as *const _ as isize)));

        let start = Instant::now();
        let mut msg = MSG::default();
        while accepted.0 != 0 && REPLY.with(|r| r.borrow().is_none()) && start.elapsed() < TIMEOUT {
            // Peek delivers cross-process sent messages (the WM_COPYDATA reply) to wndproc.
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if REPLY.with(|r| r.borrow().is_some()) {
                break;
            }
            MsgWaitForMultipleObjects(None, false, 10, QS_ALLINPUT);
        }
        let _ = DestroyWindow(hwnd);
        REPLY.with(|r| r.borrow_mut().take())
    }
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn wstr_at(b: &[u8], off: usize) -> Option<String> {
    let mut units = Vec::new();
    let mut i = off;
    loop {
        let u = u16::from_le_bytes(b.get(i..i + 2)?.try_into().unwrap());
        if u == 0 {
            break;
        }
        units.push(u);
        i += 2;
    }
    Some(String::from_utf16_lossy(&units))
}

/// EVERYTHING_IPC_LISTW: 7 DWORD header (numitems at index 5), then ITEMW {flags, filename_off, path_off}.
/// Offsets are relative to the start of the list. Returns (name, full_path, is_folder).
fn parse(b: &[u8]) -> Vec<(String, String, bool)> {
    let n = u32_at(b, 20).unwrap_or(0) as usize;
    (0..n)
        .filter_map(|i| {
            let base = 28 + i * 12;
            let flags = u32_at(b, base)?;
            let name = wstr_at(b, u32_at(b, base + 4)? as usize)?;
            let dir = wstr_at(b, u32_at(b, base + 8)? as usize)?;
            let full = if dir.is_empty() { name.clone() } else { format!("{}\\{}", dir.trim_end_matches('\\'), name) };
            Some((name, full, flags & (IPC_FOLDER | IPC_DRIVE) != 0))
        })
        .collect()
}

/// Exact/prefix name matches first, folders before files on ties, then shorter paths.
fn rank(items: Vec<(String, String, bool)>, query: &str, max: usize) -> Vec<Hit> {
    let q = query.to_lowercase();
    let mut v: Vec<_> = items
        .into_iter()
        .map(|(name, path, dir)| (score(&name.to_lowercase(), &q).unwrap_or(5), dir, name, path))
        .collect();
    v.sort_by_key(|(s, dir, _, path)| (*s, !*dir, path.len()));
    v.into_iter()
        .take(max)
        .map(|(s, dir, name, path)| Hit { name, path, kind: if dir { "folder" } else { "file" }, score: s as f32, running: false })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[(u32, &str, &str)]) -> Vec<u8> {
        let mut head = vec![0u32; 7];
        head[5] = items.len() as u32;
        let mut strings = Vec::<u8>::new();
        let str_base = 28 + items.len() * 12;
        let mut recs = Vec::new();
        for (flags, name, path) in items {
            let n_off = str_base + strings.len();
            strings.extend(name.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()));
            let p_off = str_base + strings.len();
            strings.extend(path.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()));
            recs.extend([*flags, n_off as u32, p_off as u32]);
        }
        head.iter().chain(&recs).flat_map(|v| v.to_le_bytes()).chain(strings).collect()
    }

    #[test]
    fn parses_and_ranks() {
        let b = list(&[(0, "reaper.txt", r"C:\x"), (IPC_FOLDER, "Reaper", r"D:\Music"), (0, "my reaper", r"C:\")]);
        let p = parse(&b);
        assert_eq!(p[1], ("Reaper".into(), r"D:\Music\Reaper".into(), true));
        assert_eq!(p[2].1, r"C:\my reaper");
        let r = rank(p, "reaper", 6);
        assert_eq!(r.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["Reaper", "reaper.txt", "my reaper"]);
        assert_eq!(r[0].kind, "folder");
    }

    #[test]
    fn truncated_reply_is_safe() {
        let b = list(&[(0, "a", "b")]);
        assert!(parse(&b[..30]).is_empty());
    }
}
