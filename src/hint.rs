//! `decidle hint`: the PostToolUse hook that tells the agent a report is needed, once per turn,
//! as soon as the turn crosses the Stop hook's conditions.
//! The Stop hook is unchanged and still blocks a stop without a report.

use serde::Deserialize;
use serde_json::json;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::check::{collect_facts, should_check};
use crate::config::Config;
use crate::gate::{report_format, turn_key, when_to_report};
use crate::transcript;
use crate::util::{append_line, data_dir, now_iso, read_regular, write_atomic};
use decidle::i18n::{f, t};

const MAX_INPUT_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize, Default)]
struct HookInput {
    session_id: Option<String>,
    transcript_path: Option<String>,
    cwd: Option<String>,
    prompt_id: Option<String>,
}

/// Same file name rule as the Stop hook's state, with another extension.
fn hint_path(data_dir: &Path, session_id: &str) -> PathBuf {
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let name = if safe.is_empty() { "no-session" } else { safe.as_str() };
    data_dir.join("state").join(format!("{name}.hint"))
}

/// Never fails: anything unreadable prints nothing and leaves the stop to the Stop hook.
pub fn run() {
    if std::env::var("DECIDLE_MODE").is_ok_and(|m| m == "off") {
        return;
    }
    let mut raw = String::new();
    let _ = std::io::stdin().take(MAX_INPUT_BYTES).read_to_string(&mut raw);
    match decide(&raw) {
        Ok(Some(output)) => println!("{output}"),
        Ok(None) => {}
        Err(e) => eprintln!("decidle hint: {e}"),
    }
}

fn decide(raw: &str) -> Result<Option<String>, String> {
    let cfg = Config::load()?;
    let lang = cfg.language;
    let input: HookInput = serde_json::from_str(raw).map_err(|e| format!("hook input: {e}"))?;
    let transcript_path = input.transcript_path.as_deref().ok_or(t(lang, "gate.no_transcript_path"))?;
    let entries = transcript::load(Path::new(transcript_path))
        .map_err(|e| f(lang, "gate.transcript_unreadable", &[("path", transcript_path), ("e", &e.to_string())]))?;
    let turn = input.prompt_id.as_deref();
    let start = transcript::turn_start(&entries, turn).ok_or(t(lang, "gate.no_prompt"))?;
    let cwd = input.cwd.clone().unwrap_or_default();
    let facts = collect_facts(&entries, start, &cwd, &cfg);
    // A report already written needs no hint; should_check is true for it alone.
    if facts.report_file.is_some() || !should_check(&facts, &cfg) {
        return Ok(None);
    }

    let key = turn_key(turn, &entries, start);
    let session_id = input.session_id.clone().unwrap_or_default();
    let data_dir = data_dir();
    let path = hint_path(&data_dir, &session_id);
    // Only a missing file means "not hinted yet". Any other failure must not hint again and again.
    let last = match read_regular(&path, MAX_INPUT_BYTES) {
        Ok(text) => text.trim().to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    if last == key {
        return Ok(None);
    }
    write_atomic(&path, key.as_bytes(), true).map_err(|e| e.to_string())?;

    let line = json!({ "ts": now_iso(), "session_id": session_id, "verdict": "hint", "issues": [], "rounds": 0, "error": null });
    let _ = append_line(&data_dir.join("gate.jsonl"), &line.to_string());

    let context = f(lang, "hint.needed", &[("when", &when_to_report(&cfg)), ("format", report_format(lang))]);
    let output = json!({ "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": context } });
    Ok(Some(output.to_string()))
}
