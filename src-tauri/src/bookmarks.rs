//! Browser bookmarks (Chromium `Bookmarks` JSON) for Edge, Chrome and Brave, all profiles. Loaded once at
//! startup in the background, searched in RAM.
//! shortcut: no live reload; bookmarks added after startup appear on next launch. Add a file watcher if needed.
use crate::{score, Hit};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::RwLock;

struct Bookmark {
    title: String,
    lower: String,
    url: String,
    browser: &'static str,
}

static BOOKMARKS: RwLock<Vec<Bookmark>> = RwLock::new(Vec::new());

const BROWSERS: [(&str, &str); 3] = [
    ("Edge", r"Microsoft\Edge\User Data"),
    ("Chrome", r"Google\Chrome\User Data"),
    ("Brave", r"BraveSoftware\Brave-Browser\User Data"),
];

pub fn load() -> usize {
    let Ok(local) = std::env::var("LOCALAPPDATA") else { return 0 };
    let mut out = Vec::new();
    for (browser, sub) in BROWSERS {
        let Ok(profiles) = std::fs::read_dir(PathBuf::from(&local).join(sub)) else { continue }; // not installed
        for p in profiles.flatten() {
            if let Ok(json) = std::fs::read_to_string(p.path().join("Bookmarks")) {
                parse(&json, browser, &mut out);
            }
        }
    }
    out.sort_by(|a, b| a.url.cmp(&b.url));
    out.dedup_by(|a, b| a.url == b.url); // same bookmark synced across browsers/profiles
    let n = out.len();
    *BOOKMARKS.write().unwrap() = out;
    n
}

fn parse(json: &str, browser: &'static str, out: &mut Vec<Bookmark>) {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return };
    if let Some(roots) = v["roots"].as_object() {
        for root in roots.values() {
            walk(root, browser, out);
        }
    }
}

fn walk(node: &Value, browser: &'static str, out: &mut Vec<Bookmark>) {
    match node["type"].as_str() {
        Some("url") => {
            let url = node["url"].as_str().unwrap_or("");
            if url.starts_with("http://") || url.starts_with("https://") {
                let title = node["name"].as_str().filter(|s| !s.trim().is_empty()).unwrap_or(url).to_string();
                out.push(Bookmark { lower: title.to_lowercase(), title, url: url.to_string(), browser });
            }
        }
        Some("folder") => {
            for c in node["children"].as_array().into_iter().flatten() {
                walk(c, browser, out);
            }
        }
        _ => {}
    }
}

fn host(url: &str) -> &str {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or("").trim_start_matches("www.")
}

/// Title match on the shared scale (+0.2 so an app with the same name wins ties); URL substring = 3.
pub fn search(q: &str, max: usize) -> Vec<Hit> {
    if q.is_empty() {
        return vec![];
    }
    let all = BOOKMARKS.read().unwrap();
    let mut v: Vec<(u8, &Bookmark)> = all
        .iter()
        .filter_map(|b| score(&b.lower, q).or_else(|| b.url.to_lowercase().contains(q).then_some(3)).map(|s| (s, b)))
        .collect();
    v.sort_by_key(|(s, b)| (*s, b.title.len()));
    v.into_iter()
        .take(max)
        .map(|(s, b)| Hit {
            name: b.title.clone(),
            path: b.url.clone(),
            kind: "bookmark",
            score: s as f32 + 0.2,
            detail: Some(format!("{} · {}", b.browser, host(&b.url))),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"roots":{
        "bookmark_bar":{"type":"folder","name":"Bar","children":[
            {"type":"url","name":"GitHub","url":"https://github.com/"},
            {"type":"folder","name":"Music","children":[
                {"type":"url","name":"Playboi Carti – Spotify","url":"https://open.spotify.com/artist/x"},
                {"type":"url","name":"","url":"https://www.stellarclth.pl/shop"},
                {"type":"url","name":"js","url":"javascript:alert(1)"}]}]},
        "other":{"type":"folder","name":"Other","children":[]},
        "synced":{"type":"folder","name":"Mobile","children":[{"type":"url","name":"Wiki","url":"https://pl.wikipedia.org/"}]}
    },"version":1}"#;

    #[test]
    fn parses_nested_folders_and_skips_non_http() {
        let mut out = Vec::new();
        parse(SAMPLE, "Edge", &mut out);
        let titles: Vec<_> = out.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, ["GitHub", "Playboi Carti – Spotify", "https://www.stellarclth.pl/shop", "Wiki"]);
        parse("not json", "Edge", &mut out); // no panic
        parse("{}", "Edge", &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(host("https://www.stellarclth.pl/shop"), "stellarclth.pl");
    }

    #[test]
    fn searches_title_and_url() {
        let mut out = Vec::new();
        parse(SAMPLE, "Chrome", &mut out);
        *BOOKMARKS.write().unwrap() = out;
        let hits = search("playboi", 5);
        assert_eq!(hits[0].path, "https://open.spotify.com/artist/x");
        assert_eq!(hits[0].detail.as_deref(), Some("Chrome · open.spotify.com"));
        assert_eq!(search("stellarclth", 5).len(), 1, "matches by URL");
        assert!(search("", 5).is_empty());
    }

    // cargo test --lib live_bookmarks -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_bookmarks() {
        let t = std::time::Instant::now();
        println!("loaded {} bookmarks in {:?}", load(), t.elapsed());
    }
}
