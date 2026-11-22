mod answers;
mod apps;
mod autostart;
mod bookmarks;
mod brain;
mod calc;
mod clipboard;
mod commands;
mod everything;
mod icons;
mod routes;
mod seeder;
mod settings;
mod subactions;
mod switcher;
mod tabs;
mod tray;

use brain::{Brain, Commit, Stats, Target, Weights};
use serde::Serialize;
use settings::Settings;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, State, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const MAX_RESULTS: usize = 8;
const ZERO_RESULTS: usize = 4;
const CANDIDATES: usize = 30; // textual app matches considered before brain re-ranking
const NO_MATCH: f32 = 5.0; // base score for learned targets whose name doesn't match the query

pub struct AppState {
    brain: Mutex<Brain>,
    ctx: Mutex<String>, // foreground exe captured on Alt+Space, before our window shows
    settings: Mutex<Settings>,
    dir: PathBuf, // %APPDATA%/com.stlr.searchengine
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Hit {
    pub name: String,
    pub path: String,
    pub kind: &'static str, // "app" | "folder" | "file" | "calc" | "system" | "tab" | routes::WEB_KINDS
    pub score: f32,         // lower is better: match score (0..5) minus learned boost; shared scale for merging
    pub running: bool,      // app already has an open window -> Enter switches to it
    pub detail: Option<String>, // right-hand label override ("Edge Tab", "Chrome · github.com")
    pub icon: Option<String>,   // remote/data icon URL (tab favicons); native icons come from get_icon
    pub child: bool,            // ↳ sub-action / recent project shown under the app above it
}

impl Hit {
    fn learned(t: Target, score: f32) -> Self {
        let kind = match t.kind.as_str() {
            "folder" => "folder",
            "file" => "file",
            "action" => "action",
            "recent" => "recent",
            k => routes::WEB_KINDS.iter().find(|w| **w == k).copied().unwrap_or("app"),
        };
        Hit { name: t.name, path: t.id, kind, score, ..Default::default() }
    }
}

/// Both args lowercase. 0 exact, 1 prefix, 2 word-prefix, 3 substring, 4 subsequence.
pub fn score(name: &str, q: &str) -> Option<u8> {
    if q.is_empty() {
        None
    } else if name == q {
        Some(0)
    } else if name.starts_with(q) {
        Some(1)
    } else if name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q)) {
        Some(2)
    } else if name.contains(q) {
        Some(3)
    } else {
        let mut it = name.chars();
        q.chars().all(|c| it.any(|n| n == c)).then_some(4)
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Pinned targets land here: above every match score, below the calculator and an explicit URL.
const PIN_SCORE: f32 = -10.0;

/// Apply learned boosts, pin the strongest learned intent to #1, then stable-sort (ties keep textual order).
fn rerank(b: &Brain, q: &str, ctx: &str, hits: &mut [Hit]) {
    let now = now();
    let mut best: Option<(usize, f64)> = None;
    for (i, h) in hits.iter_mut().enumerate() {
        let boost = b.boost(q, &h.path, ctx, now);
        h.score -= boost as f32;
        if boost >= b.w.pin_threshold && best.is_none_or(|(_, top)| boost > top) {
            best = Some((i, boost));
        }
    }
    // Score (not position) so the pin also wins when merged with Everything results in the frontend.
    if let Some((i, boost)) = best {
        hits[i].score = hits[i].score.min(PIN_SCORE - boost as f32);
    }
    hits.sort_by(|a, b| a.score.total_cmp(&b.score));
}

fn mark_running(hits: &mut [Hit]) {
    for h in hits {
        h.running = h.kind == "app" && switcher::is_running(&h.path);
    }
}

#[derive(Serialize)]
struct Local {
    hits: Vec<Hit>,
    ghost: Option<String>, // most frequent committed query extending the input
}

// Sync, all in RAM (apps index + brain mirror): sub-millisecond.
#[tauri::command]
fn search_local(query: String, st: State<'_, AppState>) -> Local {
    let q = brain::norm(&query);
    let ctx = st.ctx.lock().unwrap().clone();
    let b = st.brain.lock().unwrap();
    let mut hits = apps::search(&q, CANDIDATES);
    hits.extend(routes::calc(&query));
    hits.extend(commands::search(&q));
    hits.extend(tabs::search(&q, MAX_RESULTS));
    hits.extend(bookmarks::search(&q, MAX_RESULTS));
    hits.extend(routes::web(&query));
    // Learned targets whose name doesn't match (abandon/retype links, learned files/folders/web).
    // Sub-actions only ever appear under their app, never as standalone top-level results.
    for (t, _) in b.top(&q, &ctx, now(), b.w.inject_threshold) {
        if !matches!(t.kind.as_str(), "action" | "recent") && !hits.iter().any(|h| h.path.eq_ignore_ascii_case(&t.id)) {
            hits.push(Hit::learned(t, NO_MATCH));
        }
    }
    rerank(&b, &q, &ctx, &mut hits);
    hits.truncate(MAX_RESULTS);
    mark_running(&mut hits);
    // #1 is an app: show its best 3 sub-actions / recent projects right under it.
    if let Some(app) = hits.first().filter(|h| h.kind == "app") {
        let mut subs = subactions::for_app(&app.path);
        order_children(&b, &q, &ctx, &mut subs);
        subs.truncate(INLINE_CHILDREN);
        hits.splice(1..1, subs);
    }
    Local { hits, ghost: b.ghost(&q) }
}

const INLINE_CHILDREN: usize = 3;

/// Learned boost for this query first; ties keep the default order (primary action, recents, rest).
fn order_children(b: &Brain, q: &str, ctx: &str, subs: &mut Vec<Hit>) {
    let now = now();
    let mut keyed: Vec<(f64, Hit)> = subs.drain(..).map(|h| (b.boost(q, &h.path, ctx, now), h)).collect();
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
    subs.extend(keyed.into_iter().map(|(_, h)| h));
}

/// ArrowRight on an app: its full sub-action list, brain-ordered.
#[tauri::command]
fn subactions(path: String, query: String, st: State<'_, AppState>) -> Vec<Hit> {
    let mut subs = subactions::for_app(&path);
    let ctx = st.ctx.lock().unwrap().clone();
    order_children(&st.brain.lock().unwrap(), &brain::norm(&query), &ctx, &mut subs);
    subs
}

// Async so Everything IPC never blocks local results.
#[tauri::command]
async fn search_files(query: String, app: AppHandle) -> Vec<Hit> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut hits = everything::search(&query, MAX_RESULTS);
        let st = app.state::<AppState>();
        let ctx = st.ctx.lock().unwrap().clone();
        rerank(&st.brain.lock().unwrap(), &brain::norm(&query), &ctx, &mut hits);
        hits
    })
    .await
    .unwrap_or_default()
}

#[derive(Serialize)]
struct Zero {
    hits: Vec<Hit>,
    paused: bool,
}

/// Predictions for the empty bar + fresh open-window snapshot. Empty brain -> no rows (bar stays collapsed).
#[tauri::command]
async fn zero_query(app: AppHandle) -> Zero {
    tauri::async_runtime::spawn_blocking(move || {
        switcher::refresh();
        let st = app.state::<AppState>();
        let ctx = st.ctx.lock().unwrap().clone();
        let b = st.brain.lock().unwrap();
        let mut hits: Vec<Hit> = b
            .top("", &ctx, now(), 0.01)
            .into_iter()
            .filter(|(t, _)| routes::is_web(&t.kind) || t.kind == "action" || Path::new(&t.id).exists())
            .take(ZERO_RESULTS)
            .map(|(t, boost)| Hit::learned(t, -(boost as f32)))
            .collect();
        mark_running(&mut hits);
        Zero { hits, paused: b.paused() }
    })
    .await
    .unwrap_or(Zero { hits: vec![], paused: false })
}

/// Only called on Enter / click. Esc and focus loss never reach this.
#[tauri::command]
async fn commit(commit: Commit, app: AppHandle) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let st = app.state::<AppState>();
        let ctx = st.ctx.lock().unwrap().clone();
        st.brain.lock().unwrap().record(&commit, &ctx, now());
    })
    .await;
}

#[tauri::command]
fn brain_stats(st: State<'_, AppState>) -> Stats {
    st.brain.lock().unwrap().stats(now())
}

/// Scan local signals and seed the brain. The scan runs without holding the brain lock.
fn reseed(app: &AppHandle) -> seeder::Report {
    let (seeds, mut report) = seeder::collect();
    report.rows = app.state::<AppState>().brain.lock().unwrap().seed(&seeds, now());
    report
}

/// Wipe, then immediately cold-start seed again (as on first run).
#[tauri::command]
async fn brain_reset(app: AppHandle) -> seeder::Report {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>().brain.lock().unwrap().reset();
        reseed(&app)
    })
    .await
    .unwrap_or_default()
}

/// Manual rescan; never lowers or erases what was learned.
#[tauri::command]
async fn brain_reseed(app: AppHandle) -> seeder::Report {
    tauri::async_runtime::spawn_blocking(move || reseed(&app)).await.unwrap_or_default()
}

/// `paused: None` toggles. Returns the new state. Shared by tray, settings panel and `/brain pause`.
pub fn set_brain_paused(app: &AppHandle, paused: Option<bool>) -> bool {
    let next = {
        let st = app.state::<AppState>();
        let mut b = st.brain.lock().unwrap();
        let next = paused.unwrap_or(!b.paused());
        b.set_paused(next);
        next
    };
    tray::sync(app);
    next
}

#[tauri::command]
fn brain_pause(paused: Option<bool>, app: AppHandle) -> bool {
    set_brain_paused(&app, paused)
}

pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let r = autostart::set(enabled);
    let st = app.state::<AppState>();
    let mut s = st.settings.lock().unwrap();
    s.autostart = autostart::is_enabled(); // registry is the truth, even if the write failed
    let _ = s.save(&st.dir.join("settings.json"));
    drop(s);
    tray::sync(app);
    r
}

/// Swap the global shortcut. On failure (e.g. taken by another app) the old one stays registered.
fn apply_shortcut(app: &AppHandle, old: &str, new: &str) -> Result<(), String> {
    if !settings::SHORTCUTS.contains(&new) {
        return Err(format!("unsupported shortcut {new}"));
    }
    let gs = app.global_shortcut();
    let _ = gs.unregister(old);
    gs.register(new).map_err(|e| {
        let _ = gs.register(old);
        format!("{new} is not available ({e})")
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    paused: bool,
    autostart: bool,
    browsers: Vec<tabs::BrowserStatus>,
    extension_path: String,
}

/// `extension/` folder: installed copy (bundle resource) in release, the repo folder in `tauri dev`.
fn extension_dir(app: &AppHandle) -> PathBuf {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("extension");
    let installed = app.path().resource_dir().map(|d| d.join("extension")).ok().filter(|d| d.join("manifest.json").exists());
    let dir = if cfg!(debug_assertions) { Some(repo.clone()).filter(|d| d.exists()).or(installed) } else { installed };
    let dir = dir.unwrap_or(repo);
    dir.canonicalize().unwrap_or(dir)
}

/// Live state for the settings panel (polled ~1s).
#[tauri::command]
fn get_status(app: AppHandle) -> Status {
    let st = app.state::<AppState>();
    let ext = extension_dir(&app);
    Status {
        paused: st.brain.lock().unwrap().paused(),
        autostart: autostart::is_enabled(),
        browsers: tabs::status(),
        extension_path: ext.to_string_lossy().trim_start_matches(r"\\?\").to_string(),
    }
}

#[tauri::command]
fn get_settings(st: State<'_, AppState>) -> Settings {
    let mut s = st.settings.lock().unwrap().clone();
    s.autostart = autostart::is_enabled();
    s
}

/// Apply every changed field, then persist. Returns what is actually in effect.
#[tauri::command]
fn update_settings(new: Settings, app: AppHandle) -> Result<Settings, String> {
    let st = app.state::<AppState>();
    let old = st.settings.lock().unwrap().clone();
    let mut applied = new.clone();
    let mut err = None;
    if new.shortcut != old.shortcut {
        if let Err(e) = apply_shortcut(&app, &old.shortcut, &new.shortcut) {
            applied.shortcut = old.shortcut.clone();
            err = Some(e);
        }
    }
    if new.autostart != autostart::is_enabled() {
        if let Err(e) = autostart::set(new.autostart) {
            err = Some(e);
        }
    }
    applied.autostart = autostart::is_enabled();
    if new.learning_mode != old.learning_mode {
        settings::apply_preset(&mut st.brain.lock().unwrap().w, new.learning_mode);
        if let Err(e) = settings::write_preset(&st.dir.join("weights.json"), new.learning_mode) {
            err = Some(format!("weights.json: {e}"));
        }
    }
    if new.theme != old.theme {
        apply_theme(&app, new.theme);
    }
    if new.web_answers != old.web_answers || new.llm != old.llm {
        answers::configure(new.web_answers, &new.llm);
    }
    applied.save(&st.dir.join("settings.json")).map_err(|e| format!("settings.json: {e}"))?;
    *st.settings.lock().unwrap() = applied.clone();
    tray::sync(&app);
    err.map_or(Ok(applied), Err)
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    show_settings(&app);
}

#[tauri::command]
async fn get_icon(path: String, kind: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || icons::icon(&path, &kind)).await.ok().flatten()
}

/// `confirmed` must be true for shutdown/restart/empty-bin (frontend asks for a second Enter).
#[tauri::command]
async fn run_system(path: String, confirmed: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || commands::run(&path, confirmed)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn copy_text(text: String) -> Result<(), String> {
    clipboard::copy_text(&text)
}

/// Debounced by the frontend; network runs on a blocking thread and never touches local search.
#[tauri::command]
async fn instant_answer(query: String) -> Option<answers::Answer> {
    tauri::async_runtime::spawn_blocking(move || answers::lookup(&query)).await.ok().flatten()
}

/// Web: open URI (spotify:, https:). Apps: focus the existing window if there is one, else launch. Files: open.
#[tauri::command]
async fn launch(path: String, kind: String) -> Result<&'static str, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if routes::is_web(&kind) {
            return tauri_plugin_opener::open_url(&path, None::<&str>).map(|_| "opened").map_err(|e| e.to_string());
        }
        if kind == "tab" {
            return if tabs::focus(&path) { Ok("focused") } else { Err("browser disconnected".into()) };
        }
        if kind == "system" {
            return Err("use run_system".into());
        }
        if kind == "action" {
            return subactions::launch(&path).map(|_| "launched");
        }
        if (kind == "app" || switcher::is_app_path(&path)) && switcher::focus_existing(&path) {
            return Ok("switched");
        }
        tauri_plugin_opener::open_path(&path, None::<&str>).map(|_| "launched").map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Native look of the search bar per theme, switched live (DWM attributes are just window state, ~µs):
/// - acrylic: Win11 compositor-native Acrylic (DWMSBT_TRANSIENTWINDOW), not the legacy
///   SetWindowCompositionAttribute blur that lags on resize; rounded corners.
/// - stlr: no backdrop (CSS paints solid #000), square corners.
/// - dark / light: no backdrop, rounded corners; light also switches the immersive dark mode off.
/// shortcut: silently no-op before Win11 22H2 (attributes unsupported), add a fallback if that matters.
fn apply_dwm(w: &tauri::WebviewWindow, theme: settings::Theme) {
    use settings::Theme;
    use windows::Win32::Graphics::Dwm::*;
    use windows::Win32::UI::Controls::MARGINS;
    let Ok(h) = w.hwnd() else { return };
    let hwnd = windows::Win32::Foundation::HWND(h.0 as _); // tauri's windows-crate version may differ from ours
    let set = |attr: DWMWINDOWATTRIBUTE, val: i32| unsafe {
        let _ = DwmSetWindowAttribute(hwnd, attr, &val as *const i32 as _, 4);
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, (theme != Theme::Light) as i32);
    set(DWMWA_WINDOW_CORNER_PREFERENCE, if theme == Theme::Stlr { DWMWCP_DONOTROUND.0 } else { DWMWCP_ROUND.0 });
    set(DWMWA_SYSTEMBACKDROP_TYPE, if theme == Theme::Acrylic { DWMSBT_TRANSIENTWINDOW.0 } else { DWMSBT_NONE.0 });
    let m = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
    }
}

/// Apply a theme to both windows and tell their webviews (CSS tokens switch via `data-theme`).
fn apply_theme(app: &AppHandle, theme: settings::Theme) {
    if let Some(w) = app.get_webview_window("main") {
        apply_dwm(&w, theme);
    }
    if let Some(w) = app.get_webview_window("settings") {
        let light = theme == settings::Theme::Light;
        let _ = w.set_theme(Some(if light { tauri::Theme::Light } else { tauri::Theme::Dark }));
    }
    let _ = app.emit("theme-changed", theme);
}

// The windows are created once (hidden) and only shown/hidden afterwards, never destroyed.
fn toggle(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
    } else {
        show_search(app);
    }
}

pub fn show_search(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else { return };
    // Context = the app the user was in; must be read before our window takes the foreground.
    *app.state::<AppState>().ctx.lock().unwrap() = switcher::foreground_exe().unwrap_or_default();
    let _ = w.set_size(LogicalSize::new(680.0, 72.0)); // back to bar-only before centering
    let _ = w.center();
    let _ = w.show();
    let _ = w.set_focus();
    let _ = app.emit("focus-input", ());
}

pub fn show_settings(app: &AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.center();
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        let _ = app.emit_to("settings", "settings-opened", ());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let w: Weights = match std::fs::read_to_string(dir.join("weights.json")) {
                Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                    eprintln!("weights.json invalid ({e}), using defaults");
                    Weights::default()
                }),
                Err(_) => Weights::default(),
            };
            let brain = Brain::open(&dir.join("brain.db"), w.clone()).unwrap_or_else(|e| {
                eprintln!("brain.db unavailable ({e}), learning only for this session");
                Brain::in_memory(w)
            });
            let mut settings = Settings::load(&dir.join("settings.json"));
            settings.autostart = autostart::is_enabled();
            answers::configure(settings.web_answers, &settings.llm);
            // Saved shortcut, falling back to the default if another app owns it.
            let gs = app.global_shortcut();
            if let Err(e) = gs.register(settings.shortcut.as_str()) {
                eprintln!("shortcut {} unavailable ({e}), using {}", settings.shortcut, settings::SHORTCUTS[0]);
                settings.shortcut = settings::SHORTCUTS[0].into();
                let _ = gs.register(settings::SHORTCUTS[0]);
            }
            app.manage(AppState { brain: Mutex::new(brain), ctx: Mutex::new(String::new()), settings: Mutex::new(settings), dir: dir.clone() });
            tray::build(app.handle())?;
            tabs::start();
            // Recent projects: rescan periodically so newly opened files show up under their app.
            std::thread::spawn(|| loop {
                subactions::scan_recent();
                std::thread::sleep(std::time::Duration::from_secs(subactions::RESCAN_SECS));
            });
            let handle = app.handle().clone();
            let packs_override = dir.join("actions.json");
            std::thread::spawn(move || {
                apps::index();
                bookmarks::load();
                apps::prewarm_targets();
                subactions::activate(&packs_override);
                // Cold start: seed only an empty brain (first run). Never delays the window.
                if handle.state::<AppState>().brain.lock().unwrap().is_empty() {
                    let r = reseed(&handle);
                    eprintln!("brain: cold-start seeded {r:?}");
                }
                apps::prewarm_icons();
            });
            let theme = app.state::<AppState>().settings.lock().unwrap().theme;
            apply_theme(app.handle(), theme);
            Ok(())
        })
        .on_window_event(|window, event| match (window.label(), event) {
            ("main", WindowEvent::Focused(false)) => {
                let _ = window.hide();
            }
            // Settings: closing just hides it (kept in RAM like the search bar).
            ("settings", WindowEvent::CloseRequested { api, .. }) => {
                api.prevent_close();
                let _ = window.hide();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            search_local,
            search_files,
            zero_query,
            commit,
            brain_stats,
            brain_reset,
            brain_pause,
            brain_reseed,
            get_settings,
            update_settings,
            get_status,
            open_settings,
            run_system,
            subactions,
            copy_text,
            instant_answer,
            get_icon,
            launch
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picking_a_subaction_promotes_it_under_the_app() {
        let mut b = Brain::in_memory(Weights::default());
        let child = |name: &str, kind: &'static str| Hit { name: name.into(), path: format!("act:{name}"), kind, child: true, ..Default::default() };
        let defaults = || vec![child("Start new project", "action"), child("a.rpp", "recent"), child("b.rpp", "recent")];
        // User picks the 3rd sibling once; the frontend sends only siblings as `shown`.
        let shown = defaults();
        let commit = Commit {
            query: "reaper".into(),
            picked: 2,
            shown: shown.iter().map(|h| Target { id: h.path.clone(), name: h.name.clone(), kind: h.kind.into() }).collect(),
            abandoned: None,
            abandoned_age_ms: 0,
        };
        assert!(b.record(&commit, "", now()));
        let mut next = defaults();
        order_children(&b, "reaper", "", &mut next);
        assert_eq!(next[0].name, "b.rpp");
        assert_eq!(b.boost("reaper", r"C:\SM\REAPER.lnk", "", now()), 0.0, "the app itself is never penalized");
        // Learned sub-actions are never injected as standalone top-level results.
        assert!(b.top("reaper", "", now(), b.w.inject_threshold).iter().all(|(t, _)| t.kind == "recent" || t.kind == "action"));
    }

    #[test]
    fn one_spotify_pick_pins_it_to_top() {
        let mut b = Brain::in_memory(Weights::default());
        let file = Hit { name: "playboi carti.mp3".into(), path: r"D:\m\playboi carti.mp3".into(), kind: "file", score: 1.0, ..Default::default() };
        let fresh = || {
            let mut v = vec![file.clone()];
            v.extend(routes::web("playboi carti"));
            v
        };
        let mut shown = fresh();
        rerank(&b, "playboi carti", "", &mut shown);
        let idx = shown.iter().position(|h| h.kind == "spotify").unwrap();
        assert!(idx > 1, "spotify starts below the file and google");
        let commit = Commit {
            query: "playboi carti".into(),
            picked: idx,
            shown: shown.iter().map(|h| Target { id: h.path.clone(), name: h.name.clone(), kind: h.kind.into() }).collect(),
            abandoned: None,
            abandoned_age_ms: 0,
        };
        assert!(b.record(&commit, "", now()));
        let mut next = fresh();
        rerank(&b, "playboi carti", "", &mut next);
        assert_eq!(next[0].kind, "spotify");
        // And a shorter prefix now suggests it too (injected as a learned target).
        assert!(b.top("playb", "", now(), b.w.inject_threshold).iter().any(|(t, _)| t.kind == "spotify"));
        // Calculator still beats any pin.
        let mut with_calc = vec![routes::calc("2*3").unwrap()];
        with_calc.extend(fresh());
        rerank(&b, "playboi carti", "", &mut with_calc);
        assert_eq!(with_calc[0].kind, "calc");
    }

    #[test]
    fn score_order() {
        assert_eq!(score("spotify", "spotify"), Some(0));
        assert_eq!(score("spotify", "spo"), Some(1));
        assert_eq!(score("playboi carti", "car"), Some(2));
        assert_eq!(score("blender", "lend"), Some(3));
        assert_eq!(score("visual studio code", "vsc"), Some(4));
        assert_eq!(score("reaper", "xyz"), None);
        assert_eq!(score("reaper", ""), None);
    }

    // Real-machine smoke test: `cargo test -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn smoke_real_system() {
        let t = std::time::Instant::now();
        crate::apps::index();
        println!("index: {:?}", t.elapsed());
        for q in ["reaper", "blend", "spot", "vsc"] {
            let t = std::time::Instant::now();
            let hits = crate::apps::search(q, 6);
            println!("apps {q:?} in {:?}: {:?}", t.elapsed(), hits.iter().map(|h| &h.name).collect::<Vec<_>>());
        }
        let hit = crate::apps::search("blender", 1).pop().expect("blender in start menu");
        let t = std::time::Instant::now();
        let icon = crate::icons::icon(&hit.path, "app").expect("icon");
        println!("icon cold {:?}, {} bytes", t.elapsed(), icon.len());
        let t = std::time::Instant::now();
        crate::icons::icon(&hit.path, "app").unwrap();
        println!("icon cached {:?}", t.elapsed());
        assert!(crate::icons::icon(r"C:\Windows", "folder").is_some());
        assert!(crate::icons::icon(r"C:\nope\x.pdf", "file").is_some());
        let t = std::time::Instant::now();
        println!("everything: {:?} in {:?}", crate::everything::search("windows", 6).len(), t.elapsed());
        let spotify = crate::apps::search("spotify", 1).pop().unwrap().path;
        println!("spotify lnk -> {:?}", crate::switcher::resolve(&spotify));
        let t = std::time::Instant::now();
        crate::switcher::refresh();
        println!("window snapshot {:?}; spotify running: {}", t.elapsed(), crate::switcher::is_running(&spotify));
        let t = std::time::Instant::now();
        println!("foreground: {:?} in {:?}", crate::switcher::foreground_exe(), t.elapsed());
    }
}
