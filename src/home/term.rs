//! The terminals the Home page can drive: bringing an agent's terminal to the front, and typing
//! the report's recommended options into it. See "端末へ行く、勧めで答える" in docs/spec-home.md.

use std::io;
use std::process::{Command, Stdio};

use super::state::Session;

/// What the SessionStart hook records about the terminal.
#[derive(Debug, PartialEq, Eq)]
pub struct Recorded {
    pub kind: &'static str,
    pub id: String,
    pub workspace: Option<String>,
}

/// An id is passed to a command as one argument; anything unusual is treated as absent.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | ':'))
}

fn is_uuid(id: &str) -> bool {
    id.len() == 36 && id.chars().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// Reads the terminal from the hook's environment. herdr wins over cmux, since herdr may run
/// inside a cmux terminal and then the pane is the one to address.
pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Option<Recorded> {
    let get = |k: &str| var(k).filter(|v| valid_id(v));
    if let Some(id) = get("HERDR_PANE_ID") {
        return Some(Recorded { kind: "herdr", id, workspace: None });
    }
    let id = get("CMUX_SURFACE_ID")?;
    Some(Recorded { kind: "cmux", id, workspace: get("CMUX_WORKSPACE_ID") })
}

/// What a start line may carry: a known kind with a valid id, or nothing at all.
pub fn checked(
    term: Option<String>,
    id: Option<String>,
    ws: Option<String>,
) -> (Option<String>, Option<String>, Option<String>) {
    match (term.as_deref(), id) {
        (Some("herdr"), Some(id)) if valid_id(&id) => (term, Some(id), None),
        // cmux also takes an index or a short ref where it takes an id, and those can come to
        // name another terminal. Only the UUIDs it puts in the environment are accepted, and
        // the workspace must come with the surface.
        (Some("cmux"), Some(id)) if is_uuid(&id) && ws.as_deref().is_some_and(is_uuid) => (term, Some(id), ws),
        _ => (None, None, None),
    }
}

/// The first word of every answer line.
pub const ANSWER_PREFIX: &str = "回答:";

/// Characters that a shell or the terminal's own escapes would act on. The text is typed into
/// the agent's prompt, but if the agent died in the last moment a shell reads it instead.
fn plain(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(|c| c.is_control() || matches!(c, '`' | '$' | ';' | '|' | '&' | '<' | '>' | '\\' | '!' | '\'' | '"'))
}

/// The one line the answer button types: built from the stop line only, never from the request.
/// None when a decision has no recommended option, or the text is not plain.
pub fn answer_text(s: &Session) -> Option<String> {
    if s.questions.is_empty() || s.questions.len() != s.recommends.len() {
        return None;
    }
    let parts: Vec<String> = s
        .questions
        .iter()
        .zip(&s.recommends)
        .map(|(q, r)| format!("{}は「{}」で進めて。", q.trim_end_matches(['?', '？', '。']), r))
        .collect();
    let ok = s.questions.iter().chain(&s.recommends).all(|t| plain(t));
    // The line starts with a fixed word. If the agent died in the last moment and a shell reads
    // the line, the shell looks for a command of this name and finds none; without it, the
    // first word of the agent's question would be run as a command.
    ok.then(|| format!("{ANSWER_PREFIX} {}", parts.join(" ")))
}

fn arg(flag: &str, value: &str) -> [String; 2] {
    [flag.to_string(), value.to_string()]
}

/// The commands that bring the terminal to the front.
pub fn focus_commands(s: &Session) -> Option<Vec<Vec<String>>> {
    let id = s.term_id.as_deref()?;
    match s.term.as_deref()? {
        "cmux" => {
            let mut out = Vec::new();
            let mut focus = vec!["cmux".to_string(), "focus-panel".to_string()];
            focus.extend(arg("--panel", id));
            if let Some(ws) = s.term_ws.as_deref() {
                let mut select = vec!["cmux".to_string(), "select-workspace".to_string()];
                select.extend(arg("--workspace", ws));
                out.push(select);
                focus.extend(arg("--workspace", ws));
            }
            out.push(focus);
            Some(out)
        }
        "herdr" => Some(vec![vec!["herdr".into(), "agent".into(), "focus".into(), id.into()]]),
        _ => None,
    }
}

/// The commands that type `text` and Enter into the terminal. cmux refuses by itself when the
/// agent's prompt holds a draft or a dialog is open; we never pass --force.
pub fn answer_commands(s: &Session, text: &str) -> Option<Vec<Vec<String>>> {
    let id = s.term_id.as_deref()?;
    match s.term.as_deref()? {
        "cmux" => {
            let mut send = vec!["cmux".to_string(), "send".to_string()];
            let mut key = vec!["cmux".to_string(), "send-key".to_string()];
            if let Some(ws) = s.term_ws.as_deref() {
                send.extend(arg("--workspace", ws));
                key.extend(arg("--workspace", ws));
            }
            send.extend(arg("--surface", id));
            key.extend(arg("--surface", id));
            send.extend(["--".to_string(), text.to_string()]);
            key.push("enter".to_string());
            Some(vec![send, key])
        }
        "herdr" => Some(vec![vec!["herdr".into(), "agent".into(), "prompt".into(), id.into(), text.into()]]),
        _ => None,
    }
}

/// The macOS app this process was started from (the terminal that shows the multiplexer), from
/// the bundle id the system puts in the environment. The focus button raises it, since the
/// multiplexer only switches panes inside its own window.
pub fn host_app(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    var("__CFBundleIdentifier")
        .filter(|id| !id.is_empty() && id.len() <= 100 && !id.starts_with('-'))
        .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
}

/// The command that brings that app to the front.
pub fn raise_command(bundle_id: &str) -> Vec<String> {
    vec!["open".into(), "-b".into(), bundle_id.into()]
}

/// herdr numbers panes by position, so a recorded pane id can come to name another pane.
/// The command whose output says which agent session the pane holds now; None for terminals
/// whose ids are not reused.
pub fn occupant_command(s: &Session) -> Option<Vec<String>> {
    match (s.term.as_deref()?, s.term_id.as_deref()?) {
        ("herdr", id) => Some(vec!["herdr".into(), "agent".into(), "get".into(), id.into()]),
        _ => None,
    }
}

/// Whether the output of `occupant_command` names this session.
pub fn occupant_is(output: &str, session_id: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(output)
        .ok()
        .and_then(|v| v.pointer("/result/agent/agent_session/value").and_then(|x| x.as_str()).map(|x| x == session_id))
        .unwrap_or(false)
}

const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Runs one command without a shell and returns its stdout. Err carries what to tell the human.
pub fn run(argv: &[String]) -> Result<String, String> {
    let (program, args) = argv.split_first().ok_or("空の命令")?;
    let mut child = Command::new(program)
        .args(args)
        .env("CMUX_QUIET", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e: io::Error| format!("{program} を起動できない: {e}"))?;
    // Hooks give up on the append lock after a few seconds, so a command that hangs must not
    // keep the server (and the lock) for longer than that.
    let deadline = std::time::Instant::now() + RUN_TIMEOUT;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("{program}: {} 秒待っても終わらないので止めた", RUN_TIMEOUT.as_secs()));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let msg = if out.stderr.is_empty() { stdout } else { String::from_utf8_lossy(&out.stderr).into_owned() };
        return Err(format!("{program}: {}", msg.trim().chars().take(200).collect::<String>()));
    }
    Ok(stdout)
}
