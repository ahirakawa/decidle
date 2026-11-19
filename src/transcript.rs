use serde_json::Value;
use std::io::{BufRead, BufReader, Read};

use crate::util::open_regular;
use decidle::i18n;
use std::path::Path;

#[derive(PartialEq, Clone, Copy)]
pub enum Kind {
    User,
    Assistant,
}

pub struct ToolCall {
    pub name: String,
    /// Files the call writes. One for Edit and Write; every file of the patch for Codex's apply_patch.
    pub file_paths: Vec<String>,
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
                            file_paths: get("file_path").or_else(|| get("notebook_path")).into_iter().collect(),
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
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, i18n::t(i18n::current(), "file.too_large")));
    }
    // The limit also holds if the file grows while it is being read.
    let reader = BufReader::new(file.take(MAX_TRANSCRIPT_BYTES));
    let mut entries = Vec::new();
    let mut codex = codex::State::default();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let entry = if codex::is_codex_line(&v) { codex.parse_line(&v) } else { parse_line(&v) };
        if let Some(e) = entry {
            entries.push(e);
        }
    }
    Ok(entries)
}

/// Codex CLI transcripts (`~/.codex/sessions/.../rollout-*.jsonl`), read into the same entries as
/// Claude Code's. Every line is `{timestamp, ordinal, type, payload}`. The shapes follow what
/// Codex CLI 0.169 wrote in a trial.
///
/// - `session_meta`, `turn_context` and `event_msg` (`task_started`) give the session id, the cwd
///   and the current turn id. Each entry gets the current turn id as its prompt id, so that the
///   hook's `turn_id` finds the turn the way Claude Code's `prompt_id` does.
/// - `response_item` with `payload.type` = `message`: role `user` and `assistant` become entries;
///   role `developer` (instructions and additional context) is left out.
/// - `function_call` (`name`, `arguments` as a JSON string) and `custom_tool_call` (`name`, `input`
///   as a string) become tool calls of an assistant entry. The shell is `custom_tool_call` named
///   `exec` (the hook calls it Bash), so it is named Bash here and its input is the command. A
///   call named `apply_patch` writes every file named in its patch.
pub mod codex {
    use super::{str_field, Entry, Kind, ToolCall};
    use serde_json::Value;

    /// Whether the text starts like the reason `decidle stop` gives when it blocks, in any
    /// language. Codex records the reason as a user message, so such a message is marked as meta,
    /// like Claude Code's "Stop hook feedback".
    pub fn is_block(text: &str) -> bool {
        decidle::i18n::all("gate.block_prefix").any(|p| text.starts_with(p))
    }

    const TYPES: [&str; 8] = [
        "session_meta",
        "turn_context",
        "response_item",
        "event_msg",
        "token_usage_record",
        "world_state",
        "inter_agent_communication_metadata",
        "compacted",
    ];

    pub fn is_codex_line(v: &Value) -> bool {
        v.get("payload").is_some_and(Value::is_object)
            && v.get("type").and_then(Value::as_str).is_some_and(|t| TYPES.contains(&t))
    }

    #[derive(Default)]
    pub struct State {
        session_id: String,
        cwd: String,
        turn_id: Option<String>,
    }

    fn texts(content: Option<&Value>) -> String {
        match content {
            Some(Value::String(s)) => s.trim().to_string(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string(),
            _ => String::new(),
        }
    }

    /// The command of a shell call: a string, or an argv whose script is the last item when it
    /// starts with a shell (`["bash", "-lc", "..."]`).
    fn command_of(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Array(items) => {
                let argv: Vec<&str> = items.iter().filter_map(Value::as_str).collect();
                let shell = argv.first().is_some_and(|a| {
                    matches!(a.rsplit('/').next(), Some("bash" | "sh" | "zsh"))
                });
                if shell && argv.len() >= 3 && argv[argv.len() - 2].starts_with('-') {
                    argv.last().map(|s| s.to_string())
                } else {
                    Some(argv.join(" "))
                }
            }
            _ => None,
        }
    }

    /// Files named in an apply_patch text: `*** Add File:`, `*** Update File:` and `*** Move to:`.
    /// Deleted files are left out; they no longer exist.
    pub fn patch_files(patch: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for line in patch.lines() {
            let line = line.trim_end();
            let path = ["*** Add File: ", "*** Update File: ", "*** Move to: "]
                .iter()
                .find_map(|p| line.strip_prefix(p));
            if let Some(p) = path.map(str::trim).filter(|p| !p.is_empty()) {
                if !out.iter().any(|o| o == p) {
                    out.push(p.to_string());
                }
            }
        }
        out
    }

    fn tool_call(payload: &Value) -> ToolCall {
        let name = str_field(payload, "name");
        // function_call carries JSON arguments; custom_tool_call carries free text.
        let raw = payload
            .get("arguments")
            .or_else(|| payload.get("input"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let json = serde_json::from_str::<Value>(raw).ok().filter(Value::is_object);
        let field = |k: &str| json.as_ref().and_then(|j| j.get(k));
        if name == "apply_patch" {
            let patch = field("input").or_else(|| field("patch")).and_then(Value::as_str).unwrap_or(raw);
            return ToolCall { name, file_paths: patch_files(patch), command: None };
        }
        let command = field("cmd").or_else(|| field("command")).and_then(command_of);
        if name == "exec" || name == "shell" || name == "exec_command" || name == "local_shell" {
            let command = command.or_else(|| (!raw.is_empty() && json.is_none()).then(|| raw.to_string()));
            return ToolCall { name: "Bash".into(), file_paths: Vec::new(), command };
        }
        ToolCall { name, file_paths: Vec::new(), command }
    }

    impl State {
        pub fn parse_line(&mut self, v: &Value) -> Option<Entry> {
            let payload = v.get("payload")?;
            let ptype = payload.get("type").and_then(Value::as_str);
            match v.get("type").and_then(Value::as_str)? {
                "session_meta" => {
                    // A subagent's transcript copies the parent's session_meta after its own; keep the first.
                    if self.session_id.is_empty() {
                        self.session_id = str_field(payload, "session_id");
                        if self.session_id.is_empty() {
                            self.session_id = str_field(payload, "id");
                        }
                    }
                    if self.cwd.is_empty() {
                        self.cwd = str_field(payload, "cwd");
                    }
                    None
                }
                "turn_context" => {
                    self.set_turn(payload);
                    let cwd = str_field(payload, "cwd");
                    if !cwd.is_empty() {
                        self.cwd = cwd;
                    }
                    None
                }
                "event_msg" if ptype == Some("task_started") => {
                    self.set_turn(payload);
                    None
                }
                "response_item" => self.item(v, payload, ptype?),
                _ => None,
            }
        }

        fn set_turn(&mut self, payload: &Value) {
            if let Some(t) = payload.get("turn_id").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                self.turn_id = Some(t.to_string());
            }
        }

        fn item(&self, line: &Value, payload: &Value, ptype: &str) -> Option<Entry> {
            let mut entry = Entry {
                kind: Kind::Assistant,
                uuid: line.get("ordinal").map(|o| o.to_string()).unwrap_or_default(),
                session_id: self.session_id.clone(),
                timestamp: str_field(line, "timestamp"),
                prompt_id: self.turn_id.clone(),
                cwd: self.cwd.clone(),
                is_meta: false,
                has_tool_result: false,
                text: String::new(),
                tools: Vec::new(),
            };
            match ptype {
                "message" => {
                    entry.text = texts(payload.get("content"));
                    match payload.get("role").and_then(Value::as_str) {
                        Some("assistant") => {}
                        Some("user") => {
                            entry.kind = Kind::User;
                            entry.is_meta = is_block(&entry.text);
                        }
                        _ => return None,
                    }
                }
                "function_call" | "custom_tool_call" | "local_shell_call" => entry.tools.push(tool_call(payload)),
                "function_call_output" | "custom_tool_call_output" => {
                    entry.kind = Kind::User;
                    entry.has_tool_result = true;
                }
                _ => return None,
            }
            Some(entry)
        }
    }
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
