//! Per-keystroke latency budget for local search (`rank_local` + ghost text), on a fixed fixture index:
//! 600 apps, 3,000 bookmarks and a brain with 2,000 learned picks over 400 distinct queries.
//! Every prefix of every query below is one simulated keystroke; its median time over `RUNS` must stay
//! under `BUDGET`. The fixture comes from a fixed-seed PRNG, so the data is the same on every run and
//! machine, and the median keeps one-off scheduler hiccups (CI, parallel tests) from failing the build.
//! Excluded on purpose: `mark_running` / sub-actions (window + .lnk lookups, cached elsewhere) and IPC.
use crate::brain::{self, Brain, Commit, Target, Weights};
use std::time::{Duration, Instant};

const BUDGET: Duration = Duration::from_millis(1);
const RUNS: usize = 25;

/// Typed one char at a time: apps, bookmarks, calc, PL/EN commands and folders, learned links, no match.
const TYPED: [&str; 10] = [
    "visual studio code",
    "spotify",
    "playboi carti",
    "blender",
    "2+2*3",
    "pobrane",
    "wyłącz komputer",
    "github",
    "chrome",
    "qqzzxxyy no match",
];

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[(self.next() % v.len() as u64) as usize]
    }
}

const VENDORS: [&str; 20] = [
    "Microsoft", "Adobe", "Google", "Mozilla", "JetBrains", "Autodesk", "Valve", "Epic", "Corsair", "Logitech",
    "NVIDIA", "AMD", "Oracle", "Python", "Node", "Git", "Docker", "Unity", "Blackmagic", "Image-Line",
];
const PRODUCTS: [&str; 30] = [
    "Studio", "Code", "Player", "Editor", "Manager", "Center", "Launcher", "Console", "Toolkit", "Browser",
    "Designer", "Monitor", "Recorder", "Converter", "Explorer", "Assistant", "Profiler", "Terminal", "Viewer", "Sync",
    "Updater", "Bridge", "Panel", "Workbench", "Shell", "Inspector", "Debugger", "Builder", "Client", "Server",
];
const WORDS: [&str; 24] = [
    "docs", "music", "shop", "news", "blog", "wiki", "drive", "mail", "photos", "maps", "store", "forum",
    "playlist", "release", "issues", "pull", "tutorial", "recipe", "invoice", "calendar", "chat", "video", "game", "api",
];
const REAL_APPS: [&str; 12] = [
    "Visual Studio Code", "Spotify", "Blender", "Google Chrome", "Microsoft Edge", "Steam", "Discord", "REAPER",
    "Slack", "Notepad++", "OBS Studio", "GitHub Desktop",
];

fn fixture_apps(rng: &mut Rng) -> Vec<String> {
    let mut v: Vec<String> = REAL_APPS.iter().map(|s| s.to_string()).collect();
    while v.len() < 600 {
        let n = rng.next() % 1000;
        v.push(format!("{} {} {n}", rng.pick(&VENDORS), rng.pick(&PRODUCTS)));
    }
    v
}

fn fixture_bookmarks(rng: &mut Rng) -> Vec<(String, String)> {
    let mut v = vec![
        ("Playboi Carti – Spotify".to_string(), "https://open.spotify.com/artist/x".to_string()),
        ("GitHub".to_string(), "https://github.com/".to_string()),
    ];
    while v.len() < 3000 {
        let (a, b, n) = (rng.pick(&WORDS), rng.pick(&WORDS), rng.next() % 10_000);
        v.push((format!("{a} {b} {n} – {}", rng.pick(&VENDORS)), format!("https://{a}.example{}.com/{b}/{n}", n % 50)));
    }
    v
}

/// 2,000 picks over 400 queries (prefixes of real names plus noise), spread over the last 60 days.
fn fixture_brain(rng: &mut Rng, apps: &[String], now: i64) -> Brain {
    let mut b = Brain::in_memory(Weights::default());
    let target = |name: &str| Target { id: format!(r"C:\Fixture\{name}.lnk"), name: name.into(), kind: "app".into() };
    let queries: Vec<(String, usize)> = (0..400)
        .map(|_| {
            let i = (rng.next() % apps.len() as u64) as usize;
            let name = apps[i].to_lowercase();
            let len = 2 + (rng.next() % 8) as usize;
            (name.chars().take(len).collect(), i)
        })
        .collect();
    for k in 0..2000 {
        let (q, i) = &queries[(rng.next() % queries.len() as u64) as usize];
        // Show a few neighbours above the pick so skip penalties land too.
        let picked = (rng.next() % 3) as usize;
        let shown: Vec<Target> = (0..=picked).map(|d| target(&apps[(i + apps.len() - picked + d) % apps.len()])).collect();
        let when = now - 60 * 86_400 + k as i64 * 2_592; // evenly over 60 days, newest last
        b.record(&Commit { query: q.clone(), picked, shown, abandoned: None, abandoned_age_ms: 0 }, "", when);
    }
    // A learned non-matching link (abandon/retype) that gets injected by `top`.
    let spotify = Target { id: "spotify:artist:x".into(), name: "Playboi Carti".into(), kind: "spotify".into() };
    for _ in 0..5 {
        b.record(&Commit { query: "playboi carti".into(), picked: 0, shown: vec![spotify.clone()], abandoned: None, abandoned_age_ms: 0 }, "", now);
    }
    b
}

fn keystroke(b: &Brain, typed: &str) -> (Vec<crate::Hit>, Option<String>) {
    let q = brain::norm(typed);
    (crate::rank_local(b, typed, &q, ""), b.ghost(&q))
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
fn local_search_stays_under_1ms_per_keystroke() {
    let mut rng = Rng(0x5EED_57A1_2026_0001);
    let apps = fixture_apps(&mut rng);
    let marks = fixture_bookmarks(&mut rng);
    let b = fixture_brain(&mut rng, &apps, crate::now());
    crate::apps::set_fixture(&apps);

    crate::bookmarks::tests::with_fixture(&marks, || {
        // The fixture is really searched (guards against a silently empty index making the test pass).
        let (hits, _) = keystroke(&b, "spo");
        assert!(hits.iter().any(|h| h.name == "Spotify"), "{hits:?}");
        assert!(keystroke(&b, "playboi carti").0.iter().any(|h| h.kind == "bookmark"));
        assert!(keystroke(&b, "docs").0.len() >= 5);

        // Warm-up: one-time lazy init (known folders, statics) is startup cost, not keystroke cost.
        for t in TYPED {
            keystroke(&b, t);
        }

        let mut worst = (Duration::ZERO, String::new());
        let mut over = Vec::new();
        let mut total = Duration::ZERO;
        let mut n = 0;
        for t in TYPED {
            let chars: Vec<char> = t.chars().collect();
            for end in 1..=chars.len() {
                let prefix: String = chars[..end].iter().collect();
                let samples: Vec<Duration> = (0..RUNS)
                    .map(|_| {
                        let start = Instant::now();
                        std::hint::black_box(keystroke(&b, std::hint::black_box(&prefix)));
                        start.elapsed()
                    })
                    .collect();
                let m = median(samples);
                total += m;
                n += 1;
                if m > worst.0 {
                    worst = (m, prefix.clone());
                }
                if m > BUDGET {
                    over.push(format!("{prefix:?}: {m:?}"));
                }
            }
        }
        println!("perf_budget: {n} keystrokes, mean median {:?}, worst {:?} on {:?}", total / n as u32, worst.0, worst.1);
        assert!(over.is_empty(), "keystrokes over the {BUDGET:?} budget (median of {RUNS}): {over:#?}");
    });
}
