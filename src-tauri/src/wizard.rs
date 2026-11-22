//! First-run setup wizard: shows what cold-start seeding *would* use and lets the user pick.
//! Only checked items are seeded, through the same low-weight `Brain::seed` (never overrides learning).
use crate::brain::{Brain, Target};
use crate::{bookmarks, routes, seeder, tabs, Hit};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const MAX_SITES: usize = 8;
const MAX_TAB_SITES: usize = 4; // open tabs first ("in use now"), then recently used bookmarks
const CUSTOM: f64 = seeder::TASKBAR; // something the user typed in is as strong as a pinned app

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub kind: String,   // "app" | "url" | "bookmark"
    pub source: String, // "taskbar" | "open" | "startup" | "tab" | "bookmark" | "custom"
    pub detail: String, // browser / host, shown dimmed
    pub weight: f64,
}

#[derive(Serialize, Default)]
pub struct Candidates {
    pub apps: Vec<Item>,
    pub sites: Vec<Item>,
}

/// URL identity for de-duplication: scheme-less, no `www.`, no trailing slash.
fn url_key(u: &str) -> String {
    u.split("://").nth(1).unwrap_or(u).trim_start_matches("www.").trim_end_matches('/').to_lowercase()
}

pub fn candidates() -> Candidates {
    let apps = seeder::detect_apps()
        .into_iter()
        .map(|(t, w, src)| Item { id: t.id, name: t.name, kind: t.kind, source: src.into(), detail: String::new(), weight: w })
        .collect();
    let tab_sites = tabs::open_sites().into_iter().take(MAX_TAB_SITES).map(|(title, url, browser)| Item {
        name: if title.is_empty() { bookmarks::host(&url).to_string() } else { title },
        detail: format!("{browser} tab · {}", bookmarks::host(&url)),
        id: url,
        kind: "url".into(),
        source: "tab".into(),
        weight: seeder::OPEN,
    });
    let marked = bookmarks::recent(MAX_SITES).into_iter().map(|(title, url, browser)| Item {
        detail: format!("{browser} bookmark · {}", bookmarks::host(&url)),
        name: title,
        id: url,
        kind: "bookmark".into(),
        source: "bookmark".into(),
        weight: seeder::RECENT,
    });
    let mut seen = HashSet::new();
    let sites = tab_sites.chain(marked).filter(|i| seen.insert(url_key(&i.id))).take(MAX_SITES).collect();
    Candidates { apps, sites }
}

/// "Add app" autocomplete result.
pub fn app_item(h: Hit) -> Item {
    Item { id: h.path, name: h.name, kind: "app".into(), source: "custom".into(), detail: String::new(), weight: CUSTOM }
}

/// "Add website": `stellarclth.pl`, `github.com/x`, `https://…`. None if it doesn't look like a URL.
pub fn site_item(input: &str) -> Option<Item> {
    let url = routes::as_url(input.trim())?;
    let host = bookmarks::host(&url).to_string();
    Some(Item { id: url, name: host.clone(), kind: "url".into(), source: "custom".into(), detail: host, weight: CUSTOM })
}

/// Seed exactly the given (checked) items. Weights are clamped: the UI can't inject strong boosts,
/// kinds are limited to launchable ones. Returns rows written (0 while the brain is paused).
pub fn seed(brain: &mut Brain, items: &[Item], now: i64) -> usize {
    let seeds: Vec<(Target, f64)> = items
        .iter()
        .filter(|i| matches!(i.kind.as_str(), "app" | "url" | "bookmark") && !i.id.trim().is_empty())
        .map(|i| (Target { id: i.id.clone(), name: i.name.clone(), kind: i.kind.clone() }, i.weight.clamp(0.0, seeder::TASKBAR)))
        .collect();
    brain.seed(&seeds, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::Weights;

    fn item(id: &str, name: &str, kind: &str, weight: f64) -> Item {
        Item { id: id.into(), name: name.into(), kind: kind.into(), source: "custom".into(), detail: String::new(), weight }
    }

    #[test]
    fn seeds_only_checked_items_with_clamped_low_weights() {
        let mut b = Brain::in_memory(Weights::default());
        let checked = vec![
            item(r"C:\SM\REAPER.lnk", "REAPER", "app", seeder::TASKBAR),
            site_item("stellarclth.pl").unwrap(),
            item("https://github.com/", "GitHub", "bookmark", 50.0), // tampered weight -> clamped
            item("sys:shutdown", "Shut down", "system", 1.0),        // not seedable
        ];
        let rows = seed(&mut b, &checked, 0);
        assert_eq!(rows, 3 * 3, "3 targets x (zero-query + 1-2 letter prefixes)");
        let s = b.w.seed_scale;
        assert_eq!(b.boost("", r"C:\SM\REAPER.lnk", "", 0), 1.5 * s);
        assert_eq!(b.boost("st", "https://stellarclth.pl", "", 0), 1.5 * s, "custom site seeded under its host prefix");
        assert_eq!(b.boost("", "https://github.com/", "", 0), 1.5 * s, "weight clamped to the taskbar level");
        assert_eq!(b.boost("", "sys:shutdown", "", 0), 0.0);
        assert_eq!(b.boost("", "unchecked.lnk", "", 0), 0.0, "anything not sent is not seeded");
        // Still weak: one real pick (1.0) beats any wizard seed.
        assert!(b.boost("", r"C:\SM\REAPER.lnk", "", 0) < b.w.pick_first);
    }

    // cargo test --lib live_wizard -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_wizard() {
        crate::apps::index();
        crate::apps::prewarm_targets();
        crate::bookmarks::load();
        let c = candidates();
        for i in c.apps.iter().chain(&c.sites) {
            println!("{:<9} {:<8} {} {}", i.source, i.kind, i.name, i.detail);
        }
        println!("apps: {}, sites: {}", c.apps.len(), c.sites.len());
    }

    #[test]
    fn custom_sites_and_url_keys() {
        let s = site_item("  stellarclth.pl ").unwrap();
        assert_eq!((s.id.as_str(), s.name.as_str(), s.kind.as_str()), ("https://stellarclth.pl", "stellarclth.pl", "url"));
        assert!(site_item("github.com/vrouzbitek").is_some());
        assert!(site_item("not a site").is_none() && site_item("report.pdf").is_none());
        assert_eq!(url_key("https://www.GitHub.com/"), url_key("http://github.com"));
    }
}
