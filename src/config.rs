use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

use crate::util::{home, read_regular};

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
    pub report_dir: String,
    pub report_suffix: String,
    pub conclusion_max_chars: usize,
    pub reply_max_chars: usize,
    pub code_extensions: Vec<String>,
    pub ignore_path_patterns: Vec<String>,
    pub edit_tools: Vec<String>,
    pub tool_call_threshold: usize,
    pub trigger_commands: Vec<String>,
    pub role_dir_marker: String,
    pub review_command: String,
    pub review_marker: String,
    pub verification_rules: Vec<VerificationRule>,
    pub max_rounds: u32,
    pub agent_prompt_markers: Vec<String>,
}

impl Config {
    /// Looks up $REPORT_GATE_CONFIG, then ~/.config/agent-report-gate/config.json.
    /// Top-level keys of the user file override the embedded defaults.
    pub fn load() -> Result<Config, String> {
        let mut merged: Value =
            serde_json::from_str(DEFAULT_CONFIG).map_err(|e| format!("default config: {e}"))?;
        let user_path = match std::env::var("REPORT_GATE_CONFIG") {
            Ok(p) if !p.is_empty() => Some(PathBuf::from(p)),
            _ => {
                let p = home().join(".config/agent-report-gate/config.json");
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
        let mut cfg: Config = serde_json::from_value(merged).map_err(|e| format!("config: {e}"))?;
        // A config file must not be able to make blocking unbounded or widen what counts as a report.
        cfg.max_rounds = cfg.max_rounds.min(MAX_ROUNDS_LIMIT);
        if cfg.report_dir.is_empty() || cfg.report_dir.contains('/') || cfg.report_dir.starts_with('.') && cfg.report_dir.len() == 1 {
            return Err("config: report_dir はフォルダ名 1 つにする".to_string());
        }
        if cfg.report_suffix.len() <= ".json".len() || !cfg.report_suffix.ends_with(".json") {
            return Err("config: report_suffix は .json で終わる、.json より長い文字列にする".to_string());
        }
        if cfg.role_dir_marker.is_empty() || cfg.review_marker.is_empty() || cfg.review_command.is_empty() {
            return Err("config: role_dir_marker、review_marker、review_command は空にできない".to_string());
        }
        Ok(cfg)
    }
}
