//! Instant Answers. Providers are tried in priority order; the first concrete answer wins.
//! To add a source (e.g. an LLM configured in settings), implement `AnswerProvider` and list it in `providers()`.
//! Always called off the UI thread (blocking HTTP), never on the local-results path.
use crate::routes::encode;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Answer {
    pub text: String,
    pub source: String,
    pub url: String,
}

pub trait AnswerProvider: Send + Sync {
    fn answer(&self, query: &str) -> Option<Answer>;
}

const MAX_CHARS: usize = 280;

/// Ignored by the relevance check: they never appear in the answer text itself.
const QUESTION_WORDS: [&str; 22] = [
    "kto", "ile", "jak", "gdzie", "kiedy", "dlaczego", "czy", "jaki", "jaka", "jakie", "który", "która", "które", "what",
    "who", "how", "when", "where", "why", "which", "the", "does",
];

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(2500)))
        .user_agent("STLR-SearchEngine/0.1 (desktop launcher)") // Wikipedia requires a descriptive UA
        .build()
        .into()
});

static PROVIDERS: LazyLock<Vec<Box<dyn AnswerProvider>>> = LazyLock::new(providers);
static CACHE: LazyLock<Mutex<HashMap<String, Option<Answer>>>> = LazyLock::new(Default::default);

/// Free, key-less sources only. Future: build this list from user settings.
fn providers() -> Vec<Box<dyn AnswerProvider>> {
    vec![Box::new(DuckDuckGo), Box::new(Wikipedia { lang: "pl" }), Box::new(Wikipedia { lang: "en" })]
}

pub fn lookup(query: &str) -> Option<Answer> {
    let q = crate::brain::norm(query);
    if let Some(hit) = CACHE.lock().unwrap().get(&q) {
        return hit.clone();
    }
    let a = PROVIDERS.iter().find_map(|p| p.answer(&q));
    CACHE.lock().unwrap().insert(q, a.clone());
    a
}

fn get_json(url: &str) -> Option<Value> {
    AGENT.get(url).call().ok()?.body_mut().read_json().ok()
}

/// Trim to MAX_CHARS, preferring a sentence end, else a word boundary + ellipsis.
fn clip(text: &str) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= MAX_CHARS {
        return t;
    }
    let cut: String = t.chars().take(MAX_CHARS).collect();
    if let Some(i) = cut.rfind(". ").filter(|&i| i > MAX_CHARS / 3) {
        return cut[..=i].to_string();
    }
    format!("{}…", cut.rsplit_once(' ').map_or(cut.as_str(), |(a, _)| a))
}

/// A strict majority of query words (len >= 3, matched by their first 4 chars to tolerate Polish inflection)
/// must appear in the answer, so a 2-word query doesn't get a random article.
fn relevant(query: &str, title: &str, text: &str) -> bool {
    let hay = format!("{title} {text}").to_lowercase();
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| w.chars().count() >= 3 && !QUESTION_WORDS.contains(w))
        .map(|w| w.chars().take(4).collect())
        .collect();
    if words.is_empty() {
        return true;
    }
    let found = words.iter().filter(|w| hay.contains(w.as_str())).count();
    found * 2 > words.len()
}

pub struct DuckDuckGo;

impl AnswerProvider for DuckDuckGo {
    fn answer(&self, q: &str) -> Option<Answer> {
        let v = get_json(&format!("https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1", encode(q)))?;
        let s = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
        let page = format!("https://duckduckgo.com/?q={}", encode(q));
        if !s("Answer").is_empty() {
            return Some(Answer { text: clip(&s("Answer")), source: "DuckDuckGo".into(), url: page });
        }
        let text = s("AbstractText");
        if text.is_empty() || !relevant(q, &s("Heading"), &text) {
            return None;
        }
        let url = Some(s("AbstractURL")).filter(|u| !u.is_empty()).unwrap_or(page);
        let source = Some(s("AbstractSource")).filter(|u| !u.is_empty()).unwrap_or("DuckDuckGo".into());
        Some(Answer { text: clip(&text), source, url })
    }
}

pub struct Wikipedia {
    pub lang: &'static str,
}

impl AnswerProvider for Wikipedia {
    fn answer(&self, q: &str) -> Option<Answer> {
        let url = format!(
            "https://{}.wikipedia.org/w/api.php?action=query&format=json&generator=search&gsrsearch={}&gsrlimit=1\
             &prop=extracts&exintro=1&explaintext=1&exsentences=3&redirects=1",
            self.lang,
            encode(q)
        );
        let v = get_json(&url)?;
        // `pages` is keyed by page id; search rank lives in `index`.
        let page = v["query"]["pages"].as_object()?.values().min_by_key(|p| p["index"].as_u64().unwrap_or(u64::MAX))?;
        let title = page["title"].as_str()?;
        let text = page["extract"].as_str()?.trim();
        let disambiguation = text.contains("może odnosić się do") || text.contains("may refer to");
        if text.is_empty() || disambiguation || !relevant(q, title, text) {
            return None;
        }
        Some(Answer {
            text: clip(text),
            source: "Wikipedia".into(),
            url: format!("https://{}.wikipedia.org/wiki/{}", self.lang, encode(&title.replace(' ', "_"))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance_filter() {
        assert!(relevant("chomiki długość życia", "Chomik syryjski", "Chomiki żyją 2-3 lata. Długość ciała..."));
        assert!(!relevant("my documents folder", "Folder", "A folder is a container"));
        assert!(relevant("ab", "x", "y")); // nothing to check
        assert!(relevant("kto napisał pan tadeusz?", "Pan Tadeusz", "Pan Tadeusz – poemat epicki Adama Mickiewicza"));
    }

    #[test]
    fn clipping() {
        assert_eq!(clip("short  text"), "short text");
        let long = format!("{}. {}", "a".repeat(150), "b ".repeat(200));
        assert!(clip(&long).ends_with('.'));
        assert!(clip(&"word ".repeat(100)).ends_with('…'));
    }

    // Live network check: cargo test --lib live_answers -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_answers() {
        for q in ["chomiki długość życia", "kto napisał pan tadeusz", "playboi carti", "asdkjh qwelkj"] {
            let t = std::time::Instant::now();
            let a = lookup(q);
            println!("{q:?} ({:?}) -> {:?}", t.elapsed(), a.map(|a| (a.source, a.text, a.url)));
        }
    }
}
