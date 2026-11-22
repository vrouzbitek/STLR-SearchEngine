//! Instant Answers. Providers are tried in priority order; the first concrete answer wins.
//! To add a source (e.g. an LLM configured in settings), implement `AnswerProvider` and list it in `providers()`.
//! Always called off the UI thread (blocking HTTP), never on the local-results path.
use crate::routes::encode;
use serde::Serialize;
use serde_json::Value;
use crate::settings::Provider;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
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

/// Active provider chain, rebuilt from settings. `enabled = false` is the privacy switch.
struct Chain {
    enabled: bool,
    llm: Option<Arc<dyn AnswerProvider>>,
    free: Vec<Arc<dyn AnswerProvider>>,
}

static CHAIN: LazyLock<RwLock<Arc<Chain>>> =
    LazyLock::new(|| RwLock::new(Arc::new(Chain { enabled: true, llm: None, free: free_providers() })));
static CACHE: LazyLock<Mutex<HashMap<String, Option<Answer>>>> = LazyLock::new(Default::default);

/// Free, key-less sources.
fn free_providers() -> Vec<Arc<dyn AnswerProvider>> {
    vec![Arc::new(DuckDuckGo), Arc::new(Wikipedia { lang: "pl" }), Arc::new(Wikipedia { lang: "en" })]
}

/// Apply settings: privacy switch + optional LLM in front of the free sources. Clears the cache.
pub fn configure(web_answers: bool, llm: &crate::settings::Llm) {
    let llm = LlmProvider::from_settings(llm).map(|p| Arc::new(p) as Arc<dyn AnswerProvider>);
    *CHAIN.write().unwrap() = Arc::new(Chain { enabled: web_answers, llm, free: free_providers() });
    CACHE.lock().unwrap().clear();
}

/// Only real questions go to the LLM (question word, "?", or 3+ words), so short searches like
/// "my invoices" never leave the PC towards a paid third party; they still get free Wiki/DDG.
fn llm_eligible(q: &str) -> bool {
    let first = q.split_whitespace().next().unwrap_or("");
    q.ends_with('?') || QUESTION_WORDS.contains(&first) || q.split_whitespace().count() >= 3
}

pub fn lookup(query: &str) -> Option<Answer> {
    let chain = CHAIN.read().unwrap().clone(); // network calls run without holding the lock
    if !chain.enabled {
        return None;
    }
    let q = crate::brain::norm(query);
    if let Some(hit) = CACHE.lock().unwrap().get(&q) {
        return hit.clone();
    }
    let llm = chain.llm.as_ref().filter(|_| llm_eligible(&q)).and_then(|p| p.answer(&q));
    let a = llm.or_else(|| chain.free.iter().find_map(|p| p.answer(&q)));
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

/// Optional LLM (user's own key). Groq and "openai" speak the OpenAI chat API; Gemini its own.
pub struct LlmProvider {
    provider: Provider,
    key: String,
    model: String,
    base_url: String,
}

const LLM_SYSTEM: &str = "You are an instant-answer box in a desktop launcher. Answer in 1-2 short sentences, \
    in the same language as the question. Plain text, no markdown, no preamble. If you are not sure, say so briefly.";

static LLM_AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(8))).build().into()
});

impl LlmProvider {
    fn from_settings(s: &crate::settings::Llm) -> Option<Self> {
        let key = s.api_key.trim().to_string();
        if s.provider == Provider::None || key.is_empty() {
            return None; // empty key = free Wiki/DDG only
        }
        let (default_model, default_url) = match s.provider {
            Provider::Groq => ("llama-3.1-8b-instant", "https://api.groq.com/openai/v1"),
            Provider::Gemini => ("gemini-2.5-flash", "https://generativelanguage.googleapis.com/v1beta"),
            _ => ("gpt-4o-mini", "https://api.openai.com/v1"),
        };
        let pick = |v: &str, d: &str| if v.trim().is_empty() { d.to_string() } else { v.trim().trim_end_matches('/').to_string() };
        let base_url = if s.provider == Provider::Openai { pick(&s.base_url, default_url) } else { default_url.into() };
        Some(Self { provider: s.provider, key, model: pick(&s.model, default_model), base_url })
    }

    fn label(&self) -> String {
        let name = match self.provider {
            Provider::Groq => "Groq",
            Provider::Gemini => "Gemini",
            _ => "LLM",
        };
        format!("{name} · {}", self.model)
    }
}

fn openai_body(model: &str, q: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "system", "content": LLM_SYSTEM}, {"role": "user", "content": q}],
        "max_tokens": 160,
        "temperature": 0.2
    })
}

fn openai_text(v: &Value) -> Option<String> {
    v["choices"][0]["message"]["content"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

fn gemini_body(q: &str) -> Value {
    json!({
        "systemInstruction": {"parts": [{"text": LLM_SYSTEM}]},
        "contents": [{"role": "user", "parts": [{"text": q}]}],
        // thinkingBudget 0: no hidden reasoning tokens eating the tiny output budget (2.5 Flash).
        "generationConfig": {"maxOutputTokens": 160, "temperature": 0.2, "thinkingConfig": {"thinkingBudget": 0}}
    })
}

fn gemini_text(v: &Value) -> Option<String> {
    let parts = v["candidates"][0]["content"]["parts"].as_array()?;
    let t: String = parts.iter().filter_map(|p| p["text"].as_str()).collect();
    Some(t.trim().to_string()).filter(|s| !s.is_empty())
}

impl AnswerProvider for LlmProvider {
    fn answer(&self, q: &str) -> Option<Answer> {
        let text = match self.provider {
            Provider::Gemini => {
                let url = format!("{}/models/{}:generateContent", self.base_url, self.model);
                let mut r = LLM_AGENT.post(&url).header("x-goog-api-key", &self.key).send_json(gemini_body(q)).ok()?;
                gemini_text(&r.body_mut().read_json().ok()?)
            }
            _ => {
                let url = format!("{}/chat/completions", self.base_url);
                let mut r = LLM_AGENT
                    .post(&url)
                    .header("Authorization", &format!("Bearer {}", self.key))
                    .send_json(openai_body(&self.model, q))
                    .ok()?;
                openai_text(&r.body_mut().read_json().ok()?)
            }
        }?;
        // No source page for a generated answer: "open source" falls back to a web search.
        Some(Answer { text: clip(&text), source: self.label(), url: format!("https://www.google.com/search?q={}", encode(q)) })
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
    fn llm_config_and_payloads() {
        use crate::settings::Llm;
        let none = Llm { provider: Provider::Groq, api_key: "  ".into(), ..Default::default() };
        assert!(LlmProvider::from_settings(&none).is_none(), "empty key = free sources only");
        assert!(LlmProvider::from_settings(&Llm { provider: Provider::None, api_key: "k".into(), ..Default::default() }).is_none());
        let groq = LlmProvider::from_settings(&Llm { provider: Provider::Groq, api_key: "k".into(), ..Default::default() }).unwrap();
        assert_eq!((groq.base_url.as_str(), groq.model.as_str()), ("https://api.groq.com/openai/v1", "llama-3.1-8b-instant"));
        let custom = LlmProvider::from_settings(&Llm {
            provider: Provider::Openai,
            api_key: "k".into(),
            model: "my-model".into(),
            base_url: "http://localhost:11434/v1/".into(),
        })
        .unwrap();
        assert_eq!((custom.base_url.as_str(), custom.label().as_str()), ("http://localhost:11434/v1", "LLM · my-model"));

        let body = openai_body("m", "ile żyje chomik?");
        assert_eq!(body["messages"][1]["content"], "ile żyje chomik?");
        assert!(body["messages"][0]["content"].as_str().unwrap().contains("same language"));
        assert_eq!(openai_text(&json!({"choices":[{"message":{"content":" Około 2–3 lat. "}}]})).as_deref(), Some("Około 2–3 lat."));
        assert_eq!(openai_text(&json!({"error":{"message":"bad key"}})), None);
        assert_eq!(gemini_body("q")["contents"][0]["parts"][0]["text"], "q");
        assert_eq!(gemini_text(&json!({"candidates":[{"content":{"parts":[{"text":"A"},{"text":"B"}]}}]})).as_deref(), Some("AB"));
    }

    #[test]
    fn privacy_switch_and_llm_eligibility() {
        configure(false, &Default::default());
        assert_eq!(lookup("kto napisał pan tadeusz"), None, "disabled: returns before any network call");
        configure(true, &Default::default());
        assert!(llm_eligible("chomiki długość życia") && llm_eligible("ile żyje chomik") && llm_eligible("why?"));
        assert!(!llm_eligible("moje faktury") && !llm_eligible("playboi carti"));
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
