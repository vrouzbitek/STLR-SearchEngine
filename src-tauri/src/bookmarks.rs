//! Browser bookmarks (Chromium `Bookmarks` JSON) for Edge, Chrome and Brave, all profiles. Loaded once at
//! startup in the background, searched in RAM.
//! Plus the user's own links (`links.json` in the app data dir), which rank above everything else they match.
//! shortcut: no live reload; bookmarks added after startup appear on next launch. Add a file watcher if needed.
use crate::{score, Hit};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

struct Bookmark {
    title: String,
    lower: String,
    url: String,
    browser: &'static str,
    /// Chromium `date_last_used` (falls back to `date_added`), µs since 1601. Used to rank "Detected Sites".
    last_used: i64,
    /// From links.json: the user's explicit pick, ranked one full step above an equal bookmark/app match.
    link: bool,
}

static BOOKMARKS: RwLock<Vec<Bookmark>> = RwLock::new(Vec::new());

const BROWSERS: [(&str, &str); 3] = [
    ("Edge", r"Microsoft\Edge\User Data"),
    ("Chrome", r"Google\Chrome\User Data"),
    ("Brave", r"BraveSoftware\Brave-Browser\User Data"),
];

pub fn load(links: &Path) -> usize {
    let mut out = Vec::new();
    if let Ok(json) = std::fs::read_to_string(links) {
        parse_links(&json, &mut out);
    }
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    for (browser, sub) in BROWSERS {
        let Ok(profiles) = std::fs::read_dir(PathBuf::from(&local).join(sub)) else { continue }; // not installed
        for p in profiles.flatten() {
            if let Ok(json) = std::fs::read_to_string(p.path().join("Bookmarks")) {
                parse(&json, browser, &mut out);
            }
        }
    }
    out.sort_by(|a, b| a.url.cmp(&b.url).then(b.link.cmp(&a.link))); // a link wins over the same URL bookmarked
    out.dedup_by(|a, b| a.url == b.url); // same bookmark synced across browsers/profiles
    let n = out.len();
    *BOOKMARKS.write().unwrap() = out;
    n
}

/// `[{"name": "STELLAR ARCHIVE", "url": "https://...", "browser": "Edge"}]`; browser is optional
/// ("Edge" opens in Edge even if it isn't the default browser, anything else uses the default one).
fn parse_links(json: &str, out: &mut Vec<Bookmark>) {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) else {
        eprintln!("links.json must be an array of {{name, url}}");
        return;
    };
    for i in &items {
        let (Some(name), Some(url)) = (i["name"].as_str(), i["url"].as_str()) else { continue };
        if url.starts_with("http://") || url.starts_with("https://") {
            let browser = if i["browser"].as_str().is_some_and(|b| b.eq_ignore_ascii_case("edge")) { "Edge" } else { "Link" };
            out.push(Bookmark { lower: name.to_lowercase(), title: name.into(), url: url.into(), browser, last_used: 0, link: true });
        }
    }
}

/// Browser a bookmarked/linked URL belongs to ("Edge", "Chrome", "Brave", "Link").
pub fn browser_of(url: &str) -> Option<&'static str> {
    BOOKMARKS.read().unwrap().iter().find(|b| b.url == url).map(|b| b.browser)
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
                let date = |k: &str| node[k].as_str().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                let last_used = Some(date("date_last_used")).filter(|d| *d > 0).unwrap_or_else(|| date("date_added"));
                out.push(Bookmark { lower: title.to_lowercase(), title, url: url.to_string(), browser, last_used, link: false });
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

/// Most recently used (or added) bookmarks: (title, url, browser).
pub fn recent(n: usize) -> Vec<(String, String, &'static str)> {
    let all = BOOKMARKS.read().unwrap();
    let mut v: Vec<&Bookmark> = all.iter().collect();
    v.sort_by(|a, b| b.last_used.cmp(&a.last_used));
    v.into_iter().take(n).map(|b| (b.title.clone(), b.url.clone(), b.browser)).collect()
}

pub fn host(url: &str) -> &str {
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
    v.sort_by_key(|(s, b)| (*s, !b.link, b.title.len()));
    v.into_iter()
        .take(max)
        .map(|(s, b)| Hit {
            name: b.title.clone(),
            path: b.url.clone(),
            kind: "bookmark",
            score: s as f32 + if b.link { -0.8 } else { 0.2 },
            detail: Some(format!("{} · {}", b.browser, host(&b.url))),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Tests that swap the global BOOKMARKS list must not run at the same time.
    static GLOBAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `f` with BOOKMARKS swapped for `(title, url)` fixtures, holding the same lock as the tests below.
    pub(crate) fn with_fixture<R>(items: &[(String, String)], f: impl FnOnce() -> R) -> R {
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        *BOOKMARKS.write().unwrap() = items
            .iter()
            .map(|(t, u)| Bookmark { title: t.clone(), lower: t.to_lowercase(), url: u.clone(), browser: "Edge", last_used: 0, link: false })
            .collect();
        f()
    }

    const SAMPLE: &str = r#"{"roots":{
        "bookmark_bar":{"type":"folder","name":"Bar","children":[
            {"type":"url","name":"GitHub","url":"https://github.com/","date_added":"100","date_last_used":"900"},
            {"type":"folder","name":"Music","children":[
                {"type":"url","name":"Playboi Carti – Spotify","url":"https://open.spotify.com/artist/x","date_added":"500","date_last_used":"0"},
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
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        parse(SAMPLE, "Chrome", &mut out);
        *BOOKMARKS.write().unwrap() = out;
        let hits = search("playboi", 5);
        assert_eq!(hits[0].path, "https://open.spotify.com/artist/x");
        assert_eq!(hits[0].detail.as_deref(), Some("Chrome · open.spotify.com"));
        assert_eq!(search("stellarclth", 5).len(), 1, "matches by URL");
        assert!(search("", 5).is_empty());
        // Recency: last used (900) beats added-only (500); never-dated bookmarks come last.
        let r: Vec<_> = recent(2).into_iter().map(|(t, _, _)| t).collect();
        assert_eq!(r, ["GitHub", "Playboi Carti – Spotify"]);
    }

    #[test]
    fn user_links_rank_first_and_keep_their_browser() {
        let _g = GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::new();
        parse_links(r#"[{"name":"STELLAR ARCHIVE","url":"https://drive.google.com/drive/folders/X","browser":"edge"},
                        {"name":"Plain","url":"https://example.com/"},{"name":"bad","url":"file:///C:/x"},{"url":"https://no-name"}]"#, &mut out);
        parse(r#"{"roots":{"b":{"type":"folder","children":[{"type":"url","name":"Steam","url":"https://store.steampowered.com/"}]}}}"#, "Chrome", &mut out);
        parse_links("not json", &mut out); // logged, no panic
        assert_eq!(out.iter().filter(|b| b.link).count(), 2);
        *BOOKMARKS.write().unwrap() = out;
        // "st": the link (prefix) outranks a bookmark and an app with the same prefix match (score 1).
        let hits = search("st", 5);
        assert_eq!(hits[0].name, "STELLAR ARCHIVE");
        assert!(hits[0].score < 1.0 && hits[1].score > 1.0);
        assert_eq!(hits[0].detail.as_deref(), Some("Edge · drive.google.com"));
        assert_eq!(browser_of("https://drive.google.com/drive/folders/X"), Some("Edge"));
        assert_eq!(browser_of("https://example.com/"), Some("Link"));
    }

    // cargo test --lib live_bookmarks -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_bookmarks() {
        let t = std::time::Instant::now();
        println!("loaded {} bookmarks in {:?}", load(Path::new("no-links.json")), t.elapsed());
    }
}
