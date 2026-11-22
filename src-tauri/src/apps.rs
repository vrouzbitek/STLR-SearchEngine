use crate::{score, Hit};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

struct App {
    name: String,
    lower: String,
    path: String,
}

// Filled once by a background thread at startup; searches before that return nothing.
static APPS: RwLock<Vec<App>> = RwLock::new(Vec::new());

pub fn index() {
    let mut out = Vec::new();
    for var in ["ProgramData", "APPDATA"] {
        if let Ok(base) = std::env::var(var) {
            walk(&PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs"), &mut out);
        }
    }
    // Same shortcut often exists in both Start Menu roots.
    out.sort_by(|a, b| a.lower.cmp(&b.lower));
    out.dedup_by(|a, b| a.lower == b.lower);
    *APPS.write().unwrap() = out;
}

fn paths() -> Vec<String> {
    APPS.read().unwrap().iter().map(|a| a.path.clone()).collect()
}

/// Resolve every .lnk target once (feeds the "running" dot and cold-start seeding).
pub fn prewarm_targets() {
    for p in paths() {
        crate::switcher::resolve(&p);
    }
}

/// Extract every app icon once so icons are ready before the first keystroke (cold: 20-100ms each).
pub fn prewarm_icons() {
    for p in paths() {
        crate::icons::icon(&p, "app");
    }
}

/// Resolved target exe (lowercase) -> (name, Start Menu path). First entry wins on duplicates.
pub fn by_target() -> std::collections::HashMap<String, (String, String)> {
    let mut m = std::collections::HashMap::new();
    for a in APPS.read().unwrap().iter() {
        if let Some(t) = crate::switcher::resolve(&a.path) {
            m.entry(t).or_insert_with(|| (a.name.clone(), a.path.clone()));
        }
    }
    m
}

fn walk(dir: &Path, out: &mut Vec<App>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&p, out);
            continue;
        }
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext != "lnk" && ext != "exe" {
            continue;
        }
        let Some(name) = p.file_stem().and_then(|s| s.to_str()) else { continue };
        let lower = name.to_lowercase();
        if lower.contains("uninstall") {
            continue;
        }
        out.push(App { name: name.to_string(), lower, path: p.to_string_lossy().into_owned() });
    }
}

pub fn search(query: &str, max: usize) -> Vec<Hit> {
    let q = query.trim().to_lowercase();
    let apps = APPS.read().unwrap();
    let mut hits: Vec<(u8, &App)> = apps.iter().filter_map(|a| score(&a.lower, &q).map(|s| (s, a))).collect();
    hits.sort_by_key(|(s, a)| (*s, a.name.len()));
    hits.into_iter()
        .take(max)
        .map(|(s, a)| Hit { name: a.name.clone(), path: a.path.clone(), kind: "app", score: s as f32, ..Default::default() })
        .collect()
}
