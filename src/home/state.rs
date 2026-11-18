//! Deriving each session's state from the events. This is `Step` in docs/tla/Home.tla.
//! Everything here is free of side effects; the caller passes the events and the current time.

use std::collections::HashMap;

use super::note::Attention;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum St {
    #[default]
    None,
    Idle,
    Working,
    Permission,
    Stopped,
    /// Stopped, but waiting on background work (a stop line with `background`). The resolve and
    /// kill lines do not apply, as in every state other than Stopped.
    Background,
    Resolved,
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Start,
    Prompt,
    Permission,
    Stop,
    Resolved,
    Kill,
    End,
    /// Codex: a turn was interrupted (also when the human denied an approval).
    Interrupt,
}

impl Kind {
    pub fn parse(name: &str) -> Option<Kind> {
        Some(match name {
            "start" => Kind::Start,
            "prompt" => Kind::Prompt,
            "permission" => Kind::Permission,
            "stop" => Kind::Stop,
            "resolved" => Kind::Resolved,
            "kill" => Kind::Kill,
            "end" => Kind::End,
            "interrupt" => Kind::Interrupt,
            _ => return None,
        })
    }
}

/// One line of events.jsonl. `ts_us` is `ts` in microseconds since the epoch.
#[derive(Clone, Debug, Default)]
pub struct Event {
    pub ts: String,
    pub ts_us: i64,
    pub session_id: String,
    pub cwd: String,
    pub kind: Option<Kind>,
    pub pid: Option<i64>,
    pub pid_started: Option<i64>,
    pub transcript_path: Option<String>,
    pub report_path: Option<String>,
    pub report_html: Option<String>,
    pub report_title: Option<String>,
    pub decisions: i64,
    pub resolves: Option<String>,
    /// stop only: the agent still waits on background work.
    pub background: bool,
    /// stop only: the agent's `Home:` line. Missing on old lines, which reads as Unknown.
    pub attention: Attention,
    pub note: Option<String>,
    /// start only: claude or codex.
    pub agent: Option<String>,
    /// stop only: the questions of the report's decisions, and its goal.
    pub questions: Vec<String>,
    pub report_goal: Option<String>,
}

/// The accumulator of the fold. `st`, `last_stop` and `decisions` are the model's `st`,
/// `lastStop` and `dec`; the rest is carried along for display and for the kill check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    pub cwd: String,
    pub st: St,
    pub last_stop: Option<String>,
    /// `last_stop` in microseconds since the epoch.
    pub last_stop_us: Option<i64>,
    pub decisions: i64,
    /// The model's `att`: from the current stop.
    pub attention: Attention,
    pub note: Option<String>,
    pub report_path: Option<String>,
    pub report_html: Option<String>,
    pub report_title: Option<String>,
    pub questions: Vec<String>,
    pub report_goal: Option<String>,
    /// ts of the latest start line (startup, resume or clear). None if the log has none.
    pub started: Option<String>,
    /// From the latest start line: claude or codex. None on old lines.
    pub agent: Option<String>,
    pub pid: Option<i64>,
    pub pid_started: Option<i64>,
    pub transcript_path: Option<String>,
    pub last_ts: String,
    pub last_us: i64,
}

/// Whether `resolves` names the stop the session is in now.
fn current_stop(acc: &Session, resolves: &Option<String>) -> bool {
    acc.st == St::Stopped && resolves.is_some() && *resolves == acc.last_stop
}

/// One step of the fold, the same rules as `Step(acc, ev, i)` in Home.tla.
pub fn step(acc: &Session, ev: &Event) -> Session {
    let mut next = acc.clone();
    match ev.kind {
        Some(Kind::Start) => {
            next.st = St::Idle;
            next.last_stop = None;
            next.last_stop_us = None;
            next.decisions = 0;
            next.attention = Attention::Unknown;
            next.note = None;
            next.report_path = None;
            next.report_html = None;
            next.report_title = None;
            next.questions = Vec::new();
            next.report_goal = None;
            next.started = Some(ev.ts.clone());
            next.agent = ev.agent.clone();
            next.pid = ev.pid;
            next.pid_started = ev.pid_started;
            next.transcript_path = ev.transcript_path.clone();
        }
        Some(Kind::Prompt) => next.st = St::Working,
        Some(Kind::Permission) => next.st = St::Permission,
        Some(Kind::Stop) => {
            next.st = if ev.background { St::Background } else { St::Stopped };
            next.last_stop = Some(ev.ts.clone());
            next.last_stop_us = Some(ev.ts_us);
            next.decisions = ev.decisions;
            next.attention = ev.attention;
            next.note = ev.note.clone();
            next.report_path = ev.report_path.clone();
            next.report_html = ev.report_html.clone();
            next.report_title = ev.report_title.clone();
            next.questions = ev.questions.clone();
            next.report_goal = ev.report_goal.clone();
        }
        Some(Kind::Resolved) if current_stop(acc, &ev.resolves) => next.st = St::Resolved,
        Some(Kind::Kill) if current_stop(acc, &ev.resolves) => next.st = St::Ended,
        Some(Kind::End) => next.st = St::Ended,
        // The turn ended without a Stop; the agent waits for the next prompt, like after start.
        Some(Kind::Interrupt) if matches!(acc.st, St::Working | St::Permission | St::Background) => next.st = St::Idle,
        _ => {}
    }
    next.session_id = ev.session_id.clone();
    if !ev.cwd.is_empty() {
        next.cwd = ev.cwd.clone();
    }
    next.last_ts = ev.ts.clone();
    next.last_us = ev.ts_us;
    next
}

/// Folds the events in append order and returns one session per session id, in order of first appearance.
pub fn derive(events: &[Event]) -> Vec<Session> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut out: Vec<Session> = Vec::new();
    for ev in events {
        let i = *index.entry(ev.session_id.as_str()).or_insert_with(|| {
            out.push(Session::default());
            out.len() - 1
        });
        out[i] = step(&out[i], ev);
    }
    out
}

/// `Shown(a)` in Home.tla.
pub fn shown(s: &Session) -> bool {
    matches!(s.st, St::Working | St::Permission | St::Stopped | St::Background)
}

/// Sessions without an update for this long are hidden.
pub const HIDE_AFTER_US: i64 = 12 * 3600 * 1_000_000;

/// Display order. Rows that wait for a human: decisions wanted, permission, a stop whose
/// `Home:` line says 要, a stop without a readable `Home:` line. Then a stop that says 不要,
/// then working (with working in the background).
pub fn rank(s: &Session) -> u8 {
    match s.st {
        St::Stopped if s.decisions > 0 => 0,
        St::Permission => 1,
        St::Stopped if s.attention == Attention::Needed => 2,
        St::Stopped if s.attention == Attention::Unknown => 3,
        St::Stopped => 4,
        _ => 5,
    }
}

/// Whether the row belongs to the rows that wait for a human.
pub fn waits_for_human(s: &Session) -> bool {
    rank(s) <= 3
}

/// The rows of the table: shown, updated within 12 hours, sorted by rank and then newest first.
pub fn visible(sessions: Vec<Session>, now_us: i64) -> Vec<Session> {
    let mut rows: Vec<Session> = sessions
        .into_iter()
        .filter(|s| shown(s) && now_us - s.last_us < HIDE_AFTER_US)
        .collect();
    rows.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(b.last_us.cmp(&a.last_us))
            .then(a.session_id.cmp(&b.session_id))
    });
    rows
}
