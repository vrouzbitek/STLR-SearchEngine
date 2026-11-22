//! User settings: `%APPDATA%/com.stlr.searchengine/settings.json`. Unknown/missing fields fall back to
//! defaults, so older or hand-edited files keep working. The API key lives only here (never in the repo).
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// Shortcuts offered in the UI (all parse as Tauri global-shortcut strings).
pub const SHORTCUTS: [&str; 4] = ["Alt+Space", "Ctrl+Space", "Alt+Q", "Ctrl+Alt+Space"];

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum LearningMode {
    Calm,
    #[default]
    Balanced,
    Aggressive,
}

/// Visual theme for both windows; also drives native DWM (backdrop, corners, dark title bar).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Acrylic,
    Stlr,
    Dark,
    Light,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    None,
    Groq,
    Gemini,
    Openai, // any OpenAI-compatible endpoint (base_url)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Llm {
    pub provider: Provider,
    pub api_key: String,
    pub model: String,    // empty = provider default
    pub base_url: String, // OpenAI-compatible only; empty = https://api.openai.com/v1
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub shortcut: String,
    /// Mirrors the HKCU Run key (the registry is the source of truth, see autostart.rs).
    pub autostart: bool,
    pub learning_mode: LearningMode,
    /// Privacy switch: false = nothing (Wiki/DDG/LLM) ever leaves the PC.
    pub web_answers: bool,
    pub llm: Llm,
    /// Set by the first-run wizard (Finish or Skip). Backend-owned: `update_settings` never changes it.
    #[serde(rename = "onboarding_completed", alias = "onboardingCompleted")]
    pub onboarding_completed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Acrylic,
            shortcut: SHORTCUTS[0].into(),
            autostart: false,
            learning_mode: LearningMode::Balanced,
            web_answers: true,
            llm: Llm::default(),
            onboarding_completed: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                eprintln!("settings.json invalid ({e}), using defaults");
                Self::default()
            }),
            Err(_) => Self::default(), // first run
        }
    }

    /// Atomic save: write a temp file, then rename over the old one (no half-written settings on crash).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).expect("settings serialize"))?;
        std::fs::rename(tmp, path)
    }
}

/// Weight presets per learning mode (keys of `brain::Weights`). Balanced = the built-in defaults.
pub fn preset(mode: LearningMode) -> [(&'static str, f64); 4] {
    match mode {
        LearningMode::Calm => [("pick_first", 0.6), ("pick_lower", 1.0), ("skip_penalty", 0.15), ("pin_threshold", 2.0)],
        LearningMode::Balanced => [("pick_first", 1.0), ("pick_lower", 1.5), ("skip_penalty", 0.3), ("pin_threshold", 1.0)],
        LearningMode::Aggressive => [("pick_first", 1.4), ("pick_lower", 2.2), ("skip_penalty", 0.5), ("pin_threshold", 0.8)],
    }
}

pub fn apply_preset(w: &mut crate::brain::Weights, mode: LearningMode) {
    for (k, v) in preset(mode) {
        match k {
            "pick_first" => w.pick_first = v,
            "pick_lower" => w.pick_lower = v,
            "skip_penalty" => w.skip_penalty = v,
            "pin_threshold" => w.pin_threshold = v,
            _ => unreachable!(),
        }
    }
}

/// Write the preset into weights.json, keeping any other hand-tuned keys.
pub fn write_preset(weights_json: &Path, mode: LearningMode) -> std::io::Result<()> {
    let mut v: Value = std::fs::read_to_string(weights_json).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    if !v.is_object() {
        v = Value::Object(Default::default());
    }
    let obj = v.as_object_mut().unwrap();
    for (k, val) in preset(mode) {
        obj.insert(k.into(), val.into());
    }
    std::fs::write(weights_json, serde_json::to_string_pretty(&v).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("stlr-settings-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn save_load_roundtrip() {
        let p = tmp("settings.json");
        let s = Settings {
            theme: Theme::Stlr,
            shortcut: "Ctrl+Space".into(),
            autostart: true,
            learning_mode: LearningMode::Aggressive,
            web_answers: false,
            llm: Llm { provider: Provider::Gemini, api_key: "k-123".into(), model: "m".into(), base_url: String::new() },
            onboarding_completed: true,
        };
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(raw.contains("\"learningMode\": \"aggressive\"") && raw.contains("\"provider\": \"gemini\""));
        assert!(raw.contains("\"theme\": \"stlr\""));
        assert!(raw.contains("\"onboarding_completed\": true"));
        assert!(!p.with_extension("json.tmp").exists(), "temp file renamed away");
    }

    #[test]
    fn missing_partial_and_invalid_files_fall_back_to_defaults() {
        assert_eq!(Settings::load(&tmp("does-not-exist.json")), Settings::default());
        let p = tmp("partial.json");
        std::fs::write(&p, r#"{"shortcut":"Alt+Q","llm":{"provider":"groq"},"futureField":1}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!((s.shortcut.as_str(), s.web_answers, s.llm.provider), ("Alt+Q", true, Provider::Groq));
        assert_eq!(s.theme, Theme::Acrylic, "settings from before themes existed default to Acrylic");
        assert!(!s.onboarding_completed, "missing flag = wizard not done yet");
        let p = tmp("camel.json");
        std::fs::write(&p, r#"{"onboardingCompleted":true}"#).unwrap();
        assert!(Settings::load(&p).onboarding_completed, "camelCase alias accepted");
        let p = tmp("broken.json");
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn learning_mode_updates_weights_and_keeps_other_keys() {
        let p = tmp("weights.json");
        std::fs::write(&p, r#"{"half_life_days": 30, "pick_first": 9}"#).unwrap();
        write_preset(&p, LearningMode::Calm).unwrap();
        let w: crate::brain::Weights = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!((w.half_life_days, w.pick_first, w.pin_threshold), (30.0, 0.6, 2.0));
        let mut live = crate::brain::Weights::default();
        apply_preset(&mut live, LearningMode::Aggressive);
        assert_eq!((live.pick_lower, live.skip_penalty), (2.2, 0.5));
        apply_preset(&mut live, LearningMode::Balanced);
        let d = crate::brain::Weights::default();
        assert_eq!((live.pick_first, live.pick_lower, live.skip_penalty, live.pin_threshold), (d.pick_first, d.pick_lower, d.skip_penalty, d.pin_threshold));
    }
}
