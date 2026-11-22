//! Web routing actions. Each action is an ordinary `Hit` whose `path` is the URI to open,
//! so the brain learns "playboi carti -> Spotify" exactly like it learns apps and files.
use crate::Hit;

/// Fixed scores: calculator always first, an explicit URL right below it.
pub const CALC_SCORE: f32 = -100.0;
pub const URL_SCORE: f32 = -50.0;
/// Web searches sit below good local matches (0..2) but above weak ones (3..5) until the brain promotes them.
const ROUTE_BASE: f32 = 3.0;

/// Hits whose path is a URI opened with the default handler (learnable, not a file path).
pub const WEB_KINDS: [&str; 6] = ["spotify", "youtube", "google", "instagram", "url", "bookmark"];

pub fn is_web(kind: &str) -> bool {
    WEB_KINDS.contains(&kind)
}

/// RFC 3986 percent-encoding of everything except unreserved characters.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

const FILE_EXTS: [&str; 18] = [
    "exe", "lnk", "pdf", "txt", "png", "jpg", "jpeg", "gif", "zip", "rar", "mp3", "wav", "mp4", "docx", "xlsx", "json", "md", "rs",
];

/// "github.com", "stellarclth.pl/shop", "https://x.y" -> full https URL. File names like "report.pdf" are not URLs.
pub fn as_url(q: &str) -> Option<String> {
    let q = q.trim();
    if q.contains(char::is_whitespace) {
        return None;
    }
    if q.starts_with("http://") || q.starts_with("https://") {
        return Some(q.to_string());
    }
    let host = q.split('/').next()?.to_lowercase();
    let host = host.split(':').next()?; // strip port
    let labels: Vec<&str> = host.split('.').collect();
    let tld = labels.last()?;
    let valid = labels.len() >= 2
        && labels.iter().all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && !FILE_EXTS.contains(tld);
    valid.then(|| format!("https://{q}"))
}

fn hit(name: &str, path: String, kind: &'static str, score: f32) -> Hit {
    Hit { name: name.to_string(), path, kind, score, ..Default::default() }
}

/// Calculator result as a hit (name = result text, copied on Enter).
pub fn calc(query: &str) -> Option<Hit> {
    crate::calc::eval(query).map(|r| hit(&r, format!("calc:{r}"), "calc", CALC_SCORE))
}

/// Web actions for a query. Ids use the normalized query so learning is case-insensitive.
pub fn web(query: &str) -> Vec<Hit> {
    let name = query.trim();
    let q = crate::brain::norm(query);
    if q.is_empty() || q.starts_with('/') {
        return vec![];
    }
    let e = encode(&q);
    let mut v = Vec::new();
    if let Some(url) = as_url(name) {
        v.push(hit(name, url, "url", URL_SCORE));
    }
    v.push(hit(name, format!("https://www.google.com/search?q={e}"), "google", ROUTE_BASE));
    v.push(hit(name, format!("https://www.youtube.com/results?search_query={e}"), "youtube", ROUTE_BASE + 0.05));
    v.push(hit(name, format!("spotify:search:{e}"), "spotify", ROUTE_BASE + 0.1));
    // Single handle-like token -> profile, otherwise keyword search.
    let ig = if q.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') && as_url(&q).is_none() {
        format!("https://www.instagram.com/{q}/")
    } else {
        format!("https://www.instagram.com/explore/search/keyword/?q={e}")
    };
    v.push(hit(name, ig, "instagram", ROUTE_BASE + 0.15));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(as_url("stellarclth.pl").as_deref(), Some("https://stellarclth.pl"));
        assert_eq!(as_url("github.com/user/repo").as_deref(), Some("https://github.com/user/repo"));
        assert_eq!(as_url("http://localhost:1420").as_deref(), Some("http://localhost:1420"));
        for no in ["report.pdf", "250.5", "playboi carti", "spotify", "a..b", "setup.exe"] {
            assert_eq!(as_url(no), None, "{no:?}");
        }
    }

    #[test]
    fn web_actions() {
        let v = web("Playboi Carti");
        let spotify = v.iter().find(|h| h.kind == "spotify").unwrap();
        assert_eq!(spotify.path, "spotify:search:playboi%20carti");
        assert_eq!(spotify.name, "Playboi Carti");
        assert!(v.iter().all(|h| h.kind != "url"));
        assert!(v.iter().any(|h| h.path == "https://www.instagram.com/explore/search/keyword/?q=playboi%20carti"));
        assert!(web("travisscott").iter().any(|h| h.path == "https://www.instagram.com/travisscott/"));
        assert_eq!(web("github.com")[0].kind, "url");
        assert!(web("/brain").is_empty());
        assert_eq!(encode("ł ó"), "%C5%82%20%C3%B3");
    }
}
