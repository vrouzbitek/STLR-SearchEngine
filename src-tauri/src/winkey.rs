//! "Win" shortcut: a lone tap of the Windows key opens STLR instead of Start. Win+E, Win+R, Win+D...
//! keep working: only a press and release with no other key in between counts. The shell never sees a
//! lone tap at all: Win down is held back; if another key follows, Win down is replayed right before it
//! (so the combo reaches the shell intact), otherwise the release is swallowed too. (The older AutoHotkey
//! trick, a no-op key before Win up, no longer stops Start on Windows 11 build 26200.)
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
static TAP: Mutex<Tap> = Mutex::new(Tap { held: None, replayed: false });
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

/// What the hook does with one key event.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Act {
    Pass,
    Swallow,
    /// First key of a combo: inject the held-back Win down, then this key.
    Replay(u16),
    /// Lone tap released: swallow it and open STLR.
    Fire,
}

/// Lone-tap state: `held` = Win vk whose down we swallowed; `replayed` = the shell has it (combo).
#[derive(Debug, PartialEq)]
struct Tap {
    held: Option<u16>,
    replayed: bool,
}

impl Tap {
    fn step(&mut self, vk: u16, down: bool) -> Act {
        let is_win = vk == VK_LWIN.0 || vk == VK_RWIN.0;
        match (is_win, down, self.held) {
            // Auto-repeat while held: invisible until a combo replays Win, then the shell gets it as usual.
            (true, true, Some(_)) => if self.replayed { Act::Pass } else { Act::Swallow },
            (true, true, None) => {
                self.held = Some(vk);
                self.replayed = false;
                Act::Swallow
            }
            (false, true, Some(w)) if !self.replayed => {
                self.replayed = true;
                Act::Replay(w)
            }
            // Win up: a lone tap fires; after a combo the shell must see the release; a stray up passes.
            (true, false, Some(_)) => {
                self.held = None;
                if self.replayed { Act::Pass } else { Act::Fire }
            }
            _ => Act::Pass,
        }
    }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && ENABLED.load(Ordering::Relaxed) {
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if k.dwExtraInfo != OURS {
            let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let act = TAP.lock().map(|mut t| t.step(k.vkCode as u16, down)).unwrap_or(Act::Pass);
            match act {
                Act::Pass => {}
                Act::Swallow => return LRESULT(1),
                Act::Replay(win) => {
                    // Re-inject this key behind Win down so the shell sees them in order. If injection fails
                    // (e.g. an elevated window has focus), the key passes alone: combo lost, nothing stuck.
                    let ext = if k.flags.0 & LLKHF_EXTENDED.0 != 0 { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
                    if send(&[key(win, 0, KEYBD_EVENT_FLAGS(0)), key(k.vkCode as u16, k.scanCode as u16, ext)]) {
                        return LRESULT(1);
                    }
                }
                Act::Fire => {
                    if let Some(app) = APP.get() {
                        let a = app.clone();
                        // Never block inside the hook (Windows drops slow LL hooks): hand off to the event loop.
                        let _ = app.run_on_main_thread(move || crate::toggle(&a));
                    }
                    return LRESULT(1);
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn key(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: OURS } },
    }
}

fn send(inputs: &[INPUT]) -> bool {
    unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) == inputs.len() as u32 }
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
                let start = RECTS.lock().map(|r| r.1).unwrap_or(EMPTY);
                if inside(&start, p) {
                    if let Some(app) = APP.get() {
                        let a = app.clone();
                        let _ = app.run_on_main_thread(move || crate::toggle_at(&a, Some((start.left, start.top))));
                        CLICK.store(true, Ordering::SeqCst);
                        // The swallowed click reaches no one, so Windows would refuse us the foreground (bar
                        // shown unfocused, typing goes elsewhere). Being the source of the last input fixes it.
                        mask();
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
            let found = locate(&uia, &mut button);
            if let Ok(mut r) = RECTS.lock() {
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

/// Injects a no-op key press (ignored by our keyboard hook via OURS).
fn mask() {
    send(&[key(MASK_VK, 0, KEYBD_EVENT_FLAGS(0)), key(MASK_VK, 0, KEYEVENTF_KEYUP)]);
}

#[cfg(test)]
mod tests {
    use super::{Act, Tap};
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LWIN, VK_RWIN};

    const WIN: u16 = VK_LWIN.0;
    const E: u16 = 0x45;

    fn run(keys: &[(u16, bool)]) -> Vec<Act> {
        let mut t = Tap { held: None, replayed: false };
        keys.iter().map(|&(vk, d)| t.step(vk, d)).collect()
    }

    #[test]
    fn lone_tap_is_invisible_to_the_shell_and_fires_once() {
        assert_eq!(run(&[(WIN, true), (WIN, false)]), [Act::Swallow, Act::Fire]);
        // auto-repeat while held is still a lone tap
        assert_eq!(run(&[(WIN, true), (WIN, true), (WIN, true), (WIN, false)]), [Act::Swallow, Act::Swallow, Act::Swallow, Act::Fire]);
        assert_eq!(run(&[(VK_RWIN.0, true), (VK_RWIN.0, false)]), [Act::Swallow, Act::Fire]);
    }

    #[test]
    fn combos_reach_the_shell_intact() {
        // Win+E: Win replayed right before E, then everything passes, including the Win release.
        assert_eq!(run(&[(WIN, true), (E, true), (E, false), (WIN, false)]), [Act::Swallow, Act::Replay(WIN), Act::Pass, Act::Pass]);
        // Win+Shift+S: only the first key replays Win; repeats after that pass.
        assert_eq!(
            run(&[(WIN, true), (0x10, true), (WIN, true), (0x53, true), (0x53, false), (0x10, false), (WIN, false)]),
            [Act::Swallow, Act::Replay(WIN), Act::Pass, Act::Pass, Act::Pass, Act::Pass, Act::Pass]
        );
        // typing without Win, and a stray Win up (e.g. pressed before the hook existed)
        assert_eq!(run(&[(E, true), (E, false), (WIN, false)]), [Act::Pass; 3]);
        // a combo doesn't poison the next lone tap
        assert_eq!(run(&[(WIN, true), (E, true), (WIN, false), (WIN, true), (WIN, false)])[3..], [Act::Swallow, Act::Fire]);
    }
}
