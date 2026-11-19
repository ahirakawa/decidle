//! The terminals the Home page can drive: bringing an agent's terminal to the front, and typing
//! the report's recommended options into it.

use std::io;
use std::process::{Command, Stdio};

use super::state::{Session, St};
use crate::i18n::Lang;

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

/// Reads the terminal from the hook's environment, taking herdr, then hc, then cmux. herdr comes
/// first since the TUI may run inside an app terminal (or a cmux one), and then its pane is the
/// one to address. hc comes before cmux since an hc terminal inherits the environment of the
/// shell that started herdr-core, which may hold a cmux surface.
pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Option<Recorded> {
    let get = |k: &str| var(k).filter(|v| valid_id(v));
    if let Some(id) = get("HERDR_PANE_ID") {
        return Some(Recorded { kind: "herdr", id, workspace: None });
    }
    if let Some(id) = get("HC_TERMINAL_ID") {
        return Some(Recorded { kind: "hc", id, workspace: None });
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
        // A terminal on another machine (`host/t1`) fails `valid_id` on its slash.
        (Some("hc"), Some(id)) if valid_id(&id) => (term, Some(id), None),
        // cmux also takes an index or a short ref where it takes an id, and those can come to
        // name another terminal. Only the UUIDs it puts in the environment are accepted, and
        // the workspace must come with the surface.
        (Some("cmux"), Some(id)) if is_uuid(&id) && ws.as_deref().is_some_and(is_uuid) => (term, Some(id), ws),
        _ => (None, None, None),
    }
}

/// The answer line by language: the first word of every line, and one question with its
/// recommended option, where `{q}` and `{r}` are filled in, and the marks taken off the end of a
/// question. Neither text holds a character `plain` refuses, so the line stays plain when the
/// question and the option are.
struct AnswerWords {
    prefix: &'static str,
    part: &'static str,
    trim: &'static [char],
}

const ANSWER_JA: AnswerWords = AnswerWords { prefix: "回答:", part: "{q}は「{r}」で進めて。", trim: &['?', '？', '。'] };
const ANSWER_EN: AnswerWords = AnswerWords { prefix: "Answer:", part: "{q}: go with “{r}”.", trim: &['?', '？', '。', '.'] };

fn answer_words(lang: Lang) -> &'static AnswerWords {
    match lang {
        Lang::Ja => &ANSWER_JA,
        Lang::En => &ANSWER_EN,
    }
}

/// Characters that a shell or the terminal's own escapes would act on. The text is typed into
/// the agent's prompt, but if the agent died in the last moment a shell reads it instead.
fn plain(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(|c| c.is_control() || matches!(c, '`' | '$' | ';' | '|' | '&' | '<' | '>' | '\\' | '!' | '\'' | '"'))
}

/// The one line the answer button types: built from the stop line only, never from the request.
/// None when a decision has no recommended option, or the text is not plain. Written in `lang`.
pub fn answer_text(s: &Session, lang: Lang) -> Option<String> {
    let w = answer_words(lang);
    if s.questions.is_empty() || s.questions.len() != s.recommends.len() {
        return None;
    }
    let parts: Vec<String> = s
        .questions
        .iter()
        .zip(&s.recommends)
        .map(|(q, r)| crate::i18n::fill(w.part, &[("q", q.trim_end_matches(w.trim)), ("r", r)]))
        .collect();
    let ok = s.questions.iter().chain(&s.recommends).all(|t| plain(t));
    // The line starts with a fixed word. If the agent died in the last moment and a shell reads
    // the line, the shell looks for a command of this name and finds none; without it, the
    // first word of the agent's question would be run as a command.
    ok.then(|| format!("{} {}", w.prefix, parts.join(" ")))
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
        "hc" => Some(vec![vec!["herdr-app".into(), "do".into(), "pane.focus".into(), "--terminal".into(), id.into()]]),
        _ => None,
    }
}

/// The commands that type `text` and Enter into the terminal. cmux refuses by itself when the
/// agent's prompt holds a draft or a dialog is open; we never pass --force. hc refuses a blocked
/// terminal (exit code 9) without --force, and waits on a person's draft; we pass neither --force
/// nor --if (a stopped Claude Code reads as done, which --if cannot name), nor --wait.
pub fn answer_commands(s: &Session, text: &str) -> Option<Vec<Vec<String>>> {
    let id = s.term_id.as_deref()?;
    match s.term.as_deref()? {
        "cmux" => {
            let mut send = vec!["cmux".to_string(), "send".to_string()];
            if let Some(ws) = s.term_ws.as_deref() {
                send.extend(arg("--workspace", ws));
            }
            send.extend(arg("--surface", id));
            // The text and Enter go in one command: cmux types `\n` as Enter. With a separate
            // send-key, a failure there would leave the text in the prompt with no Enter. The
            // text itself holds no backslash (see `plain`), so no other escape can appear.
            send.extend(["--".to_string(), format!("{text}\\n")]);
            Some(vec![send])
        }
        "herdr" => Some(vec![vec!["herdr".into(), "agent".into(), "prompt".into(), id.into(), text.into()]]),
        "hc" => Some(vec![vec!["hc".into(), "send".into(), id.into(), text.into()]]),
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

/// A herdr pane keeps its id when the agent in it ends and another one starts, so the pane
/// can come to hold another session; an hc terminal goes on with a shell, or another program.
/// The command whose output says what the terminal holds now; None for other terminals. The
/// check and the send are two commands, so a swap in between is not caught.
pub fn occupant_command(s: &Session) -> Option<Vec<String>> {
    match (s.term.as_deref()?, s.term_id.as_deref()?) {
        ("herdr", id) => Some(vec!["herdr".into(), "agent".into(), "get".into(), id.into()]),
        ("hc", id) => Some(vec!["hc".into(), "get".into(), id.into()]),
        _ => None,
    }
}

/// Whether the output of `occupant_command` shows this session: herdr names the agent session;
/// hc names no session, so its foreground process must be the pid recorded at start.
pub fn occupant_is(output: &str, s: &Session) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(output) else {
        return false;
    };
    match s.term.as_deref() {
        Some("herdr") => v.pointer("/result/agent/agent_session/value").and_then(|x| x.as_str()) == Some(s.session_id.as_str()),
        Some("hc") => s.pid.is_some() && v.pointer("/foreground/pid").and_then(|x| x.as_i64()) == s.pid,
        _ => false,
    }
}

/// The length at which the ask line cuts a question (`ASK_QUESTION_MAX_CHARS` in events.rs).
const QUESTION_MAX_CHARS: usize = 200;

/// Whether the Home page may press an option of the session's question: one question, not
/// multi-select, with a text that is not empty and was not cut (at most 200 characters as
/// recorded; a cut one ends in an added `…`), one to eight options, in an hc terminal. An empty
/// or cut text cannot be matched against the message of `hc get`. A digit key picks and
/// confirms an option there (seen with Claude Code 2.1.327); it was not tried in Herdr TUI or cmux.
pub fn choosable(s: &Session) -> bool {
    s.st == St::Asking
        && s.term.as_deref() == Some("hc")
        && s.term_id.is_some()
        && matches!(s.ask.as_slice(), [q] if !q.multi
            && !q.question.is_empty()
            && q.question.chars().count() <= QUESTION_MAX_CHARS
            && (1..=8).contains(&q.options.len()))
}

/// Whether the output of `hc get` shows the question prompt open: the status is blocked on a
/// question whose message holds the recorded question. A digit typed anywhere else would land in
/// the agent's prompt.
pub fn question_open(output: &str, question: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(output) else {
        return false;
    };
    let field = |k: &str| v.pointer(&format!("/status/{k}")).and_then(|x| x.as_str());
    field("state") == Some("blocked") && field("kind") == Some("question") && field("msg").is_some_and(|m| m.contains(question))
}

/// The command that presses the digit key of option `option` (from 0) in an hc terminal.
pub fn choose_command(s: &Session, option: usize) -> Option<Vec<String>> {
    match (s.term.as_deref()?, s.term_id.as_deref()?) {
        ("hc", id) => Some(vec!["hc".into(), "keys".into(), id.into(), (option + 1).to_string()]),
        _ => None,
    }
}

const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// What `run` tells the human when a command fails, by language. `{program}` and `{secs}` are
/// filled in.
struct RunWords {
    empty: &'static str,
    cannot_start: &'static str,
    timed_out: &'static str,
}

const RUN_JA: RunWords = RunWords {
    empty: "空の命令",
    cannot_start: "{program} を起動できない",
    timed_out: "{program}: {secs} 秒待っても終わらないので止めた",
};

const RUN_EN: RunWords = RunWords {
    empty: "empty command",
    cannot_start: "cannot start {program}",
    timed_out: "{program}: stopped after {secs} seconds without finishing",
};

fn run_words(lang: Lang) -> &'static RunWords {
    match lang {
        Lang::Ja => &RUN_JA,
        Lang::En => &RUN_EN,
    }
}

/// Runs one command without a shell and returns its stdout. Err carries what to tell the human,
/// in `lang`.
pub fn run(argv: &[String], lang: Lang) -> Result<String, String> {
    let w = run_words(lang);
    let (program, args) = argv.split_first().ok_or(w.empty)?;
    let mut child = Command::new(program)
        .args(args)
        .env("CMUX_QUIET", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e: io::Error| format!("{}: {e}", w.cannot_start.replace("{program}", program)))?;
    // Hooks give up on the append lock after a few seconds, so a command that hangs must not
    // keep the server (and the lock) for longer than that.
    let deadline = std::time::Instant::now() + RUN_TIMEOUT;
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(w.timed_out.replace("{secs}", &RUN_TIMEOUT.as_secs().to_string()).replace("{program}", program));
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
