//! Whether a stopping session still waits on work running in the background: a subagent or a
//! command started in the background whose result has not come back, or a ScheduleWakeup that
//! has not fired. `decidle stop` records this on the stop line, and the Home page shows such
//! a session as working in the background, without the resolve and kill buttons.
//!
//! The rules follow the lines Claude Code writes to the transcript (seen in version 2.1.32x).
//! If Claude Code changes how it writes them, the detection misses.
//!
//! - Started in the background: a `user` line whose `toolUseResult` has `status` =
//!   `"async_launched"` (Agent) or a string `backgroundTaskId` (Bash with run_in_background).
//!   The key is the `tool_use_id` of its `tool_result`.
//! - Result came back: a `<task-notification>` text with `<tool-use-id>KEY</tool-use-id>`, in the
//!   `content` of a `queue-operation` line, the `prompt` of an `attachment` line, or the message
//!   of a `user` line.
//! - Stopped: the `tool_result` of a `TaskStop` call whose `toolUseResult.task_id` equals the
//!   `backgroundTaskId` (or the `agentId`) of the start. No task-notification follows a TaskStop.
//! - Wakeup scheduled: the `tool_result` of a `ScheduleWakeup` call whose `toolUseResult` has
//!   `scheduledFor`. It ends at a `system` line with `subtype` = `"scheduled_task_fire"`, or at a
//!   `ScheduleWakeup` call with `stop` = true.
//!
//! The whole transcript is read, not only the last turn: a subagent started in an earlier turn
//! can still be running.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::path::Path;

/// Largest transcript read, the same bound as the report check.
const MAX_TRANSCRIPT_BYTES: u64 = 256 * 1024 * 1024;

/// Substrings that a line must contain to matter. Other lines are not parsed.
const MARKERS: [&str; 8] = [
    "async_launched",
    "backgroundTaskId",
    "task-notification",
    "ScheduleWakeup",
    "scheduledFor",
    "scheduled_task_fire",
    "TaskStop",
    "task_id",
];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pending {
    /// Background starts whose result has not come back.
    pub tasks: usize,
    /// A ScheduleWakeup that has not fired.
    pub wakeup: bool,
}

impl Pending {
    pub fn waiting(&self) -> bool {
        self.tasks > 0 || self.wakeup
    }
}

fn tool_use_ids_in_notification(text: &str, out: &mut Vec<String>) {
    if !text.trim_start().starts_with("<task-notification>") {
        return;
    }
    let mut rest = text;
    while let Some(i) = rest.find("<tool-use-id>") {
        rest = &rest[i + "<tool-use-id>".len()..];
        let Some(j) = rest.find("</tool-use-id>") else { break };
        out.push(rest[..j].trim().to_string());
        rest = &rest[j..];
    }
}

/// The texts of a user message: the string, or the text parts.
fn message_texts(v: &Value) -> Vec<&str> {
    match v.get("message").and_then(|m| m.get("content")) {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect(),
        _ => Vec::new(),
    }
}

fn tool_results(v: &Value) -> Vec<&str> {
    match v.get("message").and_then(|m| m.get("content")) {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|p| p.get("type").and_then(Value::as_str) == Some("tool_result"))
            .filter_map(|p| p.get("tool_use_id").and_then(Value::as_str))
            .collect(),
        _ => Vec::new(),
    }
}

/// Applies the rules to the transcript lines in order.
pub fn scan<'a>(lines: impl IntoIterator<Item = &'a str>) -> Pending {
    // tool_use_id of the start -> its task id (backgroundTaskId or agentId).
    let mut running: HashMap<String, String> = HashMap::new();
    let mut wakeup_calls: HashSet<String> = HashSet::new();
    let mut stop_calls: HashSet<String> = HashSet::new();
    let mut wakeup = false;
    for line in lines {
        if !MARKERS.iter().any(|m| line.contains(m)) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let mut done = Vec::new();
        match v.get("type").and_then(Value::as_str) {
            Some("assistant") => {
                let parts = v.get("message").and_then(|m| m.get("content")).and_then(Value::as_array);
                for p in parts.into_iter().flatten() {
                    if p.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    let name = p.get("name").and_then(Value::as_str);
                    if name == Some("TaskStop") {
                        if let Some(id) = p.get("id").and_then(Value::as_str) {
                            stop_calls.insert(id.to_string());
                        }
                        continue;
                    }
                    if name != Some("ScheduleWakeup") {
                        continue;
                    }
                    if p.get("input").and_then(|i| i.get("stop")).and_then(Value::as_bool) == Some(true) {
                        wakeup = false;
                    } else if let Some(id) = p.get("id").and_then(Value::as_str) {
                        wakeup_calls.insert(id.to_string());
                    }
                }
            }
            Some("user") => {
                let result = v.get("toolUseResult");
                let text_of = |k: &str| result.and_then(|r| r.get(k)).and_then(Value::as_str);
                let task = text_of("backgroundTaskId")
                    .or_else(|| (text_of("status") == Some("async_launched")).then(|| text_of("agentId").unwrap_or_default()));
                let stopped_task = text_of("task_id");
                let scheduled = result.is_some_and(|r| r.get("scheduledFor").is_some_and(Value::is_number));
                for id in tool_results(&v) {
                    if let Some(task) = task {
                        running.insert(id.to_string(), task.to_string());
                    }
                    if let (Some(t), true) = (stopped_task, stop_calls.contains(id)) {
                        running.retain(|_, task| task.as_str() != t);
                    }
                    if scheduled && wakeup_calls.contains(id) {
                        wakeup = true;
                    }
                }
                for text in message_texts(&v) {
                    tool_use_ids_in_notification(text, &mut done);
                }
            }
            Some("queue-operation") => {
                if let Some(text) = v.get("content").and_then(Value::as_str) {
                    tool_use_ids_in_notification(text, &mut done);
                }
            }
            Some("attachment") => {
                if let Some(text) = v.get("attachment").and_then(|a| a.get("prompt")).and_then(Value::as_str) {
                    tool_use_ids_in_notification(text, &mut done);
                }
            }
            Some("system") if v.get("subtype").and_then(Value::as_str) == Some("scheduled_task_fire") => wakeup = false,
            _ => {}
        }
        for id in done {
            running.remove(&id);
        }
    }
    Pending { tasks: running.len(), wakeup }
}

/// Reads the transcript and applies the rules. Lines that are not UTF-8 are skipped.
pub fn pending(path: &Path) -> io::Result<Pending> {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NONBLOCK so that a FIFO at the path cannot hang the Stop hook.
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "通常のファイルではない"));
    }
    let mut buf = Vec::new();
    file.take(MAX_TRANSCRIPT_BYTES).read_to_end(&mut buf)?;
    Ok(scan(buf.split(|c| *c == b'\n').filter_map(|l| std::str::from_utf8(l).ok())))
}
