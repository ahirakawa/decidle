use serde_json::Value;
use std::io::{BufRead, BufReader, Read};

use crate::util::open_regular;
use std::path::Path;

#[derive(PartialEq, Clone, Copy)]
pub enum Kind {
    User,
    Assistant,
}

pub struct ToolCall {
    pub name: String,
    pub file_path: Option<String>,
    pub command: Option<String>,
}

pub struct Entry {
    pub kind: Kind,
    pub uuid: String,
    pub session_id: String,
    pub timestamp: String,
    pub prompt_id: Option<String>,
    pub cwd: String,
    pub is_meta: bool,
    pub has_tool_result: bool,
    pub text: String,
    pub tools: Vec<ToolCall>,
}

/// Prefixes of user entries that Claude Code writes itself (slash commands, skill bodies, notices).
const WRAPPER_PREFIXES: [&str; 6] = [
    "<command-",
    "<local-command",
    "Base directory for this skill",
    "<task-notification",
    "[Request interrupted",
    "<system-reminder",
];

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Returns None for sidechain entries and for line types other than user and assistant.
pub fn parse_line(v: &Value) -> Option<Entry> {
    let kind = match v.get("type").and_then(Value::as_str) {
        Some("user") => Kind::User,
        Some("assistant") => Kind::Assistant,
        _ => return None,
    };
    if v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let mut text_parts: Vec<String> = Vec::new();
    let mut tools = Vec::new();
    let mut has_tool_result = false;
    match v.get("message").and_then(|m| m.get("content")) {
        Some(Value::String(s)) => text_parts.push(s.clone()),
        Some(Value::Array(parts)) => {
            for p in parts {
                match p.get("type").and_then(Value::as_str) {
                    Some("text") => text_parts.push(str_field(p, "text")),
                    Some("tool_result") => has_tool_result = true,
                    Some("tool_use") => {
                        let input = p.get("input");
                        let get = |k: &str| {
                            input
                                .and_then(|i| i.get(k))
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        };
                        tools.push(ToolCall {
                            name: str_field(p, "name"),
                            file_path: get("file_path").or_else(|| get("notebook_path")),
                            command: get("command"),
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Some(Entry {
        kind,
        uuid: str_field(v, "uuid"),
        session_id: str_field(v, "sessionId"),
        timestamp: str_field(v, "timestamp"),
        prompt_id: v.get("promptId").and_then(Value::as_str).map(str::to_string),
        cwd: str_field(v, "cwd"),
        is_meta: v.get("isMeta").and_then(Value::as_bool).unwrap_or(false),
        has_tool_result,
        text: text_parts.join("\n").trim().to_string(),
        tools,
    })
}

/// Largest transcript the hook will read. Bigger sessions are skipped instead of slowing every stop.
const MAX_TRANSCRIPT_BYTES: u64 = 256 * 1024 * 1024;

pub fn load(path: &Path) -> std::io::Result<Vec<Entry>> {
    let file = open_regular(path)?;
    if file.metadata()?.len() > MAX_TRANSCRIPT_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "大きすぎる"));
    }
    // The limit also holds if the file grows while it is being read.
    let reader = BufReader::new(file.take(MAX_TRANSCRIPT_BYTES));
    let mut entries = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(e) = parse_line(&v) {
            entries.push(e);
        }
    }
    Ok(entries)
}

/// A prompt typed or sent into the session, as opposed to tool results and harness-written entries.
pub fn is_real_prompt(e: &Entry) -> bool {
    e.kind == Kind::User
        && !e.is_meta
        && !e.has_tool_result
        && !e.text.is_empty()
        && !WRAPPER_PREFIXES.iter().any(|p| e.text.starts_with(p))
}

/// Index of the entry that starts the current turn.
/// Stop hook feedback is recorded as a meta user entry with the same prompt id, so it never starts a turn.
pub fn turn_start(entries: &[Entry], prompt_id: Option<&str>) -> Option<usize> {
    if let Some(pid) = prompt_id {
        let found = entries.iter().position(|e| {
            e.kind == Kind::User
                && !e.is_meta
                && !e.has_tool_result
                && e.prompt_id.as_deref() == Some(pid)
        });
        if found.is_some() {
            return found;
        }
    }
    entries.iter().rposition(is_real_prompt)
}
