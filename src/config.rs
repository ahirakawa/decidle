use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

use crate::util::{home, read_regular};
use decidle::i18n::{self, Lang};

const DEFAULT_CONFIG: &str = include_str!("../config.default.json");
const MAX_ROUNDS_LIMIT: u32 = 5;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize, Clone)]
pub struct VerificationRule {
    pub words: Vec<String>,
    pub commands: Vec<String>,
}

#[derive(Deserialize, Clone)]
pub struct Config {
    /// Language of everything decidle writes for humans and agents: `ja` or `en`.
    pub language: Lang,
    pub report_dir: String,
    pub report_suffix: String,
    pub conclusion_max_chars: usize,
    /// Upper bound of decisions[].question, so that it fits on a Home card.
    pub question_max_chars: usize,
    pub reply_max_chars: usize,
    pub code_extensions: Vec<String>,
    pub ignore_path_patterns: Vec<String>,
    pub edit_tools: Vec<String>,
    /// A turn is checked when it edits at least this many files.
    pub edited_files_threshold: usize,
    pub tool_call_threshold: usize,
    pub trigger_commands: Vec<String>,
    pub role_dir_marker: String,
    pub review_command: String,
    /// Start of the request that sends a draft to the reviewer. Defaults to the configured
    /// language's marker; readers accept every language's marker as well.
    pub review_marker: String,
    pub verification_rules: Vec<VerificationRule>,
    pub max_rounds: u32,
    pub agent_prompt_markers: Vec<String>,
    /// Command whose `portal` subcommand shows the page inside Maestri. Empty (the default) turns
    /// this off; the human opens the page from the report title on the Home page instead.
    pub portal_command: String,
    /// Port of the Home page (`decidle serve`), on 127.0.0.1.
    pub home_port: u16,
    /// Reports of a session resolved or ended this many days ago are deleted by `decidle serve`
    /// and `decidle clean`. 0 turns this off.
    pub clean_after_days: u64,
    /// Reports whose decisions were never answered are deleted this many days after the session
    /// ended. 0 turns this off. Independent of clean_after_days.
    pub clean_unanswered_after_days: u64,
}

impl Config {
    pub fn clean_settings(&self) -> decidle::home::clean::Settings {
        decidle::home::clean::Settings {
            after_days: self.clean_after_days,
            unanswered_after_days: self.clean_unanswered_after_days,
            report_dir: self.report_dir.clone(),
            lang: self.language,
        }
    }
}

impl Config {
    /// Looks up $DECIDLE_CONFIG, then ~/.config/decidle/config.json.
    /// Top-level keys of the user file override the embedded defaults.
    pub fn load() -> Result<Config, String> {
        let mut merged: Value =
            serde_json::from_str(DEFAULT_CONFIG).map_err(|e| format!("default config: {e}"))?;
        let user_path = match std::env::var("DECIDLE_CONFIG") {
            Ok(p) if !p.is_empty() => Some(PathBuf::from(p)),
            _ => {
                let p = home().join(".config/decidle/config.json");
                p.exists().then_some(p)
            }
        };
        if let Some(path) = user_path {
            let text = read_regular(&path, MAX_CONFIG_BYTES).map_err(|e| format!("config: {e}"))?;
            let user: Value =
                serde_json::from_str(&text).map_err(|e| format!("config {}: {e}", path.display()))?;
            if let (Some(base), Some(over)) = (merged.as_object_mut(), user.as_object()) {
                for (k, v) in over {
                    base.insert(k.clone(), v.clone());
                }
            }
        }
        // review_marker is written by the agent, so its default follows the language. So do the
        // length limits: English takes about twice the characters of Japanese for the same content.
        // A value the user wrote is used as it is, in either language.
        if let Some(base) = merged.as_object_mut() {
            let lang: Lang = base
                .get("language")
                .map(|v| Lang::deserialize(v).map_err(|e| format!("config: language: {e}")))
                .transpose()?
                .unwrap_or_default();
            if !base.contains_key("review_marker") {
                base.insert("review_marker".into(), i18n::t(lang, "review_marker").into());
            }
            let scale = match lang {
                Lang::Ja => 1,
                Lang::En => 2,
            };
            for (key, ja_default) in [("conclusion_max_chars", 200), ("question_max_chars", 40), ("reply_max_chars", 400)] {
                if !base.contains_key(key) {
                    base.insert(key.into(), (ja_default * scale).into());
                }
            }
        }
        let mut cfg: Config = serde_json::from_value(merged).map_err(|e| format!("config: {e}"))?;
        let lang = cfg.language;
        // A config file must not be able to make blocking unbounded or widen what counts as a report.
        cfg.max_rounds = cfg.max_rounds.min(MAX_ROUNDS_LIMIT);
        if cfg.report_dir.is_empty() || cfg.report_dir.contains('/') || cfg.report_dir.starts_with('.') && cfg.report_dir.len() == 1 {
            return Err(i18n::t(lang, "config.report_dir").to_string());
        }
        if cfg.report_suffix.len() <= ".json".len() || !cfg.report_suffix.ends_with(".json") {
            return Err(i18n::t(lang, "config.report_suffix").to_string());
        }
        if cfg.role_dir_marker.is_empty() || cfg.review_marker.is_empty() || cfg.review_command.is_empty() {
            return Err(i18n::t(lang, "config.empty").to_string());
        }
        i18n::set_current(lang);
        Ok(cfg)
    }
}
