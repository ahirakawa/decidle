//! The Home server: /api/sessions on a prepared events.jsonl, the 403 checks, resolve and kill.

use decidle::home::{events, procinfo, server::{self, Server}};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("decidle-home-api-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    // The recorded report paths are real paths (macOS puts the temp folder behind /var -> /private/var).
    fs::canonicalize(dir).unwrap()
}

fn ts_ago(secs: u64) -> String {
    events::rfc3339_at(SystemTime::now() - Duration::from_secs(secs))
}

/// Starts a server on a free port and returns the port and the token from the page.
fn start(log: &PathBuf) -> (u16, String) {
    let server = Server::bind(0, log.clone()).unwrap();
    let port = server.port();
    std::thread::spawn(move || server.run());
    let (status, page) = request(port, "GET", "/", &[], "");
    assert_eq!(status, 200);
    let marker = "name=\"decidle-token\" content=\"";
    let start = page.find(marker).unwrap() + marker.len();
    let token = page[start..start + 64].to_string();
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()), "token {token}");
    (port, token)
}

/// Sends one request. `Host` is set to 127.0.0.1:port unless given in `headers`.
fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nContent-Length: {}\r\n", body.len());
    if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
        head.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = out.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn sessions(port: u16) -> Value {
    let (status, body) = request(port, "GET", "/api/sessions", &[], "");
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

fn rows(port: u16) -> Vec<(String, String)> {
    sessions(port)["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["session_id"].as_str().unwrap().to_string(), r["state"].as_str().unwrap().to_string()))
        .collect()
}

fn post(port: u16, path: &str, token: &str, body: &Value) -> (u16, String) {
    let origin = format!("http://127.0.0.1:{port}");
    request(port, "POST", path, &[("Origin", &origin), ("X-Decidle-Token", token)], &body.to_string())
}

/// One line per tuple: (session, event, extra fields). Each line is one second after the last.
fn write_log(path: &PathBuf, lines: &[(&str, &str, Value)], base: SystemTime) -> Vec<String> {
    let mut text = String::new();
    let mut ts_list = Vec::new();
    for (k, (sid, event, extra)) in lines.iter().enumerate() {
        let ts = events::rfc3339_at(base + Duration::from_secs(k as u64));
        let mut line = json!({"ts": ts, "session_id": sid, "cwd": format!("/work/{sid}"), "event": event});
        if let (Some(m), Some(x)) = (line.as_object_mut(), extra.as_object()) {
            m.extend(x.clone());
        }
        text.push_str(&format!("{line}\n"));
        ts_list.push(ts);
    }
    fs::write(path, text).unwrap();
    ts_list
}

fn append_raw(path: &PathBuf, text: &str) {
    fs::OpenOptions::new().append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
}

#[test]
fn sessions_states_and_order() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let n = json!({});
    let mut lines: Vec<(&str, &str, Value)> = vec![
        ("A", "start", n.clone()),
        ("A", "prompt", n.clone()),
        ("B", "start", n.clone()),
        ("B", "prompt", n.clone()),
        ("B", "permission", n.clone()),
        ("C", "prompt", n.clone()),
        ("C", "stop", n.clone()),
        ("D", "prompt", n.clone()),
        ("D", "stop", json!({"report_path": "/work/D/.decidle/t.report.json", "report_html": "/work/D/.decidle/t.html", "report_title": "題D", "decisions": 2})),
        ("E", "prompt", n.clone()),
        ("E", "stop", n.clone()), // index 10; resolved below
        ("F", "prompt", n.clone()),
        ("F", "stop", n.clone()), // index 12; the resolved for it comes after a newer stop
        ("F", "prompt", n.clone()),
        ("F", "stop", n.clone()),
        ("G", "prompt", n.clone()),
        ("G", "end", n.clone()),
        ("I", "stop", n.clone()), // index 17
        ("J", "stop", n.clone()), // index 18
    ];
    // Placeholders for resolved and kill; their resolves are filled in after the ts are known.
    lines.push(("E", "resolved", n.clone()));
    lines.push(("F", "resolved", n.clone()));
    lines.push(("I", "resolved", n.clone()));
    lines.push(("I", "prompt", n.clone()));
    lines.push(("J", "kill", n.clone()));
    let base = SystemTime::now() - Duration::from_secs(3600);
    let ts = write_log(&log, &lines, base);
    lines[19].2 = json!({"resolves": ts[10]});
    lines[20].2 = json!({"resolves": ts[12]});
    lines[21].2 = json!({"resolves": ts[17]});
    lines[23].2 = json!({"resolves": ts[18]});
    write_log(&log, &lines, base);
    // A session that only worked 13 hours ago, a broken line, and a line cut off by a crash.
    append_raw(&log, &format!("{}\n", json!({"ts": ts_ago(13 * 3600), "session_id": "H", "cwd": "/work/H", "event": "prompt"})));
    append_raw(&log, "not json\n");
    append_raw(&log, "{\"ts\": \"2026-11-18T00:00:00.000000Z\", \"session_id\": \"K\", \"ev");

    let (port, _) = start(&log);
    let got = rows(port);
    let want: Vec<(String, String)> = [
        ("D", "decide"),
        ("B", "permission"),
        ("F", "stopped"),
        ("C", "stopped"),
        ("I", "working"),
        ("A", "working"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(got, want);

    let all = sessions(port);
    assert_eq!(all["skipped"], 2);
    let d = &all["sessions"][0];
    assert_eq!(d["report_title"], "題D");
    assert_eq!(d["decisions"], 2);
    assert_eq!(d["folder"], "D");
    assert_eq!(d["report_url"], "/report/work/D/.decidle/t.html");
}

#[test]
fn missing_log_gives_empty_table() {
    let dir = temp_dir();
    let (port, _) = start(&dir.join("events.jsonl"));
    let all = sessions(port);
    assert_eq!(all["sessions"], json!([]));
    assert_eq!(all["skipped"], 0);
    assert!(!dir.join("events.jsonl").exists());
}

#[test]
fn forged_requests_are_403() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let ts = write_log(&log, &[("C", "prompt", json!({})), ("C", "stop", json!({}))], SystemTime::now() - Duration::from_secs(60));
    let (port, token) = start(&log);
    let body = json!({"session_id": "C", "resolves": ts[1]}).to_string();
    let good_origin = format!("http://127.0.0.1:{port}");
    let local_host = format!("localhost:{port}");

    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no token", vec![("Origin", &good_origin)]),
        ("wrong token", vec![("Origin", &good_origin), ("X-Decidle-Token", "00")]),
        ("other origin", vec![("Origin", "http://evil.example"), ("X-Decidle-Token", &token)]),
        ("no origin", vec![("X-Decidle-Token", &token)]),
        ("other host", vec![("Host", "evil.example"), ("Origin", &good_origin), ("X-Decidle-Token", &token)]),
    ];
    for (name, headers) in &cases {
        for path in ["/api/resolve", "/api/kill"] {
            let (status, _) = request(port, "POST", path, headers, &body);
            assert_eq!(status, 403, "{name} {path}");
        }
    }
    let (status, _) = request(port, "GET", "/api/sessions", &[("Host", "evil.example:80")], "");
    assert_eq!(status, 403, "GET with a rebinding Host");
    // Nothing was appended by the refused requests.
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);

    // The same request with the token, from localhost, goes through and hides the row.
    let (status, out) = request(
        port,
        "POST",
        "/api/resolve",
        &[("Host", &local_host), ("Origin", &format!("http://{local_host}")), ("X-Decidle-Token", &token)],
        &body,
    );
    assert_eq!(status, 200, "{out}");
    assert!(rows(port).is_empty());
    let last: Value = serde_json::from_str(fs::read_to_string(&log).unwrap().lines().last().unwrap()).unwrap();
    assert_eq!(last["event"], "resolved");
    assert_eq!(last["resolves"], ts[1].as_str());
}

#[test]
fn taken_port_is_an_error() {
    let taken = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = taken.local_addr().unwrap().port();
    assert!(Server::bind(port, temp_dir().join("events.jsonl")).is_err());
}

/// A process whose name is claude: sleep run through a symlink named claude.
/// (A copy of /bin/sleep is killed by macOS code signing.)
fn fake_claude(dir: &PathBuf) -> Child {
    let exe = dir.join("claude");
    std::os::unix::fs::symlink("/bin/sleep", &exe).unwrap();
    Command::new(&exe).arg("60").spawn().unwrap()
}

fn exited_within(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// A transcript whose conversation entries are stamped at the given times, followed by the
/// bookkeeping entry Claude Code writes after the Stop hook (stamped now, and not conversation).
fn transcript(dir: &PathBuf, name: &str, entries: &[(&str, SystemTime)]) -> String {
    let mut text = String::new();
    for (kind, at) in entries {
        text.push_str(&format!("{}\n", json!({"type": kind, "timestamp": ts_ms(*at), "message": {"content": "x"}})));
    }
    text.push_str(&format!("{}\n", json!({"type": "system", "subtype": "stop_hook_summary", "timestamp": ts_ms(SystemTime::now())})));
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path.to_string_lossy().into_owned()
}

/// Transcript timestamps have milliseconds.
fn ts_ms(at: SystemTime) -> String {
    let full = events::rfc3339_at(at);
    format!("{}Z", &full[..full.len() - 4])
}

fn error_of(out: &str) -> String {
    serde_json::from_str::<Value>(out).unwrap()["error"].as_str().unwrap_or_default().to_string()
}

#[test]
fn kill_checks_stop_and_process_before_signalling() {
    let dir = temp_dir();
    let mut claude = fake_claude(&dir);
    let pid = claude.id() as i64;
    let started = procinfo::info(pid).unwrap().started;
    let log = dir.join("events.jsonl");
    let base = SystemTime::now() - Duration::from_secs(60);
    // Every stop below is at base + 2 s or later; the idle transcript last spoke at base.
    let idle = transcript(&dir, "idle.jsonl", &[("user", base), ("assistant", base)]);
    let resumed = transcript(&dir, "resumed.jsonl", &[("assistant", base), ("user", SystemTime::now())]);
    let rec = |started: i64, t: &str| json!({"pid": pid, "pid_started": started, "transcript_path": t});
    let ts = write_log(
        &log,
        &[
            ("ok", "start", rec(started, &idle)),
            ("ok", "prompt", json!({})),
            ("ok", "stop", json!({})), // 2
            ("reused", "start", rec(started + 1, &idle)),
            ("reused", "stop", json!({})), // 4
            ("busy", "start", rec(started, &idle)),
            ("busy", "stop", json!({})), // 6
            ("busy", "prompt", json!({})),
            // The prompt line was lost, but the transcript shows claude working again.
            ("lost", "start", rec(started, &resumed)),
            ("lost", "stop", json!({})), // 9
            ("untracked", "start", json!({"pid": pid, "pid_started": started})),
            ("untracked", "stop", json!({})), // 11
            ("gone", "start", rec(started, &dir.join("missing.jsonl").to_string_lossy())),
            ("gone", "stop", json!({})), // 13
        ],
        base,
    );
    let (port, token) = start(&log);
    let refused = |sid: &str, i: usize, why: &str| {
        let (status, out) = post(port, "/api/kill", &token, &json!({"session_id": sid, "resolves": ts[i]}));
        assert_eq!(status, 409, "{sid}: {out}");
        assert!(error_of(&out).contains(why), "{sid}: {out}");
    };

    refused("reused", 4, "記録した起動時刻の claude ではない");
    refused("busy", 6, "見ていた停止のあとに新しい出来事がある");
    refused("ok", 4, "見ていた停止のあとに新しい出来事がある");
    refused("lost", 9, "停止のあとに作業が進んでいる");
    refused("untracked", 11, "会話記録の場所の記録がない");
    refused("gone", 13, "会話記録を読めない");
    assert!(!exited_within(&mut claude, Duration::from_millis(300)), "signalled despite a failed check");
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 14);

    let (status, out) = post(port, "/api/kill", &token, &json!({"session_id": "ok", "resolves": ts[2]}));
    assert_eq!(status, 200, "{out}");
    assert!(exited_within(&mut claude, Duration::from_secs(5)), "claude did not exit");
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(claude.wait().unwrap().signal(), Some(libc::SIGTERM));
    let last: Value = serde_json::from_str(fs::read_to_string(&log).unwrap().lines().last().unwrap()).unwrap();
    assert_eq!(last["event"], "kill");
    assert_eq!(last["resolves"], ts[2].as_str());
    assert!(!rows(port).iter().any(|(s, _)| s == "ok"));

    // The session ended, so a second kill on the same stop is refused.
    refused("ok", 2, "見ていた停止のあとに新しい出来事がある");
}

#[test]
fn kill_refuses_a_process_that_is_not_named_claude() {
    let dir = temp_dir();
    // Same pid and start time as recorded, but the program is sleep.
    let mut other = Command::new("/bin/sleep").arg("60").spawn().unwrap();
    let pid = other.id() as i64;
    let started = procinfo::info(pid).unwrap().started;
    let base = SystemTime::now() - Duration::from_secs(60);
    let idle = transcript(&dir, "idle.jsonl", &[("assistant", base)]);
    let log = dir.join("events.jsonl");
    let ts = write_log(
        &log,
        &[("s", "start", json!({"pid": pid, "pid_started": started, "transcript_path": idle})), ("s", "stop", json!({}))],
        base,
    );
    let (port, token) = start(&log);
    let (status, out) = post(port, "/api/kill", &token, &json!({"session_id": "s", "resolves": ts[1]}));
    assert_eq!(status, 409, "{out}");
    assert!(error_of(&out).contains("claude ではない"), "{out}");
    assert!(!exited_within(&mut other, Duration::from_millis(300)));
    let _ = other.kill();
}

#[test]
fn kill_waits_while_another_process_holds_the_log_lock() {
    let dir = temp_dir();
    let mut claude = fake_claude(&dir);
    let pid = claude.id() as i64;
    let started = procinfo::info(pid).unwrap().started;
    let base = SystemTime::now() - Duration::from_secs(60);
    let idle = transcript(&dir, "idle.jsonl", &[("assistant", base)]);
    let log = dir.join("events.jsonl");
    let ts = write_log(
        &log,
        &[("s", "start", json!({"pid": pid, "pid_started": started, "transcript_path": idle})), ("s", "stop", json!({}))],
        base,
    );
    let (port, token) = start(&log);

    // Another process takes the same flock as the hooks, says so, and holds it until told to stop.
    let mut holder = Command::new("perl")
        .args(["-e", "use Fcntl qw(:flock); open(my $f, '>>', $ARGV[0]) or die; flock($f, LOCK_EX) or die; $| = 1; print \"locked\\n\"; <STDIN>; print $f \"\";"])
        .arg(&log)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = holder.stdout.take().unwrap();
    let mut first = [0u8; 7];
    out.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"locked\n");

    let body = json!({"session_id": "s", "resolves": ts[1]});
    let request = std::thread::spawn(move || post(port, "/api/kill", &token, &body));
    // While the lock is held, the server neither signals nor appends.
    assert!(!exited_within(&mut claude, Duration::from_millis(1500)), "signalled while another process held the lock");
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
    assert!(!request.is_finished());

    drop(holder.stdin.take()); // perl reads EOF, exits and releases the lock
    holder.wait().unwrap();
    let (status, out) = request.join().unwrap();
    assert_eq!(status, 200, "{out}");
    assert!(exited_within(&mut claude, Duration::from_secs(5)));
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 3);
}

#[test]
fn duplicated_security_headers_are_403() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let ts = write_log(&log, &[("C", "prompt", json!({})), ("C", "stop", json!({}))], SystemTime::now() - Duration::from_secs(60));
    let (port, token) = start(&log);
    let body = json!({"session_id": "C", "resolves": ts[1]}).to_string();
    let host = format!("127.0.0.1:{port}");
    let origin = format!("http://{host}");
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("host twice", vec![("Host", &host), ("Host", &host), ("Origin", &origin), ("X-Decidle-Token", &token)]),
        ("origin twice", vec![("Origin", "http://evil.example"), ("Origin", &origin), ("X-Decidle-Token", &token)]),
        ("token twice", vec![("Origin", &origin), ("X-Decidle-Token", "00"), ("x-decidle-token", &token)]),
    ];
    for (name, headers) in &cases {
        let (status, _) = request(port, "POST", "/api/resolve", headers, &body);
        assert_eq!(status, 403, "{name}");
    }
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 2);
}

#[test]
fn only_recorded_report_pages_are_served() {
    let dir = temp_dir();
    // A folder name that needs encoding, to check that the link round-trips through the server.
    let reports = dir.join("報告 dir/.decidle");
    fs::create_dir_all(reports.join("sub")).unwrap();
    fs::write(reports.join("t.html"), "<p>recorded</p>").unwrap();
    fs::write(reports.join("other.html"), "<p>not recorded</p>").unwrap();
    let html = reports.join("t.html").to_string_lossy().into_owned();
    let log = dir.join("events.jsonl");
    write_log(
        &log,
        &[("s", "prompt", json!({})), ("s", "stop", json!({"report_path": "x", "report_html": html, "report_title": "題", "decisions": 0}))],
        SystemTime::now() - Duration::from_secs(60),
    );
    let (port, _) = start(&log);
    let link = sessions(port)["sessions"][0]["report_url"].as_str().unwrap().to_string();
    assert_eq!(Some(link.clone()), server::report_link(&html));
    let (status, body) = request(port, "GET", &link, &[], "");
    assert_eq!((status, body.as_str()), (200, "<p>recorded</p>"));

    let folder = link.strip_suffix("/t.html").unwrap();
    let refused = [
        format!("{folder}/other.html"),
        format!("{folder}/sub/../t.html"),
        format!("{folder}/sub/%2E%2E/t.html"),
        format!("{folder}/sub/%2e%2e%2ft.html"),
        format!("{folder}/%74.html"),
        format!("{link}x"),
        format!("{link}/"),
        format!("{link}%00"),
        format!("{link}?x"),
        "/report/etc/passwd".to_string(),
        "/report/".to_string(),
    ];
    for target in &refused {
        let (status, body) = request(port, "GET", target, &[], "");
        assert_eq!(status, 404, "{target}: {body}");
    }
}

#[test]
fn background_stop_is_shown_as_working_and_refuses_resolve_and_kill() {
    let dir = temp_dir();
    let mut claude = fake_claude(&dir);
    let pid = claude.id() as i64;
    let started = procinfo::info(pid).unwrap().started;
    let log = dir.join("events.jsonl");
    let base = SystemTime::now() - Duration::from_secs(60);
    // Every other kill check would pass: the pid is this claude and the transcript is idle.
    let idle = transcript(&dir, "idle.jsonl", &[("user", base), ("assistant", base)]);
    let mut lines: Vec<(&str, &str, Value)> = vec![
        ("bg", "start", json!({"pid": pid, "pid_started": started, "transcript_path": idle})),
        ("bg", "prompt", json!({})),
        ("bg", "stop", json!({"background": true, "report_title": "題", "decisions": 1})), // 2
        ("st", "prompt", json!({})),
        ("st", "stop", json!({})),
        ("wk", "prompt", json!({})),
    ];
    let ts = write_log(&log, &lines, base);
    let (port, token) = start(&log);
    // Shown with the working rows: below the stopped row, and newer than wk.
    let want: Vec<(String, String)> =
        [("st", "stopped"), ("wk", "working"), ("bg", "background")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
    assert_eq!(rows(port), want);

    for path in ["/api/resolve", "/api/kill"] {
        let (status, out) = post(port, path, &token, &json!({"session_id": "bg", "resolves": ts[2]}));
        assert_eq!(status, 409, "{path}: {out}");
        assert!(error_of(&out).contains("裏で作業中"), "{path}: {out}");
    }
    assert!(!exited_within(&mut claude, Duration::from_millis(300)), "signalled a session waiting on background work");
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), lines.len(), "a refused request appended");

    // The background work came back and the next turn ended with an ordinary stop.
    lines.push(("bg", "stop", json!({})));
    let ts = write_log(&log, &lines, base);
    assert!(rows(port).contains(&("bg".to_string(), "stopped".to_string())));
    let (status, out) = post(port, "/api/resolve", &token, &json!({"session_id": "bg", "resolves": ts[6]}));
    assert_eq!(status, 200, "{out}");
    assert!(!rows(port).iter().any(|(s, _)| s == "bg"));
    let _ = claude.kill();
}

#[test]
fn stops_are_split_by_whether_the_human_has_something_to_do() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let lines: Vec<(&str, &str, Value)> = vec![
        ("work", "prompt", json!({})),
        ("done", "stop", json!({"attention": "no", "note": "テストの完了を報告"})),
        ("old", "stop", json!({})), // written before the Home line existed
        ("need", "stop", json!({"attention": "yes", "note": "レビューを待っている"})),
        ("odd", "stop", json!({"attention": "maybe"})),
    ];
    write_log(&log, &lines, SystemTime::now() - Duration::from_secs(60));
    let (port, _) = start(&log);
    let all = sessions(port);
    let got: Vec<(String, String, bool, Value)> = all["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (r["session_id"].as_str().unwrap().into(), r["attention"].as_str().unwrap().into(), r["waits_for_human"].as_bool().unwrap(), r["note"].clone())
        })
        .collect();
    let want: Vec<(String, String, bool, Value)> = vec![
        ("need".into(), "yes".into(), true, json!("レビューを待っている")),
        ("odd".into(), "unknown".into(), true, Value::Null),
        ("old".into(), "unknown".into(), true, Value::Null),
        ("done".into(), "no".into(), false, json!("テストの完了を報告")),
        ("work".into(), "unknown".into(), false, Value::Null),
    ];
    assert_eq!(got, want);
}

#[test]
fn rows_carry_questions_goal_and_times() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let n = json!({});
    let report = json!({
        "report_path": "/work/Q/.decidle/t.report.json", "report_html": "/work/Q/.decidle/t.html",
        "report_title": "題Q", "decisions": 2,
        "questions": ["終了ボタンの確認を先にするか", "ポートを 7878 のままにするか"],
        "report_goal": "Home で問いを読めるようにしたい",
    });
    let lines: Vec<(&str, &str, Value)> = vec![
        ("Q", "start", n.clone()),
        ("Q", "prompt", n.clone()),
        ("Q", "stop", report),
        // Written before questions were recorded, and without a start line.
        ("O", "prompt", n.clone()),
        ("O", "stop", json!({"report_title": "題O", "decisions": 1, "questions": "not an array"})),
        // A newer start clears what the previous stop carried.
        ("R", "start", n.clone()),
        ("R", "prompt", n.clone()),
        ("R", "stop", json!({"report_title": "題R", "decisions": 1, "questions": ["前の問い"], "report_goal": "前の目的"})),
        ("R", "start", n.clone()),
        ("R", "prompt", n.clone()),
    ];
    let ts = write_log(&log, &lines, SystemTime::now() - Duration::from_secs(600));
    let (port, _) = start(&log);
    let all = sessions(port);
    let by = |sid: &str| all["sessions"].as_array().unwrap().iter().find(|r| r["session_id"] == sid).unwrap().clone();

    let q = by("Q");
    assert_eq!(q["questions"], json!(["終了ボタンの確認を先にするか", "ポートを 7878 のままにするか"]));
    assert_eq!(q["report_goal"], "Home で問いを読めるようにしたい");
    assert_eq!(q["started"], json!(ts[0]));
    assert_eq!(q["last_stop"], json!(ts[2]));

    let o = by("O");
    assert_eq!(o["questions"], json!([]));
    assert_eq!(o["report_goal"], Value::Null);
    assert_eq!(o["started"], Value::Null);
    assert_eq!(o["decisions"], 1);

    let r = by("R");
    assert_eq!(r["state"], "working");
    assert_eq!(r["questions"], json!([]));
    assert_eq!(r["report_goal"], Value::Null);
    assert_eq!(r["started"], json!(ts[8]));
}

static CALLS: std::sync::Mutex<Vec<Vec<String>>> = std::sync::Mutex::new(Vec::new());

fn record_call(argv: &[String]) -> Result<String, String> {
    CALLS.lock().unwrap().push(argv.to_vec());
    Ok(String::new())
}

fn refuse_call(_: &[String]) -> Result<String, String> {
    Err("cmux: アクセスが拒否されました".into())
}

/// Like `start`, with the terminal commands replaced.
fn start_with(log: &PathBuf, runner: fn(&[String]) -> Result<String, String>) -> (u16, String) {
    let mut server = Server::bind(0, log.clone()).unwrap();
    server.set_runner(runner);
    let port = server.port();
    std::thread::spawn(move || server.run());
    let (_, page) = request(port, "GET", "/", &[], "");
    let marker = "name=\"decidle-token\" content=\"";
    let at = page.find(marker).unwrap() + marker.len();
    (port, page[at..at + 64].to_string())
}

fn count_of(log: &PathBuf, event: &str) -> usize {
    fs::read_to_string(log).unwrap().lines().filter(|l| l.contains(&format!("\"event\":\"{event}\""))).count()
}

#[test]
fn answer_types_the_recommended_options_once_and_only_into_a_stopped_session() {
    let dir = temp_dir();
    let mut claude = fake_claude(&dir);
    let pid = claude.id() as i64;
    let started = procinfo::info(pid).unwrap().started;
    let log = dir.join("events.jsonl");
    let base = SystemTime::now() - Duration::from_secs(60);
    let idle = transcript(&dir, "idle.jsonl", &[("user", base), ("assistant", base)]);
    let rec = |term: Value| {
        let mut v = json!({"pid": pid, "pid_started": started, "transcript_path": idle});
        v.as_object_mut().unwrap().extend(term.as_object().unwrap().clone());
        v
    };
    let cmux = json!({"term": "cmux", "term_id": "11111111-2222-3333-4444-555555555555", "term_ws": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"});
    let asked = |q: &str, r: &str| json!({"report_title": "題", "decisions": 1, "questions": [q], "recommends": [r]});
    let ts = write_log(
        &log,
        &[
            ("ok", "start", rec(cmux.clone())),
            ("ok", "stop", asked("古い問い", "古い勧め")), // 1
            ("ok", "prompt", json!({})),
            ("ok", "stop", asked("保存先をどちらにするか?", "ローカル")), // 3
            ("bg", "start", rec(cmux.clone())),
            ("bg", "stop", {
                let mut v = asked("問い", "勧め");
                v["background"] = json!(true);
                v
            }), // 5
            ("perm", "start", rec(cmux.clone())),
            ("perm", "stop", asked("問い", "勧め")), // 7
            ("perm", "prompt", json!({})),
            ("perm", "permission", json!({})),
            ("norec", "start", rec(cmux.clone())),
            ("norec", "stop", asked("問い", "")), // 11
            ("meta", "start", rec(cmux.clone())),
            ("meta", "stop", asked("問い $(whoami)", "勧め")), // 13
            ("noterm", "start", rec(json!({}))),
            ("noterm", "stop", asked("問い", "勧め")), // 15
            ("badid", "start", rec(json!({"term": "cmux", "term_id": "a b"}))),
            ("badid", "stop", asked("問い", "勧め")), // 17
        ],
        base,
    );
    let (port, token) = start_with(&log, record_call);
    let target = |sid: &str, k: usize| json!({"session_id": sid, "resolves": ts[k]});
    let calls = || CALLS.lock().unwrap().clone();

    // The page learns what would be typed, and which rows have a terminal.
    let all = sessions(port);
    let row = |sid: &str| all["sessions"].as_array().unwrap().iter().find(|r| r["session_id"] == sid).unwrap().clone();
    assert_eq!(row("ok")["answer_text"], json!("回答: 保存先をどちらにするかは「ローカル」で進めて。"));
    assert_eq!(row("ok")["term"], json!("cmux"));
    assert_eq!(row("norec")["answer_text"], Value::Null);
    assert_eq!(row("meta")["answer_text"], Value::Null);
    assert_eq!(row("noterm")["term"], Value::Null);
    assert_eq!(row("badid")["term"], Value::Null);

    // Every refusal: nothing typed, nothing appended.
    for (sid, k) in [("ok", 1), ("bg", 5), ("perm", 7), ("norec", 11), ("meta", 13), ("noterm", 15), ("badid", 17)] {
        let (status, out) = post(port, "/api/answer", &token, &target(sid, k));
        assert_eq!(status, 409, "{sid}: {out}");
    }
    assert_eq!(post(port, "/api/focus", &token, &target("noterm", 15)).0, 409);
    assert!(calls().is_empty(), "{:?}", calls());
    assert_eq!(count_of(&log, "answer"), 0);

    // Focus brings the terminal to the front and records nothing.
    let lines_before = fs::read_to_string(&log).unwrap().lines().count();
    assert_eq!(post(port, "/api/focus", &token, &target("ok", 3)).0, 200);
    let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    assert_eq!(
        calls(),
        vec![
            strs(&["cmux", "select-workspace", "--workspace", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]),
            strs(&["cmux", "focus-panel", "--panel", "11111111-2222-3333-4444-555555555555", "--workspace", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]),
        ]
    );
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), lines_before);
    CALLS.lock().unwrap().clear();
    // Focus needs the session only: a session asking before its first stop has no stop to name.
    assert_eq!(post(port, "/api/focus", &token, &json!({"session_id": "ok", "resolves": null})).0, 200);
    assert_eq!(post(port, "/api/focus", &token, &json!({"session_id": "ok"})).0, 200);
    assert_eq!(post(port, "/api/focus", &token, &json!({"resolves": "x"})).0, 400);
    CALLS.lock().unwrap().clear();

    // Accepted: the text built from the log and Enter, in one command, once, and an answer line.
    let (status, out) = post(port, "/api/answer", &token, &target("ok", 3));
    assert_eq!(status, 200, "{out}");
    assert_eq!(
        calls(),
        vec![
            strs(&["cmux", "send", "--workspace", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "--surface", "11111111-2222-3333-4444-555555555555", "--", "回答: 保存先をどちらにするかは「ローカル」で進めて。\\n"]),
        ]
    );
    assert_eq!(count_of(&log, "answer"), 1);
    assert!(!rows(port).iter().any(|(sid, _)| sid == "ok"), "an answered row is hidden");

    // A second press on the same stop types nothing.
    assert_eq!(post(port, "/api/answer", &token, &target("ok", 3)).0, 409);
    assert_eq!(calls().len(), 1);
    assert_eq!(count_of(&log, "answer"), 1);

    // The terminal takes the text as a prompt: the row comes back as working.
    append_raw(&log, &format!("{}\n", json!({"ts": events::now_rfc3339(), "session_id": "ok", "cwd": "/work/ok", "event": "prompt"})));
    assert!(rows(port).contains(&("ok".to_string(), "working".to_string())));

    let _ = claude.kill();
    let _ = claude.wait();
}

#[test]
fn a_terminal_command_that_fails_is_reported_as_502() {
    let dir = temp_dir();
    let mut claude = fake_claude(&dir);
    let pid = claude.id() as i64;
    let started = procinfo::info(pid).unwrap().started;
    let log = dir.join("events.jsonl");
    let base = SystemTime::now() - Duration::from_secs(60);
    let idle = transcript(&dir, "idle.jsonl", &[("user", base), ("assistant", base)]);
    let ts = write_log(
        &log,
        &[
            ("s", "start", json!({"pid": pid, "pid_started": started, "transcript_path": idle, "term": "cmux", "term_id": "11111111-2222-3333-4444-555555555555", "term_ws": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"})),
            ("s", "stop", json!({"report_title": "題", "decisions": 1, "questions": ["問い"], "recommends": ["勧め"]})),
        ],
        base,
    );
    let (port, token) = start_with(&log, refuse_call);
    let target = json!({"session_id": "s", "resolves": ts[1]});
    let (status, out) = post(port, "/api/focus", &token, &target);
    assert_eq!(status, 502, "{out}");
    assert!(error_of(&out).contains("アクセスが拒否"), "{out}");
    let (status, out) = post(port, "/api/answer", &token, &target);
    assert_eq!(status, 502, "{out}");
    // The answer line was written before the command ran, so a second press is refused.
    assert_eq!(count_of(&log, "answer"), 1);
    assert_eq!(post(port, "/api/answer", &token, &target).0, 409);
    let _ = claude.kill();
    let _ = claude.wait();
}

static HERDR_CALLS: std::sync::Mutex<Vec<Vec<String>>> = std::sync::Mutex::new(Vec::new());

/// A herdr whose pane w1:p2 holds session "mine" and whose pane w1:p3 holds another session.
fn fake_herdr(argv: &[String]) -> Result<String, String> {
    HERDR_CALLS.lock().unwrap().push(argv.to_vec());
    if argv[1..3] == ["agent".to_string(), "get".to_string()] {
        let held = if argv[3] == "w1:p2" { "mine" } else { "someone-else" };
        return Ok(json!({"result": {"agent": {"agent_session": {"value": held}}}}).to_string());
    }
    Ok(String::new())
}

#[test]
fn herdr_pane_is_checked_to_still_hold_the_session_before_anything_is_typed() {
    let dir = temp_dir();
    let mut claude = fake_claude(&dir);
    let pid = claude.id() as i64;
    let started = procinfo::info(pid).unwrap().started;
    let log = dir.join("events.jsonl");
    let base = SystemTime::now() - Duration::from_secs(60);
    let idle = transcript(&dir, "idle.jsonl", &[("user", base), ("assistant", base)]);
    let rec = |pane: &str| json!({"pid": pid, "pid_started": started, "transcript_path": idle, "term": "herdr", "term_id": pane});
    let asked = json!({"report_title": "題", "decisions": 1, "questions": ["問い"], "recommends": ["推奨"]});
    let ts = write_log(
        &log,
        &[("mine", "start", rec("w1:p2")), ("mine", "stop", asked.clone()), ("moved", "start", rec("w1:p3")), ("moved", "stop", asked)],
        base,
    );
    let (port, token) = start_with(&log, fake_herdr);
    let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();

    // The pane was renumbered and now holds another session: refused, nothing typed or appended.
    let moved = json!({"session_id": "moved", "resolves": ts[3]});
    assert_eq!(post(port, "/api/answer", &token, &moved).0, 409);
    assert_eq!(post(port, "/api/focus", &token, &moved).0, 409);
    assert!(HERDR_CALLS.lock().unwrap().iter().all(|c| c[2] == "get"), "{:?}", HERDR_CALLS.lock().unwrap());
    assert_eq!(count_of(&log, "answer"), 0);
    HERDR_CALLS.lock().unwrap().clear();

    let (status, out) = post(port, "/api/answer", &token, &json!({"session_id": "mine", "resolves": ts[1]}));
    assert_eq!(status, 200, "{out}");
    assert_eq!(
        *HERDR_CALLS.lock().unwrap(),
        vec![strs(&["herdr", "agent", "get", "w1:p2"]), strs(&["herdr", "agent", "prompt", "w1:p2", "回答: 問いは「推奨」で進めて。"])]
    );
    assert_eq!(count_of(&log, "answer"), 1);
    let _ = claude.kill();
    let _ = claude.wait();
}

static HC_CALLS: std::sync::Mutex<Vec<Vec<String>>> = std::sync::Mutex::new(Vec::new());
/// The pid in front of terminal t3; t4 has a shell (pid 1) in front.
static HC_FOREGROUND: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// An hc whose `get` reports the foreground process of each terminal.
fn fake_hc(argv: &[String]) -> Result<String, String> {
    HC_CALLS.lock().unwrap().push(argv.to_vec());
    if argv[..2] == ["hc".to_string(), "get".to_string()] {
        let pid = if argv[2] == "t3" { HC_FOREGROUND.load(Ordering::SeqCst) } else { 1 };
        return Ok(json!({"address": argv[2], "foreground": {"name": "claude", "pid": pid}, "pid": 1}).to_string());
    }
    Ok(String::new())
}

#[test]
fn hc_terminal_is_checked_to_still_run_the_recorded_claude_before_anything_is_typed() {
    let dir = temp_dir();
    let mut claude = fake_claude(&dir);
    let pid = claude.id() as i64;
    HC_FOREGROUND.store(pid, Ordering::SeqCst);
    let started = procinfo::info(pid).unwrap().started;
    let log = dir.join("events.jsonl");
    let base = SystemTime::now() - Duration::from_secs(60);
    let idle = transcript(&dir, "idle.jsonl", &[("user", base), ("assistant", base)]);
    let rec = |t: &str| json!({"pid": pid, "pid_started": started, "transcript_path": idle, "term": "hc", "term_id": t});
    let asked = json!({"report_title": "題", "decisions": 1, "questions": ["問い"], "recommends": ["推奨"]});
    let ts = write_log(
        &log,
        &[
            ("mine", "start", rec("t3")),
            ("mine", "stop", asked.clone()),
            ("swapped", "start", rec("t4")),
            ("swapped", "stop", asked.clone()),
            ("remote", "start", rec("host/t1")),
            ("remote", "stop", asked),
        ],
        base,
    );
    let (port, token) = start_with(&log, fake_hc);
    let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let row = |sid: &str| sessions(port)["sessions"].as_array().unwrap().iter().find(|r| r["session_id"] == sid).unwrap().clone();
    assert_eq!(row("mine")["term"], json!("hc"));
    assert_eq!(row("remote")["term"], Value::Null);
    // The mod brings the terminal to the front by its id.
    assert_eq!(row("mine")["term_id"], json!("t3"));
    assert_eq!(row("remote")["term_id"], Value::Null);

    // Another program is in front of the terminal: refused, nothing typed or appended.
    let swapped = json!({"session_id": "swapped", "resolves": ts[3]});
    assert_eq!(post(port, "/api/answer", &token, &swapped).0, 409);
    assert_eq!(post(port, "/api/focus", &token, &swapped).0, 409);
    assert_eq!(post(port, "/api/answer", &token, &json!({"session_id": "remote", "resolves": ts[5]})).0, 409);
    assert!(HC_CALLS.lock().unwrap().iter().all(|c| c[1] == "get"), "{:?}", HC_CALLS.lock().unwrap());
    assert_eq!(count_of(&log, "answer"), 0);
    HC_CALLS.lock().unwrap().clear();

    let mine = json!({"session_id": "mine", "resolves": ts[1]});
    assert_eq!(post(port, "/api/focus", &token, &mine).0, 200);
    assert_eq!(*HC_CALLS.lock().unwrap(), vec![strs(&["hc", "get", "t3"]), strs(&["herdr-app", "do", "pane.focus", "--terminal", "t3"])]);
    HC_CALLS.lock().unwrap().clear();

    let (status, out) = post(port, "/api/answer", &token, &mine);
    assert_eq!(status, 200, "{out}");
    assert_eq!(*HC_CALLS.lock().unwrap(), vec![strs(&["hc", "get", "t3"]), strs(&["hc", "send", "t3", "回答: 問いは「推奨」で進めて。"])]);
    assert_eq!(count_of(&log, "answer"), 1);
    // A second press sends nothing.
    assert_eq!(post(port, "/api/answer", &token, &mine).0, 409);
    assert_eq!(HC_CALLS.lock().unwrap().len(), 2);
    assert_eq!(count_of(&log, "answer"), 1);
    let _ = claude.kill();
    let _ = claude.wait();
}

#[test]
fn the_token_file_holds_this_start_s_token_for_the_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let path = dir.join("share/home.token");
    let write_log_line = |sid: &str| write_log(&log, &[(sid, "start", json!({}))], SystemTime::now());

    let first = Server::bind(0, log.clone()).unwrap();
    first.write_token(&path).unwrap();
    let old = fs::read_to_string(&path).unwrap();
    assert_eq!(old.len(), 64);
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);

    // A later start replaces the file; the token from the file is the one the server takes, in
    // the header, with the same Origin check as the page.
    let ts = write_log_line("s");
    let server = Server::bind(0, log.clone()).unwrap();
    server.write_token(&path).unwrap();
    let port = server.port();
    std::thread::spawn(move || server.run());
    let token = fs::read_to_string(&path).unwrap();
    assert_ne!(token, old);
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    let target = json!({"session_id": "s", "resolves": ts[0]}).to_string();
    assert_eq!(request(port, "POST", "/api/resolve", &[("X-Decidle-Token", &token)], &target).0, 403);
    assert_eq!(post(port, "/api/resolve", &old, &json!({"session_id": "s", "resolves": ts[0]})).0, 403);
    assert_eq!(post(port, "/api/resolve", &token, &json!({"session_id": "s", "resolves": ts[0]})).0, 200);
    assert!(fs::read_dir(path.parent().unwrap()).unwrap().count() == 1, "no leftover file beside the token");
}

static RAISE_CALLS: std::sync::Mutex<Vec<Vec<String>>> = std::sync::Mutex::new(Vec::new());

fn record_raise(argv: &[String]) -> Result<String, String> {
    RAISE_CALLS.lock().unwrap().push(argv.to_vec());
    Ok(String::new())
}

#[test]
fn focus_raises_the_terminal_app_and_answer_does_not() {
    let dir = temp_dir();
    let mut claude = fake_claude(&dir);
    let pid = claude.id() as i64;
    let started = procinfo::info(pid).unwrap().started;
    let log = dir.join("events.jsonl");
    let base = SystemTime::now() - Duration::from_secs(60);
    let idle = transcript(&dir, "idle.jsonl", &[("user", base), ("assistant", base)]);
    let ts = write_log(
        &log,
        &[
            ("s", "start", json!({"pid": pid, "pid_started": started, "transcript_path": idle, "term": "cmux", "term_id": "11111111-2222-3333-4444-555555555555", "term_ws": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"})),
            ("s", "stop", json!({"report_title": "題", "decisions": 1, "questions": ["問い"], "recommends": ["推奨"]})),
        ],
        base,
    );
    let mut server = Server::bind(0, log.clone()).unwrap();
    server.set_runner(record_raise);
    server.set_host_app(Some("com.example.term".into()));
    let port = server.port();
    std::thread::spawn(move || server.run());
    let (_, page) = request(port, "GET", "/", &[], "");
    let marker = "name=\"decidle-token\" content=\"";
    let at = page.find(marker).unwrap() + marker.len();
    let token = page[at..at + 64].to_string();
    let target = json!({"session_id": "s", "resolves": ts[1]});
    let last = || RAISE_CALLS.lock().unwrap().last().cloned().unwrap();

    assert_eq!(post(port, "/api/focus", &token, &target).0, 200);
    assert_eq!(last(), vec!["open", "-b", "com.example.term"]);
    assert_eq!(post(port, "/api/answer", &token, &target).0, 200);
    assert_eq!(last()[..2], ["cmux".to_string(), "send".to_string()]);
    assert!(!RAISE_CALLS.lock().unwrap().iter().skip(3).any(|c| c[0] == "open"));
    let _ = claude.kill();
    let _ = claude.wait();
}

#[test]
fn report_json_is_served_by_session_only_for_the_recorded_file() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let report = dir.join("task.report.json");
    fs::write(&report, json!({"title": "題", "conclusion": "結論", "decisions": []}).to_string()).unwrap();
    let not_json = dir.join("broken.report.json");
    fs::write(&not_json, "not json").unwrap();
    let link = dir.join("link.report.json");
    std::os::unix::fs::symlink(&report, &link).unwrap();
    // A folder on the way that is a link to the real folder.
    let real_folder = dir.join("real");
    fs::create_dir(&real_folder).unwrap();
    fs::write(real_folder.join("task.report.json"), json!({"title": "題", "decisions": []}).to_string()).unwrap();
    let folder_link = dir.join("via");
    std::os::unix::fs::symlink(&real_folder, &folder_link).unwrap();
    let via = folder_link.join("task.report.json");
    let p = |x: &PathBuf| x.to_string_lossy().into_owned();
    write_log(
        &log,
        &[
            ("with", "start", json!({})),
            ("with", "stop", json!({"report_title": "題", "report_path": p(&report)})),
            ("none", "start", json!({})),
            ("none", "stop", json!({})),
            ("broken", "start", json!({})),
            ("broken", "stop", json!({"report_title": "題", "report_path": p(&not_json)})),
            ("linked", "start", json!({})),
            ("linked", "stop", json!({"report_title": "題", "report_path": p(&link)})),
            ("via", "start", json!({})),
            ("via", "stop", json!({"report_title": "題", "report_path": p(&via)})),
            ("gone", "start", json!({})),
            ("gone", "stop", json!({"report_title": "題", "report_path": p(&dir.join("missing.json"))})),
        ],
        SystemTime::now() - Duration::from_secs(60),
    );
    let (port, _) = start(&log);
    let get = |q: &str| request(port, "GET", &format!("/api/report{q}"), &[], "");
    let (status, body) = get("?session_id=with");
    assert_eq!(status, 200, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["report"]["conclusion"], "結論");
    for sid in ["none", "broken", "linked", "via", "gone", "unknown"] {
        assert_eq!(get(&format!("?session_id={sid}")).0, 404, "{sid}");
    }
    // The page names the stop it shows; for an older stop the report is not returned.
    assert_eq!(get("?session_id=with&stop=2020-01-01T00%3A00%3A00.000000Z").0, 409);
    // The request cannot name a path.
    assert_eq!(get("").0, 400);
    assert_eq!(get(&format!("?session_id={}", p(&report))).0, 400);
    assert_eq!(get("?session_id=..%2Fwith").0, 400);
    assert_eq!(get(&format!("?path={}", p(&report))).0, 400);
}

#[test]
fn the_page_can_tell_that_the_server_was_started_again() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    write_log(&log, &[("a", "start", json!({}))], SystemTime::now());
    let boot_of = |port: u16| {
        let (_, page) = request(port, "GET", "/", &[], "");
        let marker = "name=\"decidle-boot\" content=\"";
        let at = page.find(marker).unwrap() + marker.len();
        page[at..at + 16].to_string()
    };
    let (first, _) = start(&log);
    let (second, _) = start(&log);
    // The page and /api/sessions of one server agree; another start has another value.
    assert_eq!(sessions(first)["boot"], json!(boot_of(first)));
    assert_ne!(boot_of(first), boot_of(second));
}

#[test]
fn a_question_is_shown_with_the_permission_rows_until_it_is_answered() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let question = json!({"question": "どれにする?", "header": "選ぶ", "options": ["赤", "青"], "multi": true});
    let mut lines: Vec<(&str, &str, Value)> = vec![
        ("perm", "prompt", json!({})),
        ("perm", "permission", json!({})),
        ("st", "prompt", json!({})),
        ("st", "stop", json!({})),
        ("ask", "prompt", json!({})),
        ("ask", "ask", json!({"ask": [question.clone()]})),
        // The permission notification that comes with the question.
        ("ask", "permission", json!({})),
        ("wk", "prompt", json!({})),
    ];
    let base = SystemTime::now() - Duration::from_secs(60);
    write_log(&log, &lines, base);
    let (port, _) = start(&log);
    let want: Vec<(String, String)> = [("ask", "ask"), ("perm", "permission"), ("st", "stopped"), ("wk", "working")]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    assert_eq!(rows(port), want);
    let all = sessions(port);
    let rows_of = all["sessions"].as_array().unwrap();
    assert_eq!(rows_of[0]["ask"], json!([question]));
    assert_eq!(rows_of[0]["waits_for_human"], true);
    for r in &rows_of[1..] {
        assert_eq!(r["ask"], json!([]), "{r}");
    }

    lines.push(("ask", "asked", json!({})));
    write_log(&log, &lines, base);
    let all = sessions(port);
    let row = all["sessions"].as_array().unwrap().iter().find(|r| r["session_id"] == "ask").unwrap().clone();
    assert_eq!(row["state"], "working");
    assert_eq!(row["ask"], json!([]));
}
