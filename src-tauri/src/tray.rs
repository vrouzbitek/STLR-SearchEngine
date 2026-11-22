//! System tray icon + menu. Checkbox states are re-synced via `sync()` whenever pause/autostart/shortcut
//! change from anywhere (tray, settings panel, `/brain pause`).
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

pub struct Tray {
    open: MenuItem<Wry>,
    pause: CheckMenuItem<Wry>,
    autostart: CheckMenuItem<Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open Search", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Pause Brain", true, false, None::<&str>)?;
    let autostart = CheckMenuItem::with_id(app, "autostart", "Start with Windows", true, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &settings, &sep1, &pause, &autostart, &sep2, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().expect("bundle icon"))
        .tooltip("STLR Search")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "open" => crate::show_search(app),
            "settings" => crate::show_settings(app),
            "pause" => {
                crate::set_brain_paused(app, None);
            }
            "autostart" => {
                if let Err(err) = crate::set_autostart(app, !crate::autostart::is_enabled()) {
                    eprintln!("autostart: {err}");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                crate::show_search(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(Tray { open, pause, autostart });
    sync(app);
    Ok(())
}

/// Reflect current state in the menu (checkmarks toggle themselves on click; this makes them truthful).
pub fn sync(app: &AppHandle) {
    let Some(t) = app.try_state::<Tray>() else { return };
    let st = app.state::<crate::AppState>();
    let shortcut = st.settings.lock().unwrap().shortcut.clone();
    let _ = t.open.set_text(format!("Open Search ({shortcut})"));
    let _ = t.pause.set_checked(st.brain.lock().unwrap().paused());
    let _ = t.autostart.set_checked(crate::autostart::is_enabled());
}
