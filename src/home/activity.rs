//! Whether claude has worked since a stop, judged from its transcript instead of events.jsonl.
//! The kill button uses this so that a lost prompt line cannot make a working session look
//! stopped. The file's modification time cannot be used: Claude Code appends bookkeeping
//! entries (stop_hook_summary, turn_duration, attachments) right after the Stop hook returns.

use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::events::parse_rfc3339_us;

/// How much of the end of the transcript is read. The newest entries are at the end.
const TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Ok(true) when the transcript has a user or assistant entry stamped after `stop_us`.
/// An error means the question cannot be answered, and the caller must not signal.
pub fn worked_since(path: &Path, stop_us: i64) -> Result<bool, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(format!("{}: 通常のファイルではない", path.display()));
    }
    let start = meta.len().saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    file.take(TAIL_BYTES).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let mut newest: Option<i64> = None;
    for line in buf.split(|c| *c == b'\n') {
        let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
        if !matches!(v.get("type").and_then(Value::as_str), Some("user" | "assistant")) {
            continue;
        }
        if let Some(t) = v.get("timestamp").and_then(Value::as_str).and_then(parse_rfc3339_us) {
            newest = Some(newest.map_or(t, |n| n.max(t)));
        }
    }
    match newest {
        Some(t) => Ok(t > stop_us),
        None => Err(format!("{}: 会話の行が見つからない", path.display())),
    }
}
