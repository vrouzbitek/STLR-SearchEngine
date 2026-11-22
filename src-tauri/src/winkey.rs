//! "Win" shortcut: a lone tap of the Windows key opens STLR instead of Start. Win+E, Win+R, Win+D...
//! keep working: only a press and release with no other key in between counts. Start is suppressed the
//! way AutoHotkey does it: the lone Win release is held back and replayed right after a no-op key (vkE8),
//! so the shell sees "a combo happened" and doesn't open Start. Every other key passes untouched.
//! Implemented with a low-level keyboard hook on its own thread (installed on first enable).
//! Same mode, mouse side: a left click on the taskbar's Start button opens STLR right above it.
//! Win11 draws Start in XAML (no HWND), so its rect comes from UI Automation on a worker thread and is
//! cached; the mouse hook only compares points (LL hooks must never block).
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Mutex, Once, OnceLock};
use std::time::Duration;
use tauri::AppHandle;
use windows::core::{w, BSTR};
use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_BSTR};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Descendants, UIA_AutomationIdPropertyId};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static ENABLED: AtomicBool = AtomicBool::new(false);
const EMPTY: RECT = RECT { left: 0, top: 0, right: 0, bottom: 0 };
/// Cached (taskbar, Start button) rects in physical screen px; primary taskbar only.
static RECTS: Mutex<(RECT, RECT)> = Mutex::new((EMPTY, EMPTY));
static REFRESH: OnceLock<SyncSender<()>> = OnceLock::new();
/// We swallowed a Start button press, so its release must be swallowed too.
static CLICK: AtomicBool = AtomicBool::new(false);
static TAP: Mutex<Tap> = Mutex::new(Tap { win_down: false, combo: false });
static APP: OnceLock<AppHandle> = OnceLock::new();
static HOOK: Once = Once::new();
/// Marks our own injected mask key so the hook ignores it (other injected input, e.g. tests, still counts).
const OURS: usize = 0x5354_4C52; // "STLR"
const MASK_VK: u16 = 0xE8; // unassigned virtual key

/// Turn the Win-key shortcut on/off. The hook thread is created once and stays idle while disabled.
pub fn set(app: &AppHandle, on: bool) {
    let _ = APP.set(app.clone());
    ENABLED.store(on, Ordering::SeqCst);
    if on {
        HOOK.call_once(|| {
            let (tx, rx) = sync_channel::<()>(1);
            let _ = REFRESH.set(tx);
            std::thread::spawn(move || locate_loop(rx));
            std::thread::spawn(|| unsafe {
                if SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), None, 0).is_err() {
                    eprintln!("Start button hook unavailable");
                }
                let Ok(_hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0) else {
                    eprintln!("Win key hook unavailable");
                    return;
                };
                // LL hooks are called on the installing thread's message loop.
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
            });
        });
    }
}

/// Lone-tap detector. Returns true on the Win release that should open STLR.
#[derive(Debug, PartialEq)]
struct Tap {
    win_down: bool,
    combo: bool,
}

impl Tap {
    fn step(&mut self, is_win: bool, down: bool) -> bool {
        match (is_win, down) {
            // Auto-repeat sends more Win downs: only the first one starts a new tap.
            (true, true) => {
                if !self.win_down {
                    self.win_down = true;
                    self.combo = false;
                }
                false
            }
            (false, true) => {
                self.combo |= self.win_down;
                false
            }
            (true, false) => std::mem::replace(&mut self.win_down, false) && !self.combo,
            (false, false) => false,
        }
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && ENABLED.load(Ordering::Relaxed) {
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if k.dwExtraInfo != OURS {
            let is_win = k.vkCode == VK_LWIN.0 as u32 || k.vkCode == VK_RWIN.0 as u32;
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let fire = TAP.lock().map(|mut t| t.step(is_win, down)).unwrap_or(false);
            if fire {
                if let Some(app) = APP.get() {
                    let a = app.clone();
                    // Never block inside the hook (Windows drops slow LL hooks): hand off to the event loop.
                    let _ = app.run_on_main_thread(move || crate::toggle(&a));
                }
                // Swallow this Win up and replay it after the mask key, so the order is guaranteed.
                if mask_and_release(k.vkCode as u16) {
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn inside(r: &RECT, p: POINT) -> bool {
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

unsafe extern "system" fn mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && ENABLED.load(Ordering::Relaxed) {
        let p = unsafe { (*(lparam.0 as *const MSLLHOOKSTRUCT)).pt };
        match wparam.0 as u32 {
            // Hovering the taskbar: re-read the Start rect (it moves as centered taskbar icons come and go).
            WM_MOUSEMOVE => {
                if RECTS.lock().map(|r| inside(&r.0, p)).unwrap_or(false) {
                    if let Some(tx) = REFRESH.get() {
                        let _ = tx.try_send(()); // full = a refresh is already queued
                    }
                }
            }
            WM_LBUTTONDOWN => {
                let start = RECTS.lock().map(|r| r.1).unwrap_or(EMPTY);                if inside(&start, p) {
                    if let Some(app) = APP.get() {
                        let a = app.clone();
                        let _ = app.run_on_main_thread(move || crate::toggle_at(&a, Some((start.left, start.top))));
                        CLICK.store(true, Ordering::SeqCst);
                        return LRESULT(1);
                    }
                }
            }
            WM_LBUTTONUP if CLICK.swap(false, Ordering::SeqCst) => return LRESULT(1),
            _ => {}
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Worker: refreshes RECTS on request (throttled) and every few seconds while enabled.
fn locate_loop(rx: std::sync::mpsc::Receiver<()>) {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let Ok(uia) = (unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }) else {
        eprintln!("UI Automation unavailable: Start button click not intercepted");
        return;
    };
    let mut button: Option<IUIAutomationElement> = None;
    loop {
        if ENABLED.load(Ordering::Relaxed) {
            let found = locate(&uia, &mut button);            if let Ok(mut r) = RECTS.lock() {
                *r = found.unwrap_or((EMPTY, EMPTY)); // not found = never intercept (Start works as usual)
            }
        }
        std::thread::sleep(Duration::from_millis(250)); // throttle for mouse-move bursts
        if let Err(RecvTimeoutError::Disconnected) = rx.recv_timeout(Duration::from_secs(5)) {
            return;
        }
    }
}

fn locate(uia: &IUIAutomation, button: &mut Option<IUIAutomationElement>) -> Option<(RECT, RECT)> {
    unsafe {
        let tray = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let mut tr = EMPTY;
        GetWindowRect(tray, &mut tr).ok()?;
        // Cached element: one cheap cross-process call. Re-found after Explorer restarts (call fails / empty rect).
        let cached = button.as_ref().and_then(|b| b.CurrentBoundingRectangle().ok()).filter(|r| r.right > r.left);
        let start = match cached {
            Some(r) => r,
            None => {
                let mut id = VARIANT::default();
                (*id.Anonymous.Anonymous).vt = VT_BSTR;
                (*id.Anonymous.Anonymous).Anonymous.bstrVal = std::mem::ManuallyDrop::new(BSTR::from("StartButton"));
                let cond = uia.CreatePropertyCondition(UIA_AutomationIdPropertyId, &id);
                let _ = VariantClear(&mut id); // frees the BSTR
                let cond = cond.ok()?;
                let b = uia.ElementFromHandle(tray).ok()?.FindFirst(TreeScope_Descendants, &cond).ok()?;
                let r = b.CurrentBoundingRectangle().ok()?;
                *button = Some(b);
                r
            }
        };
        Some((tr, start))
    }
}

/// Injects mask down, mask up, Win up. Returns false if nothing could be injected: the caller then lets
/// the real Win up through, so Win can never get stuck down (worst case Start opens as usual).
fn mask_and_release(win_vk: u16) -> bool {
    let key = |vk, flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: OURS } },
    };
    let inputs = [key(MASK_VK, KEYBD_EVENT_FLAGS(0)), key(MASK_VK, KEYEVENTF_KEYUP), key(win_vk, KEYEVENTF_KEYUP)];
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) == inputs.len() as u32 }
}

#[cfg(test)]
mod tests {
    use super::Tap;

    fn run(keys: &[(bool, bool)]) -> Vec<bool> {
        let mut t = Tap { win_down: false, combo: false };
        keys.iter().map(|&(w, d)| t.step(w, d)).collect()
    }

    #[test]
    fn lone_tap_fires_once() {
        assert_eq!(run(&[(true, true), (true, false)]), [false, true]);
        // auto-repeat while held is still a lone tap
        assert_eq!(run(&[(true, true), (true, true), (true, true), (true, false)]), [false, false, false, true]);
    }

    #[test]
    fn combos_and_other_keys_never_fire() {
        // Win+E
        assert_eq!(run(&[(true, true), (false, true), (false, false), (true, false)]), [false; 4]);
        // typing without Win, and a stray Win up (e.g. pressed before the hook existed)
        assert_eq!(run(&[(false, true), (false, false), (true, false)]), [false; 3]);
        // a combo doesn't poison the next lone tap
        assert_eq!(run(&[(true, true), (false, true), (true, false), (true, true), (true, false)]), [false, false, false, false, true]);
    }
}
