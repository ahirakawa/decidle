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

const PAGE: &str = include_str!("page.html");
const TOKEN_HEADER: &str = "x-decidle-token";
const MAX_HEAD_BYTES: u64 = 16 * 1024;
const MAX_BODY_BYTES: usize = 4096;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// The server holds the lock only for one read of the log, so it may wait longer than a hook.
const KILL_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Server {
    listener: TcpListener,
    port: u16,
    token: String,
    events: PathBuf,
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
/// Neither resolve nor kill applies to a session that waits on background work.
const BACKGROUND_REFUSAL: &str = "裏で作業中なので、解決と終了は受け付けない";
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

fn read_page(path: &str) -> io::Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
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
        St::Background => "background",
        _ => "working",
    }
}

fn row(s: &Session) -> Value {
    let folder = s.cwd.trim_end_matches('/').rsplit('/').next().unwrap_or_default();
    json!({
        "session_id": s.session_id,
        "folder": folder,
        "cwd": s.cwd,
        "state": state_name(s),
        "started": s.started,
        "last_stop": s.last_stop,
        "report_title": s.report_title,
        "report_goal": s.report_goal,
        "questions": s.questions,
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

fn read_request(stream: &mut TcpStream) -> io::Result<Request> {
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
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "header が途中で切れた"));
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
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body が大きすぎる"));
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
    pub fn bind(port: u16, events: PathBuf) -> io::Result<Server> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        Ok(Server { listener, port, token: random_token()?, events })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Serves one connection at a time, forever.
    pub fn run(self) {
        for stream in self.listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
            let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
            let response = match read_request(&mut stream) {
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
                body: PAGE.replace("__TOKEN__", &self.token),
            },
            ("GET", path) if path.starts_with("/report/") => self.report_page(&req.target),
            ("GET", "/api/sessions") => self.sessions(),
            ("POST", "/api/resolve") => self.with_target(req, |t| self.resolve(t)),
            ("POST", "/api/kill") => self.with_target(req, |t| self.kill(t)),
            _ => error(404, "not found"),
        }
    }

    fn with_target(&self, req: &Request, f: impl FnOnce(&Target) -> Response) -> Response {
        match serde_json::from_slice::<Target>(&req.body) {
            Ok(t) if !t.session_id.is_empty() && !t.resolves.is_empty() => f(&t),
            _ => error(400, "session_id と resolves が要る"),
        }
    }

    fn sessions(&self) -> Response {
        match read::load(&self.events) {
            Ok(loaded) => {
                let rows: Vec<Value> = state::visible(state::derive(&loaded.events), now_us()).iter().map(row).collect();
                json_response(200, json!({ "ok": true, "sessions": rows, "skipped": loaded.skipped }))
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

    fn find(&self, session_id: &str) -> Result<Session, Response> {
        let loaded = read::load(&self.events).map_err(|e| error(500, &e))?;
        state::derive(&loaded.events)
            .into_iter()
            .find(|s| s.session_id == session_id)
            .ok_or_else(|| error(404, "そのセッションの記録がない"))
    }

    fn resolve(&self, t: &Target) -> Response {
        let session = match self.find(&t.session_id) {
            Ok(s) => s,
            Err(r) => return r,
        };
        if session.st == St::Background {
            return error(409, BACKGROUND_REFUSAL);
        }
        let line = events::resolution(&t.session_id, &session.cwd, "resolved", &t.resolves);
        match events::append(&self.events, line, KILL_LOCK_TIMEOUT) {
            Ok(_) => json_response(200, json!({ "ok": true })),
            Err(e) => error(500, &e.to_string()),
        }
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
        if session.st == St::Background {
            return error(409, BACKGROUND_REFUSAL);
        }
        if session.st != St::Stopped || session.last_stop.as_deref() != Some(t.resolves.as_str()) {
            return error(409, "見ていた停止のあとに新しい出来事がある");
        }
        let (Some(pid), Some(started)) = (session.pid, session.pid_started) else {
            return error(409, "claude のプロセス番号の記録がない");
        };
        // The log can miss a prompt line (a hook that timed out), so check the transcript too.
        let Some(transcript) = session.transcript_path.as_deref().filter(|p| !p.is_empty()) else {
            return error(409, "会話記録の場所の記録がないので、止まっているか確かめられない");
        };
        let stop_us = session.last_stop_us.unwrap_or(i64::MAX);
        match activity::worked_since(Path::new(transcript), stop_us) {
            Ok(false) => {}
            Ok(true) => return error(409, "会話記録では、停止のあとに作業が進んでいる"),
            Err(e) => return error(409, &format!("会話記録を読めないので、止まっているか確かめられない: {e}")),
        }
        if !procinfo::is_same_claude(pid, started) {
            return error(409, "記録したプロセス番号は、記録した起動時刻の claude ではない");
        }
        // SAFETY: kill(2) with a pid checked above to be > 1.
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } != 0 {
            let err = io::Error::last_os_error();
            // ESRCH: claude ended after the check. It is gone, which is what the human asked for.
            if err.raw_os_error() != Some(libc::ESRCH) {
                return error(500, &format!("SIGTERM を送れない: {err}"));
            }
        }
        match lock.append(events::resolution(&t.session_id, &session.cwd, "kill", &t.resolves)) {
            Ok(_) => json_response(200, json!({ "ok": true })),
            Err(e) => error(500, &e.to_string()),
        }
    }
}

/// `decidle serve`. Stops with an error when the port is taken.
pub fn serve(port: u16) -> i32 {
    match Server::bind(port, events::events_path()) {
        Ok(server) => {
            eprintln!("decidle serve: http://127.0.0.1:{}/", server.port());
            server.run();
            0
        }
        Err(e) => {
            eprintln!("decidle serve: 127.0.0.1:{port} で待ち受けられない: {e}");
            1
        }
    }
}
