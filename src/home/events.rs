//! Appending to events.jsonl. Every append, and the kill check, happens while holding an
//! exclusive flock on the log file itself, so that the server's check and its kill line cannot
//! interleave with a hook (see the counterexample of Home_unlocked.cfg).

use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::note::{self, Attention};
use super::agent;
use super::procinfo;

/// How long a hook waits for the lock before giving up. Losing one line is better than
/// holding up the agent.
pub const HOOK_LOCK_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_HOOK_INPUT_BYTES: u64 = 1024 * 1024;
/// How much of the end of the log is read to find the last line's ts.
const TAIL_BYTES: u64 = 64 * 1024;

pub fn events_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/decidle/events.jsonl")
}

/// The current time as RFC 3339 UTC with microseconds. The microseconds keep the ts of two
/// stops of one session apart, because `resolves` refers to a stop by its ts.
pub fn now_rfc3339() -> String {
    rfc3339_at(SystemTime::now())
}

pub fn rfc3339_at(t: SystemTime) -> String {
    rfc3339_from_us(t.duration_since(UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0))
}

/// Microseconds since the epoch to `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
pub fn rfc3339_from_us(us: i64) -> String {
    let secs = us.div_euclid(1_000_000);
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:06}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        us.rem_euclid(1_000_000)
    )
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction]Z` (UTC only) into microseconds since the epoch.
/// Used for our own ts and for the timestamps in Claude Code transcripts.
pub fn parse_rfc3339_us(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || *b.last()? != b'Z' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let frac = &text[19..text.len() - 1];
    let micros = match frac.strip_prefix('.') {
        None if frac.is_empty() => 0,
        Some(digits) if !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) => {
            let six: String = digits.chars().chain(std::iter::repeat('0')).take(6).collect();
            six.parse::<i64>().ok()?
        }
        _ => return None,
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    // Days-from-civil (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86400 + hh * 3600 + mm * 60 + ss) * 1_000_000) + micros)
}

/// Creates the folder if needed and makes it readable by the owner only.
pub fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{}: フォルダではない", path.display())));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// The log file opened for appending, with the exclusive lock held until this is dropped.
pub struct LogLock {
    file: File,
}

impl LogLock {
    pub fn acquire(path: &Path, timeout: Duration) -> io::Result<LogLock> {
        if let Some(parent) = path.parent() {
            private_dir(parent)?;
        }
        // O_NONBLOCK so that a FIFO planted at the path cannot hang the hook.
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.nlink() != 1 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "log is not a private regular file"));
        }
        let deadline = Instant::now() + timeout;
        loop {
            // SAFETY: flock on a file descriptor we own.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(LogLock { file });
            }
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EWOULDBLOCK) && err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "events.jsonl のロックを取れない"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Looks at the end of the log: whether it ends with a newline, and the ts of the last
    /// complete line, if it is one of ours.
    fn tail(&self) -> io::Result<(bool, Option<i64>)> {
        use std::os::unix::fs::FileExt;
        let len = self.file.metadata()?.len();
        if len == 0 {
            return Ok((true, None));
        }
        let start = len.saturating_sub(TAIL_BYTES);
        let mut buf = vec![0u8; (len - start) as usize];
        self.file.read_exact_at(&mut buf, start)?;
        let ends_with_newline = buf.last() == Some(&b'\n');
        let last_ts = buf
            .split(|c| *c == b'\n')
            .rev()
            .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
            .find_map(|v| v.get("ts").and_then(Value::as_str).and_then(parse_rfc3339_us));
        Ok((ends_with_newline, last_ts))
    }

    /// Writes one line with a single write. `ts` is set here, inside the lock, and is made
    /// later than the ts of the line before, so that ts are unique and follow the line order.
    /// A line cut off by a crash is ended first, so that it does not swallow this one.
    /// Returns the ts.
    pub fn append(&mut self, mut fields: Map<String, Value>) -> io::Result<String> {
        let (ends_with_newline, last_us) = self.tail()?;
        let now_us = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0);
        let ts = rfc3339_from_us(last_us.map_or(now_us, |last| now_us.max(last + 1)));
        let mut line = Map::new();
        line.insert("ts".to_string(), Value::String(ts.clone()));
        line.append(&mut fields);
        let prefix = if ends_with_newline { "" } else { "\n" };
        self.file.write_all(format!("{prefix}{}\n", Value::Object(line)).as_bytes())?;
        Ok(ts)
    }
}

/// Takes the lock, appends one line and releases the lock.
pub fn append(path: &Path, fields: Map<String, Value>, timeout: Duration) -> io::Result<String> {
    LogLock::acquire(path, timeout)?.append(fields)
}

fn base(session_id: &str, cwd: &str, event: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("session_id".into(), json!(session_id));
    m.insert("cwd".into(), json!(cwd));
    m.insert("event".into(), json!(event));
    m
}

/// The report that came with a stop.
pub struct StopReport {
    pub path: String,
    /// The page that decidle stop actually wrote. The Home page serves only these.
    pub html: String,
    pub title: String,
    pub decisions: usize,
    /// The `question` of each decision in the report, as the agent wrote it (at most
    /// `question_max_chars` characters each). Never prompt or reply text.
    pub questions: Vec<String>,
    /// The recommended option of each decision, in the order of `questions`; empty where the
    /// report's `recommend` names no option.
    pub recommends: Vec<String>,
    /// The report's `goal`, as the agent wrote it.
    pub goal: String,
}

/// What `decidle stop` records about the stop besides the report.
#[derive(Default)]
pub struct StopInfo {
    /// The agent still waits on background work (see `background.rs`).
    pub background: bool,
    /// From the agent's `Home:` line (see `note.rs`).
    pub attention: Attention,
    pub note: Option<String>,
}

/// Appends the stop line. `decidle stop` calls this when it lets the agent stop.
pub fn append_stop(
    path: &Path,
    session_id: &str,
    cwd: &str,
    report: Option<&StopReport>,
    info: &StopInfo,
) -> io::Result<String> {
    let mut m = base(session_id, cwd, "stop");
    if info.background {
        m.insert("background".into(), json!(true));
    }
    m.insert("attention".into(), json!(info.attention.as_str()));
    if let Some(n) = &info.note {
        m.insert("note".into(), json!(n));
    }
    if let Some(r) = report {
        m.insert("report_path".into(), json!(r.path));
        m.insert("report_html".into(), json!(r.html));
        m.insert("report_title".into(), json!(r.title));
        m.insert("decisions".into(), json!(r.decisions));
        m.insert("questions".into(), json!(r.questions));
        m.insert("recommends".into(), json!(r.recommends));
        if !r.goal.is_empty() {
            m.insert("report_goal".into(), json!(r.goal));
        }
    }
    append(path, m, HOOK_LOCK_TIMEOUT)
}

/// Appends a resolved or kill line that refers to the stop the human saw.
pub fn resolution(session_id: &str, cwd: &str, event: &str, resolves: &str) -> Map<String, Value> {
    let mut m = base(session_id, cwd, event);
    m.insert("resolves".into(), json!(resolves));
    m
}

/// The choose line: the Home page pressed option `option` (from 0) of the question on the ask
/// line `resolves`.
pub fn choice(session_id: &str, cwd: &str, resolves: &str, option: usize) -> Map<String, Value> {
    let mut m = resolution(session_id, cwd, "choose", resolves);
    m.insert("option".into(), json!(option));
    m
}

/// Only the fields we keep. The prompt and the message are never read into this struct.
#[derive(Deserialize, Default)]
struct HookInput {
    session_id: Option<String>,
    cwd: Option<String>,
    hook_event_name: Option<String>,
    transcript_path: Option<String>,
    /// SessionStart: startup, resume, clear or compact.
    source: Option<String>,
    notification_type: Option<String>,
    message: Option<String>,
    /// Codex only; see agent.rs.
    turn_id: Option<String>,
    /// PreToolUse and PostToolUse.
    tool_name: Option<String>,
    /// Read only for AskUserQuestion on PreToolUse, and then only the questions. The answers
    /// (PostToolUse) are never copied.
    tool_input: Option<Value>,
}

const ASK_TOOL: &str = "AskUserQuestion";
const ASK_MAX_QUESTIONS: usize = 4;
const ASK_MAX_OPTIONS: usize = 8;
const ASK_QUESTION_MAX_CHARS: usize = 200;
const ASK_HEADER_MAX_CHARS: usize = 40;
const ASK_OPTION_MAX_CHARS: usize = 80;

/// The same as `clip` in src/util.rs, which the library cannot reach: line breaks and other
/// control characters become spaces, bidi overrides are dropped, and the text is cut.
fn clip(text: &str, max_chars: usize) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !matches!(*c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() <= max_chars {
        clean
    } else {
        format!("{}…", clean.chars().take(max_chars).collect::<String>())
    }
}

/// The `ask` array of an ask line from the tool input of AskUserQuestion: the first four
/// questions, each with its first eight option labels. Descriptions are not copied.
fn ask_questions(tool_input: Option<&Value>) -> Value {
    let questions = tool_input.and_then(|t| t.get("questions")).and_then(Value::as_array);
    let text = |v: &Value, key: &str, max: usize| clip(v.get(key).and_then(Value::as_str).unwrap_or_default(), max);
    let list: Vec<Value> = questions
        .into_iter()
        .flatten()
        .filter(|q| q.is_object())
        .take(ASK_MAX_QUESTIONS)
        .map(|q| {
            let options: Vec<String> = q
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(ASK_MAX_OPTIONS)
                .map(|o| text(o, "label", ASK_OPTION_MAX_CHARS))
                .collect();
            json!({
                "question": text(q, "question", ASK_QUESTION_MAX_CHARS),
                "header": text(q, "header", ASK_HEADER_MAX_CHARS),
                "options": options,
                "multi": q.get("multiSelect").and_then(Value::as_bool) == Some(true),
            })
        })
        .collect();
    Value::Array(list)
}

fn is_permission(input: &HookInput) -> bool {
    match input.notification_type.as_deref() {
        Some(t) => t == "permission_prompt",
        None => input.message.as_deref().is_some_and(|m| m.contains("permission")),
    }
}

/// Turns a hook input into the fields of one line, or None when the hook is not recorded.
fn line_for(input: &HookInput) -> Option<Map<String, Value>> {
    let session_id = input.session_id.as_deref().filter(|s| !s.is_empty())?;
    let cwd = input.cwd.as_deref().unwrap_or_default();
    let event = match input.hook_event_name.as_deref()? {
        // A compaction does not start a new claude; only the context is given again.
        "SessionStart" if input.source.as_deref() == Some("compact") => return None,
        "SessionStart" => "start",
        "UserPromptSubmit" => "prompt",
        "Notification" if is_permission(input) => "permission",
        // Codex asks for approval with this hook (Claude Code has it too).
        "PermissionRequest" => "permission",
        // Codex: the turn was interrupted, also when the human denied an approval. No Stop follows.
        "Interrupt" => "interrupt",
        "SessionEnd" => "end",
        "PreToolUse" if input.tool_name.as_deref() == Some(ASK_TOOL) => "ask",
        "PostToolUse" if input.tool_name.as_deref() == Some(ASK_TOOL) => "asked",
        _ => return None,
    };
    let mut m = base(session_id, cwd, event);
    if event == "ask" {
        m.insert("ask".into(), ask_questions(input.tool_input.as_ref()));
    }
    if event == "start" {
        let agent = agent::detect(&json!({"transcript_path": input.transcript_path, "turn_id": input.turn_id}));
        m.insert("agent".into(), json!(agent.as_str()));
        let found = procinfo::find_ancestor(agent.process_name());
        m.insert("pid".into(), json!(found.map(|(pid, _)| pid)));
        m.insert("pid_started".into(), json!(found.map(|(_, started)| started)));
        m.insert("transcript_path".into(), json!(input.transcript_path));
        if let Some(t) = super::term::from_env(|k| std::env::var(k).ok()) {
            m.insert("term".into(), json!(t.kind));
            m.insert("term_id".into(), json!(t.id));
            if let Some(ws) = t.workspace {
                m.insert("term_ws".into(), json!(ws));
            }
        }
    }
    Some(m)
}

/// The hook's stdout for SessionStart: the instruction for the `Home:` line, as additional context.
pub fn session_start_output() -> String {
    json!({
        "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": note::INSTRUCTION }
    })
    .to_string()
}

/// Appends the line for the hook, if any. Returns whether the hook was SessionStart.
fn record(path: &Path, stdin: &mut dyn Read) -> (bool, Result<(), String>) {
    let mut text = String::new();
    if let Err(e) = stdin.take(MAX_HOOK_INPUT_BYTES).read_to_string(&mut text) {
        return (false, Err(e.to_string()));
    }
    let input: HookInput = match serde_json::from_str(&text) {
        Ok(i) => i,
        Err(e) => return (false, Err(e.to_string())),
    };
    let start = input.hook_event_name.as_deref() == Some("SessionStart");
    let result = match line_for(&input) {
        Some(fields) => append(path, fields, HOOK_LOCK_TIMEOUT).map(|_| ()).map_err(|e| e.to_string()),
        None => Ok(()),
    };
    (start, result)
}

/// `decidle event`. Never fails: the exit code is always 0. Only SessionStart prints, and it
/// prints the additional context even when the append fails. Other hooks print nothing, because
/// the UserPromptSubmit hook would add stdout to the agent's context.
pub fn run_hook() -> i32 {
    let path = events_path();
    let result = std::panic::catch_unwind(|| record(&path, &mut io::stdin()));
    let start = match result {
        Ok((start, Err(e))) => {
            eprintln!("decidle event: {e}");
            start
        }
        Ok((start, Ok(()))) => start,
        Err(_) => {
            eprintln!("decidle event: panic");
            false
        }
    };
    if start {
        println!("{}", session_start_output());
    }
    0
}
