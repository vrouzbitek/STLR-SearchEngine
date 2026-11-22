//! Cold-start seeding: local, admin-free signals about what the user already uses, so zero-query and
//! 1-2 letter prefixes are useful before the first click. Scores are scaled down by `Weights::seed_scale`.
use crate::brain::Target;
use crate::{apps, switcher};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Source weights (x `Weights::seed_scale`). TASKBAR is also the cap for anything the wizard seeds.
pub const TASKBAR: f64 = 1.5;
pub const OPEN: f64 = 1.2;
pub const STARTUP: f64 = 1.0;
pub const RECENT: f64 = 0.8;
const RECENT_FOLDERS: usize = 5;

#[derive(Serialize, Default, Debug)]
pub struct Report {
    pub taskbar: usize,
    pub open: usize,
    pub startup: usize,
    pub recent: usize,
    pub total: usize, // distinct targets after de-duplication
    pub rows: usize,  // brain rows written (0 when paused or nothing new)
}

fn appdata(sub: &str) -> Option<PathBuf> {
    std::env::var("APPDATA").ok().map(|a| PathBuf::from(a).join(sub))
}

fn lnks(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"))).collect()
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Prefer the Start Menu entry for the same exe, so the seeded id is exactly what search returns.
fn app_target(lnk: &Path, index: &HashMap<String, (String, String)>) -> Target {
    let path = lnk.to_string_lossy().into_owned();
    match switcher::resolve(&path).and_then(|exe| index.get(&exe)) {
        Some((name, p)) => Target { id: p.clone(), name: name.clone(), kind: "app".into() },
        None => Target { id: path, name: stem(lnk), kind: "app".into() },
    }
}

/// A detected seed: target, source weight, and which signal found it ("taskbar" | "open" | "startup" | "recent").
pub type Seed = (Target, f64, &'static str);

/// Collect seeds for auto-seeding (`/brain reseed`, reset): all four signals, de-duplicated.
pub fn collect() -> (Vec<(Target, f64)>, Report) {
    let (seeds, mut r) = detect(true);
    let out: Vec<_> = dedupe(seeds).into_iter().map(|(t, w, _)| (t, w)).collect();
    r.total = out.len();
    (out, r)
}

/// App signals only (taskbar, open windows, startup), de-duplicated: the first-run wizard's "Detected Apps".
pub fn detect_apps() -> Vec<Seed> {
    let mut v = dedupe(detect(false).0);
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.name.to_lowercase().cmp(&b.0.name.to_lowercase())));
    v
}

fn detect(include_recent: bool) -> (Vec<Seed>, Report) {
    let index = apps::by_target();
    let mut r = Report::default();
    let mut seeds: Vec<Seed> = Vec::new();

    if let Some(dir) = appdata(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar") {
        for l in lnks(&dir) {
            seeds.push((app_target(&l, &index), TASKBAR, "taskbar"));
            r.taskbar += 1;
        }
    }
    // Open windows: only apps we can map to the Start Menu index (skips helpers/background tools).
    switcher::refresh();
    for exe in switcher::running_exes() {
        if let Some((name, p)) = index.get(&exe) {
            seeds.push((Target { id: p.clone(), name: name.clone(), kind: "app".into() }, OPEN, "open"));
            r.open += 1;
        }
    }
    if let Some(dir) = appdata(r"Microsoft\Windows\Start Menu\Programs\Startup") {
        for l in lnks(&dir) {
            seeds.push((app_target(&l, &index), STARTUP, "startup"));
            r.startup += 1;
        }
    }
    // Recent: newest shortcuts first, keep those pointing at existing folders.
    if let Some(dir) = appdata(r"Microsoft\Windows\Recent").filter(|_| include_recent) {
        let mut recent: Vec<(std::time::SystemTime, PathBuf)> =
            lnks(&dir).into_iter().filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p))).collect();
        recent.sort_by(|a, b| b.0.cmp(&a.0));
        let mut seen = std::collections::HashSet::new();
        for (_, l) in recent {
            if r.recent >= RECENT_FOLDERS {
                break;
            }
            let Some(target) = switcher::lnk_target(&l.to_string_lossy()) else { continue };
            let tp = Path::new(&target);
            if tp.is_dir() && seen.insert(target.to_lowercase()) {
                let name = tp.file_name().map_or(target.clone(), |n| n.to_string_lossy().into_owned());
                seeds.push((Target { id: target.clone(), name, kind: "folder".into() }, RECENT, "recent"));
                r.recent += 1;
            }
        }
    }

    (seeds, r)
}

/// One entry per target id, keeping the strongest source (and its label).
fn dedupe(seeds: Vec<Seed>) -> Vec<Seed> {
    let mut best: HashMap<String, Seed> = HashMap::new();
    for s in seeds {
        let e = best.entry(s.0.id.to_lowercase()).or_insert_with(|| s.clone());
        if s.1 > e.1 {
            *e = s;
        }
    }
    best.into_values().collect()
}

#[cfg(test)]
mod tests {
    // Real-system check: cargo test --lib live_seeds -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_seeds() {
        crate::apps::index();
        let t = std::time::Instant::now();
        let (seeds, r) = super::collect();
        println!("collect {:?}: {r:?}", t.elapsed());
        let mut seeds = seeds;
        seeds.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (t, w) in &seeds {
            println!("  {w:.1}  {:<7} {}", t.kind, t.name);
        }
        let mut b = crate::brain::Brain::in_memory(Default::default());
        println!("rows written: {}", b.seed(&seeds, 0));
        let top: Vec<_> = b.top("", "", 0, 0.01).into_iter().take(4).map(|(t, s)| format!("{} ({s:.2})", t.name)).collect();
        println!("zero-query top 4: {top:?}");
    }
}
