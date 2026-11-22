//! Update check against GitHub Releases (`latest.json`, minisign-verified by tauri-plugin-updater):
//! shortly after launch (STLR starts with Windows) and every 12h, since it lives in the tray for days.
//! A found update shows a toast and turns the tray item into "Install update vX".
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct Pending(Mutex<Option<Update>>);

pub fn start(app: &AppHandle) {
    app.manage(Pending::default());
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20)); // let the network come up after boot
        loop {
            tauri::async_runtime::block_on(check(&app, false));
            std::thread::sleep(Duration::from_secs(12 * 3600));
        }
    });
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("notification: {e}");
    }
}

/// Background checks toast once per new version; a manual check (tray) always reports.
async fn check(app: &AppHandle, manual: bool) {
    let found = match app.updater() {
        Ok(u) => u.check().await,
        Err(e) => Err(e),
    };
    match found {
        Ok(Some(u)) => {
            let v = u.version.clone();
            let old = app.state::<Pending>().0.lock().unwrap().replace(u).map(|o| o.version);
            crate::tray::set_update(app, Some(&v));
            if manual || old.as_deref() != Some(&v) {
                notify(app, &format!("STLR Search {v} is available"), "Tray icon → Install update");
            }
        }
        Ok(None) if manual => notify(app, "STLR Search is up to date", &format!("Version {}", app.package_info().version)),
        Ok(None) => {}
        Err(e) => {
            eprintln!("update check: {e}");
            if manual {
                notify(app, "Update check failed", &e.to_string());
            }
        }
    }
}

/// Tray item: install the pending update (the NSIS installer closes and relaunches STLR), else check now.
pub async fn install_or_check(app: AppHandle) {
    let pending = app.state::<Pending>().0.lock().unwrap().take();
    let Some(u) = pending else { return check(&app, true).await };
    notify(&app, &format!("Updating STLR Search to {}", u.version), "It restarts by itself when done");
    if let Err(e) = u.download_and_install(|_, _| {}, || {}).await {
        crate::tray::set_update(&app, None);
        notify(&app, "Update failed", &e.to_string());
    }
}
