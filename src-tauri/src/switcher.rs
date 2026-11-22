//! Foreground-app context + "switch to the already-open window instead of launching a 2nd instance".
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex, RwLock};
use windows::core::{Interface, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::*;

/// lowercased .lnk path -> lowercased target exe (None = not resolvable to an exe)
static TARGETS: LazyLock<Mutex<HashMap<String, Option<String>>>> = LazyLock::new(Default::default);
/// lowercased exe path -> top-most visible window (HWND as isize), refreshed on every launcher open.
static RUNNING: LazyLock<RwLock<HashMap<String, isize>>> = LazyLock::new(Default::default);

fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// File name of the foreground app (e.g. "chrome.exe"). Call BEFORE showing our window.
pub fn foreground_exe() -> Option<String> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return None;
    }
    let pid = window_pid(hwnd);
    if pid == std::process::id() {
        return None;
    }
    let path = process_path(pid)?;
    Path::new(&path).file_name().map(|n| n.to_string_lossy().to_lowercase())
}

unsafe extern "system" fn collect(hwnd: HWND, lp: LPARAM) -> BOOL {
    unsafe {
        let out = &mut *(lp.0 as *mut Vec<HWND>);
        let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid());
        let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
        if IsWindowVisible(hwnd).as_bool() && !owned && !tool && GetWindowTextLengthW(hwnd) > 0 {
            out.push(hwnd);
        }
    }
    true.into()
}

/// Re-snapshot app windows (~1-3ms). EnumWindows is top-to-bottom z-order, so the first window per exe wins.
pub fn refresh() {
    let mut hwnds: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut hwnds as *mut _ as isize));
    }
    let me = std::process::id();
    let mut pids: HashMap<u32, Option<String>> = HashMap::new();
    let mut map = HashMap::new();
    for h in hwnds {
        let pid = window_pid(h);
        if pid == me {
            continue;
        }
        if let Some(exe) = pids.entry(pid).or_insert_with(|| process_path(pid).map(|p| p.to_lowercase())) {
            if is_browser(exe) && is_web_app(h) {
                continue; // an installed web app (PWA) runs as msedge.exe/chrome.exe but is not the browser
            }
            map.entry(exe.clone()).or_insert(h.0 as isize);
        }
    }
    *RUNNING.write().unwrap() = map;
}

fn is_browser(exe: &str) -> bool {
    ["msedge.exe", "chrome.exe", "brave.exe", "vivaldi.exe", "opera.exe"].iter().any(|b| exe.ends_with(b))
}

/// PWA windows carry their own AppUserModelID ("127.0.0.1-79B5721F_x!App", "Chrome._crx_abc..."); browser windows don't.
fn is_web_app(hwnd: HWND) -> bool {
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
    const PKEY_APP_USER_MODEL_ID: PROPERTYKEY =
        PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };
    unsafe {
        let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) else { return false };
        let Ok(v) = store.GetValue(&PKEY_APP_USER_MODEL_ID) else { return false };
        let id = v.to_string();
        id.contains('!') || id.contains("_crx_")
    }
}

fn ext(path: &str) -> String {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub fn is_app_path(path: &str) -> bool {
    matches!(ext(path).as_str(), "exe" | "lnk")
}

/// Target exe of an .exe/.lnk (lowercased). Resolves and caches .lnk via IShellLink; call off the hot path.
pub fn resolve(path: &str) -> Option<String> {
    match ext(path).as_str() {
        "exe" => Some(path.to_lowercase()),
        "lnk" => {
            let key = path.to_lowercase();
            if let Some(t) = TARGETS.lock().unwrap().get(&key) {
                return t.clone();
            }
            // Store/MSIX app shortcuts resolve to explorer.exe: matching that would "switch" to any Explorer window.
            let t = if lnk_app_id(path).is_some() {
                None
            } else {
                lnk_target(path).map(|s| squirrel_app(&s.to_lowercase())).filter(|s| s.ends_with(".exe"))
            };
            TARGETS.lock().unwrap().insert(key, t.clone());
            t
        }
        _ => None,
    }
}

/// Squirrel installs (Discord, Slack, Teams classic): the shortcut runs a stub `<dir>\X.exe` while the
/// window belongs to `<dir>\app-<version>\X.exe`. Map the stub to the newest version so Switch finds it.
fn squirrel_app(exe: &str) -> String {
    let p = Path::new(exe);
    let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { return exe.into() };
    let version = |d: &Path| -> Option<Vec<u32>> {
        let v = d.file_name()?.to_str()?.strip_prefix("app-")?;
        v.split('.').map(|n| n.parse().ok()).collect()
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|d| d.join(name).is_file())
        .filter_map(|d| Some((version(&d)?, d)))
        .max()
        .map_or(exe.into(), |(_, d)| d.join(name).to_string_lossy().to_lowercase())
}

/// Hot-path variant: cache only, never touches COM or disk.
fn cached_target(path: &str) -> Option<String> {
    match ext(path).as_str() {
        "exe" => Some(path.to_lowercase()),
        "lnk" => TARGETS.lock().unwrap().get(&path.to_lowercase()).cloned().flatten(),
        _ => None,
    }
}

/// Exes (lowercased full paths) with a visible window in the last `refresh()`.
pub fn running_exes() -> Vec<String> {
    RUNNING.read().unwrap().keys().cloned().collect()
}

/// Raw .lnk target (files, folders, exes), original case. Not cached; call off the hot path.
pub fn lnk_target(path: &str) -> Option<String> {
    unsafe { lnk_target_raw(path) }
}

pub fn is_running(path: &str) -> bool {
    cached_target(path).is_some_and(|t| RUNNING.read().unwrap().contains_key(&t))
}

/// Restore + foreground the app's existing window. Returns false if none (caller launches normally).
/// shortcut: matches by exe path only, so Store/UWP apps (ApplicationFrameHost) and launcher-stub shortcuts
/// (e.g. Update.exe --processStart) always launch; those apps usually focus their own window anyway.
pub fn focus_existing(path: &str) -> bool {
    let Some(target) = resolve(path) else { return false };
    refresh();
    let Some(h) = RUNNING.read().unwrap().get(&target).copied() else { return false };
    unsafe {
        let hwnd = HWND(h as _);
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
    true
}

/// AppUserModelID ("Claude_pzs8sxrjxfjjc!Claude") of a shortcut to a packaged (MSIX/Store) app. Two forms:
/// a PIDL into shell:AppsFolder (Store-created), or `explorer.exe shell:AppsFolder\<AUMID>` (hand-made).
pub fn lnk_app_id(path: &str) -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetNameFromIDList, SIGDN_DESKTOPABSOLUTEPARSING};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        link.cast::<IPersistFile>().ok()?.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut args = [0u16; 1024];
        if link.GetArguments(&mut args).is_ok() {
            let end = args.iter().position(|&c| c == 0).unwrap_or(args.len());
            let a = String::from_utf16_lossy(&args[..end]);
            if let Some(id) = a.trim().trim_matches('"').strip_prefix("shell:AppsFolder\\").or_else(|| a.trim().strip_prefix("shell:appsFolder\\")) {
                return id.contains('!').then(|| id.to_string());
            }
        }
        let pidl = link.GetIDList().ok()?;
        let name = SHGetNameFromIDList(pidl, SIGDN_DESKTOPABSOLUTEPARSING);
        CoTaskMemFree(Some(pidl as *const _));
        let name = name.ok()?;
        let s = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        let id = s?.rsplit('\\').next()?.to_string();
        id.contains('!').then_some(id)
    }
}

unsafe fn lnk_target_raw(path: &str) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        link.cast::<IPersistFile>().ok()?.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut buf = [0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (end > 0).then(|| String::from_utf16_lossy(&buf[..end]))
    }
}

#[cfg(test)]
mod tests {
    /// Manual: run with a PWA (e.g. Edge "Johnny – dashboard") open and the browser itself closed.
    #[test]
    #[ignore]
    fn pwa_window_is_not_the_browser() {
        super::refresh();
        assert!(!super::running_exes().iter().any(|e| e.ends_with("msedge.exe")), "{:?}", super::running_exes());
    }

    #[test]
    fn squirrel_stub_maps_to_newest_app_dir() {
        let root = std::env::temp_dir().join("stlr-squirrel-test");
        let _ = std::fs::remove_dir_all(&root);
        for d in ["", "app-1.0.999", "app-1.0.10000", "packages"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
            std::fs::write(root.join(d).join("discord.exe"), "").unwrap();
        }
        let stub = root.join("discord.exe").to_string_lossy().to_lowercase();
        assert_eq!(super::squirrel_app(&stub), root.join("app-1.0.10000").join("discord.exe").to_string_lossy().to_lowercase());
        let plain = root.join("packages").join("discord.exe").to_string_lossy().to_lowercase();
        assert_eq!(super::squirrel_app(&plain), plain, "no app-* siblings: unchanged");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Manual: a Store app shortcut (Claude) with an Explorer window open must not count as running.
    #[test]
    #[ignore]
    fn store_app_shortcut_is_not_explorer() {
        let p = r"E:\ProgramData\Microsoft\Windows\Start Menu\Programs\🎮 Gamedev\Claude.lnk";
        assert_eq!(super::resolve(p), None);
        super::refresh();
        assert!(!super::is_running(p));
    }
}
