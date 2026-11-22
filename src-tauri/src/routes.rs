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

/// "yt austin evans" beats prefix matches (1) and all-words files (2); only an exact local name (0) wins,
/// so "google chrome" still opens Chrome.
const KEYWORD_SCORE: f32 = 0.5;

/// Keyword prefix -> site search: (aliases, label, url prefix the encoded query is appended to, kind).
/// Kind "url" rows get a "Search <label>" chip and the site's favicon.
const SITES: &[(&[&str], &str, &str, &str)] = &[
    (&["yt", "youtube"], "YouTube", "https://www.youtube.com/results?search_query=", "youtube"),
    (&["ytm", "ytmusic"], "YouTube Music", "https://music.youtube.com/search?q=", "url"),
    (&["spotify", "spoti", "spot", "spt"], "Spotify", "spotify:search:", "spotify"),
    (&["google", "gg"], "Google", "https://www.google.com/search?q=", "google"),
    (&["img", "images", "obrazy"], "Google Images", "https://www.google.com/search?tbm=isch&q=", "url"),
    (&["maps", "gmaps", "mapy"], "Google Maps", "https://www.google.com/maps/search/", "url"),
    (&["translate", "tlumacz", "gt"], "Google Translate", "https://translate.google.com/?sl=auto&text=", "url"),
    (&["gmail", "mail"], "Gmail", "https://mail.google.com/mail/u/0/#search/", "url"),
    (&["drive", "gdrive", "dysk"], "Google Drive", "https://drive.google.com/drive/search?q=", "url"),
    (&["ig", "insta", "instagram"], "Instagram", "", "instagram"),
    (&["tt", "tiktok"], "TikTok", "https://www.tiktok.com/search?q=", "url"),
    (&["x", "twitter"], "X", "https://x.com/search?q=", "url"),
    (&["fb", "facebook"], "Facebook", "https://www.facebook.com/search/top?q=", "url"),
    (&["reddit", "rd"], "Reddit", "https://www.reddit.com/search/?q=", "url"),
    (&["tw", "twitch"], "Twitch", "https://www.twitch.tv/search?term=", "url"),
    (&["kick"], "Kick", "https://kick.com/search?query=", "url"),
    (&["pin", "pinterest"], "Pinterest", "https://www.pinterest.com/search/pins/?q=", "url"),
    (&["li", "linkedin"], "LinkedIn", "https://www.linkedin.com/search/results/all/?keywords=", "url"),
    (&["sc", "soundcloud"], "SoundCloud", "https://soundcloud.com/search?q=", "url"),
    (&["genius", "tekst"], "Genius", "https://genius.com/search?q=", "url"),
    (&["netflix", "nf"], "Netflix", "https://www.netflix.com/search?q=", "url"),
    (&["imdb"], "IMDb", "https://www.imdb.com/find/?q=", "url"),
    (&["filmweb", "fw"], "Filmweb", "https://www.filmweb.pl/search#/?query=", "url"),
    (&["wiki", "wikipedia"], "Wikipedia", "https://pl.wikipedia.org/w/index.php?search=", "url"),
    (&["enwiki"], "Wikipedia (EN)", "https://en.wikipedia.org/w/index.php?search=", "url"),
    (&["allegro", "al"], "Allegro", "https://allegro.pl/listing?string=", "url"),
    (&["olx"], "OLX", "https://www.olx.pl/oferty/?q=", "url"),
    (&["amazon", "amz"], "Amazon", "https://www.amazon.pl/s?k=", "url"),
    (&["ceneo"], "Ceneo", "https://www.ceneo.pl/szukaj-", "url"),
    (&["vinted"], "Vinted", "https://www.vinted.pl/catalog?search_text=", "url"),
    (&["ebay"], "eBay", "https://www.ebay.com/sch/i.html?_nkw=", "url"),
    (&["steam"], "Steam", "https://store.steampowered.com/search/?term=", "url"),
    (&["gh", "github"], "GitHub", "https://github.com/search?q=", "url"),
    (&["so", "stackoverflow"], "Stack Overflow", "https://stackoverflow.com/search?q=", "url"),
    (&["npm"], "npm", "https://www.npmjs.com/search?q=", "url"),
    (&["crates", "crate"], "crates.io", "https://crates.io/search?q=", "url"),
    (&["gpt", "chatgpt"], "ChatGPT", "https://chatgpt.com/?q=", "url"),
    (&["claude"], "Claude", "https://claude.ai/new?q=", "url"),
    (&["pplx", "perplexity"], "Perplexity", "https://www.perplexity.ai/search?q=", "url"),
    (&["ddg", "duckduckgo"], "DuckDuckGo", "https://duckduckgo.com/?q=", "url"),
    (&["bing"], "Bing", "https://www.bing.com/search?q=", "url"),
];

/// Single handle-like token -> profile, otherwise keyword search.
fn instagram(q: &str) -> String {
    if q.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') && as_url(q).is_none() {
        format!("https://www.instagram.com/{q}/")
    } else {
        format!("https://www.instagram.com/explore/search/keyword/?q={}", encode(q))
    }
}

/// "yt austin evans" -> YouTube search for "austin evans". Needs a non-empty rest, so "spotify" alone stays the app.
fn keyword(name: &str, q: &str) -> Option<Hit> {
    let (kw, rest) = q.split_once(' ')?;
    let rest = rest.trim();
    let &(_, label, prefix, kind) = SITES.iter().find(|s| s.0.contains(&kw))?;
    if rest.is_empty() {
        return None;
    }
    let shown = name.split_once(char::is_whitespace).map_or(rest, |(_, r)| r.trim());
    let path = if kind == "instagram" { instagram(rest) } else { format!("{prefix}{}", encode(rest)) };
    let kind = WEB_KINDS.into_iter().find(|k| *k == kind)?;
    let mut h = hit(shown, path, kind, KEYWORD_SCORE);
    if kind == "url" {
        h.detail = Some(format!("Search {label}"));
        let host = prefix.split('/').nth(2)?;
        h.icon = Some(format!("https://www.google.com/s2/favicons?domain={host}&sz=64"));
    }
    Some(h)
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
    v.extend(keyword(name, &q));
    v.push(hit(name, format!("https://www.google.com/search?q={e}"), "google", ROUTE_BASE));
    v.push(hit(name, format!("https://www.youtube.com/results?search_query={e}"), "youtube", ROUTE_BASE + 0.05));
    v.push(hit(name, format!("spotify:search:{e}"), "spotify", ROUTE_BASE + 0.1));
    v.push(hit(name, instagram(&q), "instagram", ROUTE_BASE + 0.15));
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

    #[test]
    fn keyword_routes() {
        let first = |q: &str| web(q).into_iter().find(|h| h.score == KEYWORD_SCORE);
        let yt = first("yt Austin Evans").unwrap();
        assert_eq!((yt.kind, yt.name.as_str()), ("youtube", "Austin Evans"));
        assert_eq!(yt.path, "https://www.youtube.com/results?search_query=austin%20evans");
        assert_eq!(first("youtube austin evans").unwrap().path, yt.path);
        for q in ["spotify vorzu", "spoti vorzu", "spot vorzu"] {
            assert_eq!(first(q).unwrap().path, "spotify:search:vorzu", "{q}");
        }
        assert_eq!(first("ig travisscott").unwrap().path, "https://www.instagram.com/travisscott/");
        let tw = first("tw xqc").unwrap();
        assert_eq!((tw.kind, tw.detail.as_deref()), ("url", Some("Search Twitch")));
        assert!(tw.icon.unwrap().contains("domain=www.twitch.tv"));
        for no in ["spotify", "yt", "yt ", "austin evans", "/yt x"] {
            assert!(first(no).is_none(), "{no:?}");
        }
        // Every row builds a URL (kind known, host parseable).
        for s in SITES {
            assert!(first(&format!("{} x", s.0[0])).is_some(), "{}", s.1);
        }
    }
}
