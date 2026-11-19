//! Which agent runs the hook: Claude Code or Codex CLI.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// The process name the kill button looks for.
    pub fn process_name(self) -> &'static str {
        self.as_str()
    }
}

/// Codex writes its transcripts as `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`
/// and passes that path in every hook, SessionStart and SessionEnd included. Claude Code's are
/// `~/.claude/projects/<project>/<id>.jsonl`. A hook input without a transcript path that has a
/// `turn_id` is also Codex: Claude Code has no turn_id.
pub fn detect(input: &Value) -> Agent {
    let path = input.get("transcript_path").and_then(Value::as_str).unwrap_or_default();
    if is_codex_transcript(path) {
        return Agent::Codex;
    }
    if path.is_empty() && input.get("turn_id").and_then(Value::as_str).is_some() {
        return Agent::Codex;
    }
    Agent::Claude
}

pub fn is_codex_transcript(path: &str) -> bool {
    let p = std::path::Path::new(path);
    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    name.starts_with("rollout-") && name.ends_with(".jsonl") && path.contains("/sessions/")
}
