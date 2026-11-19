//! `decidle serve`: a small HTTP server on 127.0.0.1 for the Home page.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::activity;
use super::events::{self, LogLock};
use super::procinfo;
use super::read;
use super::state::{self, Kind, Session, St};
use super::term;
use crate::i18n::Lang;

const PAGE: &str = include_str!("page.html");
const TOKEN_HEADER: &str = "x-decidle-token";
const MAX_HEAD_BYTES: u64 = 16 * 1024;
const MAX_BODY_BYTES: usize = 4096;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// The server holds the lock only for one read of the log, so it may wait longer than a hook.
const KILL_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

/// The server's replies and messages, by language. The replies are shown on the Home page as
/// they are. A message that takes a detail is followed by `: <detail>`; `{path}` and `{port}`
/// are filled in.
struct Words {
    header_cut: &'static str,
    body_too_large: &'static str,
    need_session: &'static str,
    need_choose: &'static str,
    need_target: &'static str,
    newer_stop_than_shown: &'static str,
    no_report_file: &'static str,
    no_session: &'static str,
    /// Neither resolve nor kill applies to a session that waits on background work.
    background: &'static str,
    newer_event: &'static str,
    no_pid: &'static str,
    no_transcript: &'static str,
    worked_since: &'static str,
    transcript_unreadable: &'static str,
    other_process: &'static str,
    cannot_sigterm: &'static str,
    cannot_focus: &'static str,
    cannot_answer_text: &'static str,
    cannot_answer_term: &'static str,
    newer_than_question: &'static str,
    cannot_choose: &'static str,
    no_option: &'static str,
    pane_taken: &'static str,
    question_not_open: &'static str,
    pane_lost: &'static str,
    command_failed: &'static str,
    cannot_write_token: &'static str,
    cannot_listen: &'static str,
    /// The words of the Home page's script, embedded in the page as a JSON object. `{name}` in a
    /// word is filled in by the script. Every language has the same names (tests/home_api.rs).
    page: &'static [(&'static str, &'static str)],
}

const JA: Words = Words {
    header_cut: "header が途中で切れた",
    body_too_large: "body が大きすぎる",
    need_session: "session_id が要る",
    need_choose: "session_id と ask と option が要る",
    need_target: "session_id と resolves が要る",
    newer_stop_than_shown: "見ている停止のあとに新しい停止がある",
    no_report_file: "報告のファイルがない",
    no_session: "そのセッションの記録がない",
    background: "裏で作業中なので、解決と終了は受け付けない",
    newer_event: "見ていた停止のあとに新しい出来事がある",
    no_pid: "claude のプロセス番号の記録がない",
    no_transcript: "会話記録の場所の記録がないので、止まっているか確かめられない",
    worked_since: "会話記録では、停止のあとに作業が進んでいる",
    transcript_unreadable: "会話記録を読めないので、止まっているか確かめられない",
    other_process: "記録したプロセス番号は、記録した起動時刻の claude ではない",
    cannot_sigterm: "SIGTERM を送れない",
    cannot_focus: "このセッションの端末は、Home から前に出せない",
    cannot_answer_text: "勧めのない問いがあるか、送れない文字が入っているので、端末で答えてほしい",
    cannot_answer_term: "このセッションの端末には、Home から答えを送れない",
    newer_than_question: "見ていた質問のあとに新しい出来事がある",
    cannot_choose: "この質問は Home から選べないので、端末で回答してほしい",
    no_option: "その番号の選択肢はない",
    pane_taken: "記録したペインには、いま別のものが入っている",
    question_not_open: "端末に、この質問の選択画面が開いていない",
    pane_lost: "記録したペインに、このセッションが見つからない",
    command_failed: "端末への命令が通らなかった",
    cannot_write_token: "合言葉を {path} に書けない",
    cannot_listen: "127.0.0.1:{port} で待ち受けられない",
    page: &[
        ("title", "あなた待ち"),
        ("label_decide", "決めてほしい"),
        ("label_permission", "許可待ち"),
        ("label_ask", "質問に回答"),
        ("label_need", "確認してほしい"),
        ("label_unknown", "止まっている"),
        ("label_noneed", "対応は不要"),
        ("label_working", "作業中"),
        ("label_background", "裏で作業中"),
        ("missing", "一言なしで止まっている"),
        ("unit_sec", "秒"),
        ("unit_min", "分"),
        ("unit_hour", "時間"),
        ("restarted", "サーバーが起動し直されたので、ページを読み込み直す。もう一度押してほしい。"),
        ("failed", "失敗: {status}"),
        ("cannot_send", "送れない: {error}"),
        ("started", "開始 {time}"),
        ("since", "{time}〜"),
        ("kill_confirm", "{folder}({start})の {agent} を終了する。よいか。"),
        ("permission", "ターミナルで許可を出してほしい"),
        ("multi", "複数選べる"),
        ("choose_title", "この選択肢を端末で選ぶ"),
        ("bullet", "・{text}"),
        ("recommend", "推奨: {text}"),
        ("decisions", "決めてほしいこと {n} 件"),
        ("lab", "{folder} ／ {label}"),
        ("goal", "目的: {text}"),
        ("meta_sep", " ・ "),
        ("no_term", "この端末は Home から操作できない(herdr、Herdr アプリ、cmux の中で始めたセッションだけ)"),
        ("ask_no_term", "この端末は Home から前に出せないので、端末で回答してほしい"),
        ("send_head", "「推奨で進める」で端末に送る文"),
        ("go_ask", "回答しに行く"),
        ("focus_title", "このセッションの端末を前に出す"),
        ("answer", "推奨で進める"),
        ("answer_title", "上の文を端末に入力して Enter を押す"),
        ("go", "端末へ行く"),
        ("resolve", "外す"),
        ("resolve_title", "見たので一覧から外す。エージェントには何も送らない"),
        ("end", "終了…"),
        ("report_gone", "報告のファイルを読めない(もう消えているかもしれない)。"),
        ("loading", "読み込み中…"),
        ("open_tab", "別のタブで開く"),
        ("recommend_badge", "推奨"),
        ("reason", "理由: {text}"),
        ("h_decisions", "決めてほしいこと"),
        ("h_verified", "確かめたこと"),
        ("h_unverified", "確かめていないこと"),
        ("h_deliverables", "成果物"),
        ("clear_title", "待ちなし。"),
        ("clear_sub", "そのまま自分の作業を続けて大丈夫。"),
        ("background_mark", "(裏)"),
        ("skipped", "読めなかった行: {n} 件 ・ "),
        ("foot", "5秒ごとに取り直す ・ ↑↓ で選ぶ ・ T で見た目を替える"),
        ("skin_naname", "斜め"),
        ("skin_shimen", "紙面"),
        ("fetch_failed", "取得できない: {error}"),
    ],
};

const EN: Words = Words {
    header_cut: "the header was cut off",
    body_too_large: "the body is too large",
    need_session: "session_id is required",
    need_choose: "session_id, ask and option are required",
    need_target: "session_id and resolves are required",
    newer_stop_than_shown: "there is a newer stop than the one shown",
    no_report_file: "the report file is missing",
    no_session: "no record of that session",
    background: "working in the background, so dismiss and end are not accepted",
    newer_event: "something new happened after the stop you saw",
    no_pid: "no process id of claude was recorded",
    no_transcript: "no transcript location was recorded, so it cannot be checked that the agent has stopped",
    worked_since: "the transcript shows work after the stop",
    transcript_unreadable: "cannot read the transcript, so it cannot be checked that the agent has stopped",
    other_process: "the recorded process id is not the claude started at the recorded time",
    cannot_sigterm: "cannot send SIGTERM",
    cannot_focus: "this session's terminal cannot be brought forward from Home",
    cannot_answer_text: "a question has no recommendation or holds characters that cannot be sent; answer in the terminal",
    cannot_answer_term: "answers cannot be sent from Home to this session's terminal",
    newer_than_question: "something new happened after the question you saw",
    cannot_choose: "this question cannot be answered from Home; answer in the terminal",
    no_option: "there is no option with that number",
    pane_taken: "the recorded pane now holds something else",
    question_not_open: "the terminal does not show the options of this question",
    pane_lost: "this session was not found in the recorded pane",
    command_failed: "the terminal command failed",
    cannot_write_token: "cannot write the token to {path}",
    cannot_listen: "cannot listen on 127.0.0.1:{port}",
    page: &[
        ("title", "Your turn"),
        ("label_decide", "Decide"),
        ("label_permission", "Needs permission"),
        ("label_ask", "Answer a question"),
        ("label_need", "Please check"),
        ("label_unknown", "Stopped"),
        ("label_noneed", "Nothing to do"),
        ("label_working", "Working"),
        ("label_background", "Working in background"),
        ("missing", "Stopped without a note"),
        ("unit_sec", "s"),
        ("unit_min", "m"),
        ("unit_hour", "h"),
        ("restarted", "The server was restarted, so the page will reload. Please press again."),
        ("failed", "Failed: {status}"),
        ("cannot_send", "Cannot send: {error}"),
        ("started", "started {time}"),
        ("since", "from {time}"),
        ("kill_confirm", "End {agent} in {folder} ({start})?"),
        ("permission", "Please grant permission in the terminal"),
        ("multi", "multiple choice"),
        ("choose_title", "Pick this option in the terminal"),
        ("bullet", "• {text}"),
        ("recommend", "Recommended: {text}"),
        ("decisions", "{n} decisions to make"),
        ("lab", "{folder} / {label}"),
        ("goal", "Goal: {text}"),
        ("meta_sep", " · "),
        ("no_term", "This terminal cannot be controlled from Home (only sessions started in herdr, the Herdr app or cmux)"),
        ("ask_no_term", "This terminal cannot be brought forward from Home; answer in the terminal"),
        ("send_head", "What \"Go with recommended\" types into the terminal"),
        ("go_ask", "Go answer"),
        ("focus_title", "Bring this session's terminal forward"),
        ("answer", "Go with recommended"),
        ("answer_title", "Type the text above into the terminal and press Enter"),
        ("go", "Go to terminal"),
        ("resolve", "Dismiss"),
        ("resolve_title", "Seen; remove from the list. Nothing is sent to the agent"),
        ("end", "End…"),
        ("report_gone", "Cannot read the report file (it may have been deleted)."),
        ("loading", "Loading…"),
        ("open_tab", "Open in a new tab"),
        ("recommend_badge", "Recommended"),
        ("reason", "Reason: {text}"),
        ("h_decisions", "Decisions"),
        ("h_verified", "Verified"),
        ("h_unverified", "Not verified"),
        ("h_deliverables", "Deliverables"),
        ("clear_title", "Nothing waiting."),
        ("clear_sub", "Carry on with your own work."),
        ("background_mark", " (bg)"),
        ("skipped", "Unreadable lines: {n} · "),
        ("foot", "Refreshes every 5 s · ↑↓ to select · T to change the look"),
        ("skin_naname", "Slant"),
        ("skin_shimen", "Paper"),
        ("fetch_failed", "Cannot fetch: {error}"),
    ],
};

fn words(lang: Lang) -> &'static Words {
    match lang {
        Lang::Ja => &JA,
        Lang::En => &EN,
    }
}

/// Where `decidle serve` writes its token for the Herdr app mod.
pub fn token_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share/decidle/home.token")
}

/// Runs one terminal command and returns its stdout, or what to tell the human.
type Runner = fn(&[String]) -> Result<String, String>;

pub struct Server {
    listener: TcpListener,
    port: u16,
    token: String,
    /// Changes with every start of the server. A page loaded from an earlier start holds a
    /// token this server does not accept; it sees the new value and reloads itself.
    boot: String,
    events: PathBuf,
    /// Replaces the terminal commands (`term::run`) in tests.
    runner: Option<Runner>,
    /// The terminal app to raise after a focus; None leaves the windows as they are.
    host_app: Option<String>,
    lang: Lang,
    w: &'static Words,
}

struct Request {
    method: String,
    /// The request target as sent, with any query.
    target: String,
    path: String,
    headers: HashMap<String, String>,
    /// Host, Origin or the token header came more than once.
    ambiguous: bool,
    body: Vec<u8>,
}

struct Response {
    status: u16,
    content_type: &'static str,
    csp: &'static str,
    body: String,
}

/// The Home page needs its inline script and fetches from itself.
const HOME_CSP: &str =
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'";
/// A report page is static HTML with inline styles. It runs no script, so that text taken from
/// a report cannot reach the token on this origin.
const REPORT_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'";
const MAX_REPORT_PAGE_BYTES: u64 = 5 * 1024 * 1024;

fn json_response(status: u16, value: Value) -> Response {
    Response { status, content_type: "application/json; charset=utf-8", csp: HOME_CSP, body: value.to_string() }
}

fn error(status: u16, message: &str) -> Response {
    json_response(status, json!({ "ok": false, "error": message }))
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn now_us() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0)
}

/// Compares without stopping at the first difference.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The link to a report page that decidle stop wrote: `/report` followed by the absolute
/// path, with every byte other than unreserved characters and `/` percent-encoded. A request is
/// served only when its raw path equals the link of a recorded page, byte for byte, so `..`,
/// other encodings and extra characters never match.
pub fn report_link(html_path: &str) -> Option<String> {
    if !html_path.starts_with('/') {
        return None;
    }
    let mut link = String::from("/report");
    for b in html_path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            link.push(b as char);
        } else {
            link.push_str(&format!("%{b:02X}"));
        }
    }
    Some(link)
}

/// Percent-encodes everything but unreserved characters, as `encodeURIComponent` does for the
/// characters of a timestamp.
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn read_page(path: &str) -> io::Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    // O_NOFOLLOW covers only the last part. The recorded path is the real one (gate.rs resolves
    // links before recording), so a folder on the way that has become a link is refused here.
    let parent = Path::new(path).parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no folder"))?;
    if std::fs::canonicalize(parent)? != parent {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "a folder on the path is a link"));
    }
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_REPORT_PAGE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a report page"));
    }
    let mut text = String::new();
    file.take(MAX_REPORT_PAGE_BYTES).read_to_string(&mut text)?;
    Ok(text)
}

fn state_name(s: &Session) -> &'static str {
    match s.st {
        St::Stopped if s.decisions > 0 => "decide",
        St::Stopped => "stopped",
        St::Permission => "permission",
        St::Asking => "ask",
        St::Background => "background",
        _ => "working",
    }
}

fn row(s: &Session, lang: Lang) -> Value {
    let folder = s.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    json!({
        "session_id": s.session_id,
        "folder": folder,
        "cwd": s.cwd,
        "state": state_name(s),
        "started": s.started,
        "agent": s.agent.as_deref().unwrap_or("claude"),
        "last_stop": s.last_stop,
        "report_title": s.report_title,
        "report_goal": s.report_goal,
        "questions": s.questions,
        "recommends": s.recommends,
        // The reducer keeps the questions only while Asking.
        "ask": s.ask,
        "ask_ts": s.ask_ts,
        "choosable": term::choosable(s),
        "term": s.term,
        // The Herdr app mod brings an hc terminal to the front by this id, without the server.
        "term_id": s.term_id,
        "answer_text": if s.st == St::Stopped && s.term.is_some() { term::answer_text(s, lang) } else { None },
        "report_url": s.report_html.as_deref().and_then(report_link),
        "decisions": s.decisions,
        "attention": s.attention.as_str(),
        "note": s.note,
        "waits_for_human": state::waits_for_human(s),
        "last_ts": s.last_ts,
    })
}

#[derive(Deserialize)]
struct Target {
    session_id: String,
    resolves: String,
}

/// What focus takes: the session only. Focus records nothing and does not depend on a stop, and a
/// session asking a question before its first stop has none to name.
#[derive(Deserialize)]
struct FocusTarget {
    session_id: String,
}

/// What choose takes: the session, the ts of the ask line the human saw, and the pressed option.
#[derive(Deserialize)]
struct ChooseTarget {
    session_id: String,
    ask: String,
    option: usize,
}

fn read_request(stream: &mut TcpStream, w: &Words) -> io::Result<Request> {
    let mut reader = BufReader::new(Read::take(&mut *stream, MAX_HEAD_BYTES));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let path = target.split('?').next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    let mut ambiguous = false;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, w.header_cut));
        }
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let checked = matches!(key.as_str(), "host" | "origin" | TOKEN_HEADER);
            if headers.insert(key, v.trim().to_string()).is_some() && checked {
                ambiguous = true;
            }
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, w.body_too_large));
    }
    let mut body = vec![0u8; len];
    // The BufReader may already hold part of the body.
    reader.read_exact(&mut body)?;
    Ok(Request { method, target, path, headers, ambiguous, body })
}

fn write_response(stream: &mut TcpStream, r: &Response) -> io::Result<()> {
    let reason = match r.status {
        200 => "OK",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        502 => "Bad Gateway",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         X-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\
         Content-Security-Policy: {}\r\nConnection: close\r\n\r\n",
        r.status,
        r.content_type,
        r.body.len(),
        r.csp
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(r.body.as_bytes())?;
    stream.flush()
}

impl Server {
    /// Listens on 127.0.0.1:`port`. Port 0 picks a free port (for tests).
    pub fn bind(port: u16, events: PathBuf, lang: Lang) -> io::Result<Server> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        let (token, boot) = (random_token()?, random_token()?[..16].to_string());
        Ok(Server { listener, port, token, boot, events, runner: None, host_app: None, lang, w: words(lang) })
    }

    pub fn set_host_app(&mut self, app: Option<String>) {
        self.host_app = app;
    }

    pub fn set_runner(&mut self, runner: fn(&[String]) -> Result<String, String>) {
        self.runner = Some(runner);
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Writes this start's token to `path`, readable by the owner only, for the Herdr app mod,
    /// which cannot read it from the page. The file is written beside and renamed over, so a
    /// reader never sees half a token.
    pub fn write_token(&self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no folder"))?;
        events::private_dir(dir)?;
        let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
        let tmp = dir.join(format!(".{}.{}", name.to_string_lossy(), std::process::id()));
        let _ = std::fs::remove_file(&tmp);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&tmp)?;
        let written = file.write_all(self.token.as_bytes()).and_then(|_| file.sync_all()).and_then(|_| std::fs::rename(&tmp, path));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        written
    }

    /// Serves one connection at a time, forever.
    pub fn run(self) {
        for stream in self.listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
            let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
            let response = match read_request(&mut stream, self.w) {
                Ok(req) => self.handle(&req),
                Err(e) => error(400, &e.to_string()),
            };
            let _ = write_response(&mut stream, &response);
        }
    }

    fn allowed_hosts(&self) -> [String; 2] {
        [format!("127.0.0.1:{}", self.port), format!("localhost:{}", self.port)]
    }

    /// Host must name this server, an Origin (when sent) must be this server, and a POST must
    /// carry both an Origin and the token.
    fn permitted(&self, req: &Request) -> bool {
        if req.ambiguous {
            return false;
        }
        let hosts = self.allowed_hosts();
        let host_ok = req.headers.get("host").is_some_and(|h| hosts.contains(h));
        let origin = req.headers.get("origin");
        let origin_ok = origin.is_none_or(|o| hosts.iter().any(|h| *o == format!("http://{h}")));
        if !host_ok || !origin_ok {
            return false;
        }
        if req.method == "GET" {
            return true;
        }
        let token_ok = req.headers.get(TOKEN_HEADER).is_some_and(|t| same_secret(t, &self.token));
        origin.is_some() && token_ok
    }

    fn handle(&self, req: &Request) -> Response {
        if !self.permitted(req) {
            return error(403, "forbidden");
        }
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/") => Response {
                status: 200,
                content_type: "text/html; charset=utf-8",
                csp: HOME_CSP,
                body: self.page(),
            },
            ("GET", path) if path.starts_with("/report/") => self.report_page(&req.target),
            ("GET", "/api/sessions") => self.sessions(),
            ("GET", "/api/report") => self.report_json(&req.target),
            ("POST", "/api/resolve") => self.with_target(req, |t| self.resolve(t)),
            ("POST", "/api/kill") => self.with_target(req, |t| self.kill(t)),
            ("POST", "/api/focus") => match serde_json::from_slice::<FocusTarget>(&req.body) {
                Ok(t) if !t.session_id.is_empty() => self.focus(&t.session_id),
                _ => error(400, self.w.need_session),
            },
            ("POST", "/api/answer") => self.with_target(req, |t| self.answer(t)),
            ("POST", "/api/choose") => match serde_json::from_slice::<ChooseTarget>(&req.body) {
                Ok(t) if !t.session_id.is_empty() && !t.ask.is_empty() => self.choose(&t),
                _ => error(400, self.w.need_choose),
            },
            _ => error(404, "not found"),
        }
    }

    /// The Home page with this start's token and boot, and the language: `<html lang>` and the
    /// words of the script as a JSON object.
    fn page(&self) -> String {
        let table: serde_json::Map<String, Value> = self.w.page.iter().map(|(k, v)| (k.to_string(), Value::from(*v))).collect();
        // The JSON sits in a script element, which a `</script>` inside would end.
        let table = Value::Object(table).to_string().replace('<', "\\u003c");
        PAGE.replace("__TOKEN__", &self.token)
            .replace("__BOOT__", &self.boot)
            .replace("__LANG__", self.lang.code())
            .replace("__WORDS__", &table)
    }

    fn with_target(&self, req: &Request, f: impl FnOnce(&Target) -> Response) -> Response {
        match serde_json::from_slice::<Target>(&req.body) {
            Ok(t) if !t.session_id.is_empty() && !t.resolves.is_empty() => f(&t),
            _ => error(400, self.w.need_target),
        }
    }

    fn sessions(&self) -> Response {
        match read::load(&self.events) {
            Ok(loaded) => {
                let rows: Vec<Value> = state::visible(state::derive(&loaded.events), now_us()).iter().map(|s| row(s, self.lang)).collect();
                json_response(200, json!({ "ok": true, "sessions": rows, "skipped": loaded.skipped, "boot": self.boot, "lang": self.lang.code() }))
            }
            Err(e) => error(500, &e),
        }
    }

    /// Serves a page only if a stop line recorded it and the request names it exactly.
    fn report_page(&self, raw_target: &str) -> Response {
        let loaded = match read::load(&self.events) {
            Ok(l) => l,
            Err(e) => return error(500, &e),
        };
        let recorded = loaded.events.iter().filter(|e| e.kind == Some(Kind::Stop)).find_map(|e| {
            let html = e.report_html.as_deref()?;
            (report_link(html).as_deref() == Some(raw_target)).then_some(html)
        });
        match recorded.map(read_page) {
            Some(Ok(body)) => Response { status: 200, content_type: "text/html; charset=utf-8", csp: REPORT_CSP, body },
            _ => error(404, "not found"),
        }
    }

    /// The report JSON of the session's current stop, for the panel on the Home page. The
    /// request names a session, never a path; the path comes from the stop line.
    fn report_json(&self, raw_target: &str) -> Response {
        let id = raw_target
            .split_once('?')
            .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("session_id=")))
            .filter(|id| !id.is_empty() && id.len() <= 100 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
        let Some(id) = id else {
            return error(400, self.w.need_session);
        };
        let session = match self.find(id) {
            Ok(s) => s,
            Err(r) => return r,
        };
        // The page says which stop it shows; a newer stop has another report.
        let stop = raw_target.split_once('?').and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("stop=")));
        if stop.is_some_and(|s| session.last_stop.as_deref().map(percent_encode).as_deref() != Some(s)) {
            return error(409, self.w.newer_stop_than_shown);
        }
        let report = session
            .report_path
            .as_deref()
            .filter(|p| p.starts_with('/'))
            .and_then(|p| read_page(p).ok())
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .filter(Value::is_object);
        match report {
            Some(report) => json_response(200, json!({ "ok": true, "report": report })),
            None => error(404, self.w.no_report_file),
        }
    }

    fn find(&self, session_id: &str) -> Result<Session, Response> {
        let loaded = read::load(&self.events).map_err(|e| error(500, &e))?;
        state::derive(&loaded.events)
            .into_iter()
            .find(|s| s.session_id == session_id)
            .ok_or_else(|| error(404, self.w.no_session))
    }

    fn resolve(&self, t: &Target) -> Response {
        let session = match self.find(&t.session_id) {
            Ok(s) => s,
            Err(r) => return r,
        };
        if session.st == St::Background {
            return error(409, self.w.background);
        }
        let line = events::resolution(&t.session_id, &session.cwd, "resolved", &t.resolves);
        match events::append(&self.events, line, KILL_LOCK_TIMEOUT) {
            Ok(_) => json_response(200, json!({ "ok": true })),
            Err(e) => error(500, &e.to_string()),
        }
    }

    /// What both kill and answer need: the session is stopped at the stop the human saw, the
    /// transcript shows no work since, and the recorded pid is still that agent. Returns the pid.
    fn check_stopped(&self, session: &Session, t: &Target) -> Result<i64, Response> {
        if session.st == St::Background {
            return Err(error(409, self.w.background));
        }
        if session.st != St::Stopped || session.last_stop.as_deref() != Some(t.resolves.as_str()) {
            return Err(error(409, self.w.newer_event));
        }
        let (Some(pid), Some(started)) = (session.pid, session.pid_started) else {
            return Err(error(409, self.w.no_pid));
        };
        // The log can miss a prompt line (a hook that timed out), so check the transcript too.
        let Some(transcript) = session.transcript_path.as_deref().filter(|p| !p.is_empty()) else {
            return Err(error(409, self.w.no_transcript));
        };
        let stop_us = session.last_stop_us.unwrap_or(i64::MAX);
        match activity::worked_since(Path::new(transcript), stop_us) {
            Ok(false) => {}
            Ok(true) => return Err(error(409, self.w.worked_since)),
            Err(e) => return Err(error(409, &format!("{}: {e}", self.w.transcript_unreadable))),
        }
        if !procinfo::is_same_claude(pid, started) {
            return Err(error(409, self.w.other_process));
        }
        Ok(pid)
    }

    /// Checks and signals while holding the append lock, so that no hook can append a prompt
    /// between the check and the signal.
    fn kill(&self, t: &Target) -> Response {
        let mut lock = match LogLock::acquire(&self.events, KILL_LOCK_TIMEOUT) {
            Ok(l) => l,
            Err(e) => return error(500, &e.to_string()),
        };
        let session = match self.find(&t.session_id) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let pid = match self.check_stopped(&session, t) {
            Ok(pid) => pid,
            Err(r) => return r,
        };
        // SAFETY: kill(2) with a pid checked above to be > 1.
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
            let err = io::Error::last_os_error();
            // ESRCH: claude ended after the check. It is gone, which is what the human asked for.
            if err.raw_os_error() != Some(libc::ESRCH) {
                return error(500, &format!("{}: {err}", self.w.cannot_sigterm));
            }
        }
        match lock.append(events::resolution(&t.session_id, &session.cwd, "kill", &t.resolves)) {
            Ok(_) => json_response(200, json!({ "ok": true })),
            Err(e) => error(500, &e.to_string()),
        }
    }

    /// Brings the session's terminal to the front. Nothing is recorded.
    fn focus(&self, session_id: &str) -> Response {
        let session = match self.find(session_id) {
            Ok(s) => s,
            Err(r) => return r,
        };
        let Some(commands) = term::focus_commands(&session) else {
            return error(409, self.w.cannot_focus);
        };
        if let Err(r) = self.check_occupant(&session) {
            return r;
        }
        let mut commands = commands;
        // The multiplexer switched the pane inside its window; bring that window forward too.
        commands.extend(self.host_app.as_deref().map(term::raise_command));
        self.run_all(&commands)
    }

    /// Types the recommended options into the session's terminal. The check, the answer line
    /// and the typing all happen under the append lock, so the agent cannot start a new turn
    /// in between, and a second press finds the stop already answered.
    fn answer(&self, t: &Target) -> Response {
        let mut lock = match LogLock::acquire(&self.events, KILL_LOCK_TIMEOUT) {
            Ok(l) => l,
            Err(e) => return error(500, &e.to_string()),
        };
        let session = match self.find(&t.session_id) {
            Ok(s) => s,
            Err(r) => return r,
        };
        if let Err(r) = self.check_stopped(&session, t) {
            return r;
        }
        let Some(text) = term::answer_text(&session, self.lang) else {
            return error(409, self.w.cannot_answer_text);
        };
        let Some(commands) = term::answer_commands(&session, &text) else {
            return error(409, self.w.cannot_answer_term);
        };
        if let Err(r) = self.check_occupant(&session) {
            return r;
        }
        if let Err(e) = lock.append(events::resolution(&t.session_id, &session.cwd, "answer", &t.resolves)) {
            return error(500, &e.to_string());
        }
        self.run_all(&commands)
    }

    /// Presses the digit key of an option in the session's question prompt. As with answer, the
    /// checks, the choose line and the key press all happen under the append lock, and the line
    /// comes first, so a second press finds the question already chosen.
    fn choose(&self, t: &ChooseTarget) -> Response {
        let mut lock = match LogLock::acquire(&self.events, KILL_LOCK_TIMEOUT) {
            Ok(l) => l,
            Err(e) => return error(500, &e.to_string()),
        };
        let session = match self.find(&t.session_id) {
            Ok(s) => s,
            Err(r) => return r,
        };
        // A choose already accepted for this ask has moved the session to Working.
        if session.st != St::Asking || session.ask_ts.as_deref() != Some(t.ask.as_str()) {
            return error(409, self.w.newer_than_question);
        }
        if !term::choosable(&session) {
            return error(409, self.w.cannot_choose);
        }
        let question = &session.ask[0];
        if t.option >= question.options.len() {
            return error(409, self.w.no_option);
        }
        let (Some(get), Some(keys)) = (term::occupant_command(&session), term::choose_command(&session, t.option)) else {
            return error(409, self.w.cannot_choose);
        };
        match self.run_one(&get) {
            Ok(out) if !term::occupant_is(&out, &session) => return error(409, self.w.pane_taken),
            Ok(out) if !term::question_open(&out, &question.question) => {
                return error(409, self.w.question_not_open)
            }
            Ok(_) => {}
            Err(e) => return error(409, &format!("{}: {e}", self.w.pane_lost)),
        }
        if let Err(e) = lock.append(events::choice(&t.session_id, &session.cwd, &t.ask, t.option)) {
            return error(500, &e.to_string());
        }
        self.run_all(&[keys])
    }

    /// Where a pane can come to hold another agent (herdr), asks the terminal which session the pane holds now.
    fn check_occupant(&self, session: &Session) -> Result<(), Response> {
        let Some(argv) = term::occupant_command(session) else {
            return Ok(());
        };
        match self.run_one(&argv) {
            Ok(out) if term::occupant_is(&out, session) => Ok(()),
            Ok(_) => Err(error(409, self.w.pane_taken)),
            Err(e) => Err(error(409, &format!("{}: {e}", self.w.pane_lost))),
        }
    }

    fn run_one(&self, argv: &[String]) -> Result<String, String> {
        match self.runner {
            Some(runner) => runner(argv),
            None => term::run(argv, self.lang),
        }
    }

    fn run_all(&self, commands: &[Vec<String>]) -> Response {
        for argv in commands {
            if let Err(e) = self.run_one(argv) {
                return error(502, &format!("{}: {e}", self.w.command_failed));
            }
        }
        json_response(200, json!({ "ok": true }))
    }
}

/// `decidle serve`. Stops with an error when the port is taken. `lang` is `language` in the config.
pub fn serve(port: u16, clean: super::clean::Settings, lang: Lang) -> i32 {
    let w = words(lang);
    match Server::bind(port, events::events_path(), lang) {
        Ok(mut server) => {
            server.set_host_app(term::host_app(|k| std::env::var(k).ok()));
            // Without the file the mod cannot POST; the browser Home still works.
            if let Err(e) = server.write_token(&token_path()) {
                eprintln!("decidle serve: {}: {e}", w.cannot_write_token.replace("{path}", &token_path().display().to_string()));
            }
            eprintln!("decidle serve: http://127.0.0.1:{}/", server.port());
            // Old reports: now, then once a day (see clean.rs).
            super::clean::spawn_daily(events::events_path(), clean);
            server.run();
            0
        }
        Err(e) => {
            eprintln!("decidle serve: {}: {e}", w.cannot_listen.replace("{port}", &port.to_string()));
            1
        }
    }
}
