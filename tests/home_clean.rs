//! Deleting old reports: what is deleted, and every case where nothing may be deleted.

use decidle::home::clean::{self, Settings};
use decidle::home::events;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

const DAY: u64 = 24 * 3600;

fn temp_dir() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("decidle-clean-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    // The real path: /var/folders is a symbolic link on macOS, and clean refuses those.
    fs::canonicalize(&dir).unwrap()
}

fn ago(secs: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(secs)
}

/// Writes a file and dates it `secs` ago.
fn file_at(path: &Path, secs: u64) -> String {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "x").unwrap();
    fs::File::options().write(true).open(path).unwrap().set_modified(ago(secs)).unwrap();
    path.to_string_lossy().into_owned()
}

/// A report pair in `<cwd>/.decidle/`, written `secs` ago. Returns (json, html).
fn report(cwd: &Path, name: &str, secs: u64) -> (String, String) {
    let d = cwd.join(".decidle");
    (file_at(&d.join(format!("{name}.report.json")), secs), file_at(&d.join(format!("{name}.html")), secs))
}

fn stop(r: &(String, String), decisions: i64) -> Value {
    json!({"report_path": r.0, "report_html": r.1, "report_title": "題", "decisions": decisions})
}

/// One line per (session, event, extra fields, seconds ago). resolved and kill take the ts of
/// the session's last stop line.
fn write_log(path: &Path, lines: &[(&str, &str, Value, u64)]) {
    let mut text = String::new();
    let mut last_stop: std::collections::HashMap<String, String> = Default::default();
    for (sid, event, extra, secs) in lines {
        let ts = events::rfc3339_at(ago(*secs));
        let mut line = json!({"ts": ts, "session_id": sid, "cwd": "/w", "event": event});
        let m = line.as_object_mut().unwrap();
        if let Some(x) = extra.as_object() {
            m.extend(x.clone());
        }
        if matches!(*event, "resolved" | "kill") {
            m.insert("resolves".into(), json!(last_stop[*sid]));
        }
        if *event == "stop" {
            last_stop.insert(sid.to_string(), ts);
        }
        text.push_str(&format!("{line}\n"));
    }
    fs::write(path, text).unwrap();
}

fn settings() -> Settings {
    Settings { after_days: 1, report_dir: ".decidle".into() }
}

fn exists(p: &str) -> bool {
    fs::symlink_metadata(p).is_ok()
}

#[test]
fn reports_of_sessions_closed_a_day_ago_are_deleted() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let n = json!({});
    let resolved = report(&dir.join("a"), "t", 3 * DAY);
    let killed = report(&dir.join("b"), "t", 3 * DAY);
    let ended = report(&dir.join("c"), "t", 3 * DAY);
    // Decisions asked and then resolved, or answered by a prompt before the session ended.
    let decided = report(&dir.join("d"), "t", 3 * DAY);
    let answered = report(&dir.join("e"), "t", 3 * DAY);
    let earlier = report(&dir.join("f"), "first", 3 * DAY);
    let later = report(&dir.join("f"), "second", 3 * DAY);
    write_log(&log, &[
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", stop(&resolved, 0), 3 * DAY), ("A", "resolved", n.clone(), 2 * DAY),
        ("B", "prompt", n.clone(), 3 * DAY), ("B", "stop", stop(&killed, 0), 3 * DAY), ("B", "kill", n.clone(), 2 * DAY),
        ("C", "prompt", n.clone(), 3 * DAY), ("C", "stop", stop(&ended, 0), 3 * DAY), ("C", "end", n.clone(), 2 * DAY),
        ("D", "prompt", n.clone(), 3 * DAY), ("D", "stop", stop(&decided, 2), 3 * DAY), ("D", "resolved", n.clone(), 2 * DAY),
        ("E", "prompt", n.clone(), 3 * DAY), ("E", "stop", stop(&answered, 1), 3 * DAY), ("E", "prompt", n.clone(), 3 * DAY), ("E", "end", n.clone(), 2 * DAY),
        // An earlier report with decisions, replaced by a newer stop, in a session that then ended.
        ("F", "prompt", n.clone(), 3 * DAY), ("F", "stop", stop(&earlier, 1), 3 * DAY), ("F", "prompt", n.clone(), 3 * DAY),
        ("F", "stop", stop(&later, 0), 3 * DAY), ("F", "end", n.clone(), 2 * DAY),
    ]);
    // A dry run lists them and deletes nothing.
    let o = clean::run(&log, &settings(), true).unwrap();
    assert_eq!(o.deleted.len(), 14, "{:?} {:?}", o.deleted, o.refused);
    assert!(o.refused.is_empty(), "{:?}", o.refused);
    for p in [&resolved, &killed, &ended, &decided, &answered, &earlier, &later] {
        assert!(exists(&p.0) && exists(&p.1));
    }
    let o = clean::run(&log, &settings(), false).unwrap();
    assert_eq!(o.deleted.len(), 14);
    for p in [&resolved, &killed, &ended, &decided, &answered, &earlier, &later] {
        assert!(!exists(&p.0) && !exists(&p.1), "{p:?}");
    }
    // Files already gone are not listed again.
    assert!(clean::run(&log, &settings(), false).unwrap().deleted.is_empty());
}

#[test]
fn open_shown_recent_and_reused_reports_are_kept() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let n = json!({});
    let recent = report(&dir.join("a"), "t", 3 * DAY);
    let working = report(&dir.join("b"), "t", 3 * DAY);
    let stopped = report(&dir.join("c"), "t", 3 * DAY);
    let stale = report(&dir.join("d"), "t", 3 * DAY);
    let open = report(&dir.join("e"), "t", 3 * DAY);
    let shared = report(&dir.join("f"), "t", 3 * DAY);
    let resumed = report(&dir.join("g"), "t", 3 * DAY);
    let background = report(&dir.join("h"), "t", 3 * DAY);
    write_log(&log, &[
        // Resolved an hour ago: not a day yet.
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", stop(&recent, 0), 3 * DAY), ("A", "resolved", n.clone(), 3600),
        // Working again after its report.
        ("B", "prompt", n.clone(), 3 * DAY), ("B", "stop", stop(&working, 0), 3 * DAY), ("B", "prompt", n.clone(), 60),
        // Stopped and shown on the Home page.
        ("C", "prompt", n.clone(), 3 * DAY), ("C", "stop", stop(&stopped, 0), 60),
        // Stopped two days ago: hidden after 12 hours, but never resolved or ended.
        ("D", "prompt", n.clone(), 3 * DAY), ("D", "stop", stop(&stale, 0), 2 * DAY),
        // Decisions asked, then the session ended without anyone answering.
        ("E", "prompt", n.clone(), 3 * DAY), ("E", "stop", stop(&open, 2), 3 * DAY), ("E", "end", n.clone(), 2 * DAY),
        // The same path recorded by an ended session and by one that is still stopped.
        ("F", "prompt", n.clone(), 3 * DAY), ("F", "stop", stop(&shared, 0), 3 * DAY), ("F", "end", n.clone(), 2 * DAY),
        ("F2", "prompt", n.clone(), 3 * DAY), ("F2", "stop", stop(&shared, 0), 60),
        // Resolved long ago, then resumed and recorded the same path again; now working.
        ("G", "prompt", n.clone(), 3 * DAY), ("G", "stop", stop(&resumed, 0), 3 * DAY), ("G", "resolved", n.clone(), 2 * DAY),
        ("G", "prompt", n.clone(), 120), ("G", "stop", stop(&resumed, 0), 90), ("G", "prompt", n.clone(), 60),
        // Waiting on background work: resolved does not apply, so it is not closed.
        ("H", "prompt", n.clone(), 3 * DAY), ("H", "stop", json!({"report_path": background.0, "report_html": background.1, "decisions": 0, "background": true}), 3 * DAY),
        ("H", "resolved", n.clone(), 2 * DAY),
    ]);
    let o = clean::run(&log, &settings(), false).unwrap();
    assert!(o.deleted.is_empty(), "{:?}", o.deleted);
    for p in [&recent, &working, &stopped, &stale, &open, &shared, &resumed, &background] {
        assert!(exists(&p.0) && exists(&p.1), "{p:?}");
    }
}

#[test]
fn only_recorded_files_inside_the_report_folder_are_deleted() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let n = json!({});
    let cwd = dir.join("w");
    let good = report(&cwd, "t", 3 * DAY);
    // Not recorded, in the same folder and with a similar name.
    let unrecorded = file_at(&cwd.join(".decidle/t2.report.json"), 3 * DAY);
    let similar = file_at(&cwd.join(".decidle/t.report.json.bak"), 3 * DAY);
    // Recorded, but outside a .decidle folder, or not a report file.
    let outside = file_at(&cwd.join("notes/t.report.json"), 3 * DAY);
    let source = file_at(&cwd.join(".decidle/main.rs"), 3 * DAY);
    // `..` would leave the folder.
    let dotdot = format!("{}/.decidle/../t.report.json", cwd.display());
    file_at(&cwd.join("t.report.json"), 3 * DAY);
    // A .decidle that is a symbolic link to another folder, and a file that is a link out.
    let target = dir.join("elsewhere");
    let linked_file = file_at(&target.join("keep.html"), 3 * DAY);
    let linked_cwd = dir.join("l");
    fs::create_dir_all(&linked_cwd).unwrap();
    std::os::unix::fs::symlink(&target, linked_cwd.join(".decidle")).unwrap();
    let via_link = format!("{}/.decidle/keep.html", linked_cwd.display());
    let out_link = cwd.join(".decidle/out.html");
    std::os::unix::fs::symlink(&linked_file, &out_link).unwrap();
    let out_link = out_link.to_string_lossy().into_owned();
    // Written again after the session ended (a newer report at the same path).
    let rewritten = report(&dir.join("r"), "t", 60);
    let rec = |p: &str| json!({"report_path": p, "decisions": 0});
    write_log(&log, &[
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", stop(&good, 0), 3 * DAY),
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", rec(&outside), 3 * DAY),
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", rec(&source), 3 * DAY),
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", rec(&dotdot), 3 * DAY),
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", rec(&via_link), 3 * DAY),
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", rec(&out_link), 3 * DAY),
        ("A", "prompt", n.clone(), 3 * DAY), ("A", "stop", stop(&rewritten, 0), 3 * DAY),
        ("A", "end", n.clone(), 2 * DAY),
    ]);
    let o = clean::run(&log, &settings(), false).unwrap();
    let deleted: Vec<String> = o.deleted.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    assert_eq!(deleted, vec![good.1.clone(), good.0.clone()], "{:?}", o.refused);
    let refused: Vec<String> = o.refused.iter().map(|(p, _)| p.to_string_lossy().into_owned()).collect();
    for p in [&outside, &source, &dotdot, &via_link, &out_link, &rewritten.0, &rewritten.1] {
        assert!(refused.contains(p), "{p} not refused: {:?}", o.refused);
    }
    for p in [&unrecorded, &similar, &outside, &source, &linked_file, &via_link, &out_link, &rewritten.0, &rewritten.1] {
        assert!(exists(p), "{p}");
    }
    assert!(exists(&format!("{}/t.report.json", cwd.display())));
}

#[test]
fn zero_days_turns_cleaning_off() {
    let dir = temp_dir();
    let log = dir.join("events.jsonl");
    let r = report(&dir.join("a"), "t", 3 * DAY);
    write_log(&log, &[("A", "stop", stop(&r, 0), 3 * DAY), ("A", "end", json!({}), 2 * DAY)]);
    let o = clean::run(&log, &Settings { after_days: 0, report_dir: ".decidle".into() }, false).unwrap();
    assert!(o.deleted.is_empty());
    let o = clean::run(&log, &Settings { after_days: 3, report_dir: ".decidle".into() }, false).unwrap();
    assert!(o.deleted.is_empty());
    assert!(exists(&r.0) && exists(&r.1));
}

#[test]
fn clean_command_dry_run_lists_and_keeps() {
    let dir = temp_dir();
    let home = dir.join("home");
    let log = home.join(".local/share/decidle/events.jsonl");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    let r = report(&dir.join("a"), "t", 3 * DAY);
    write_log(&log, &[("A", "stop", stop(&r, 0), 3 * DAY), ("A", "end", json!({}), 2 * DAY)]);
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_decidle"))
            .arg("clean")
            .args(args)
            .env("HOME", &home)
            .env_remove("DECIDLE_CONFIG")
            .output()
            .unwrap()
    };
    let out = run(&["--dry-run"]);
    assert!(out.status.success());
    let listed = String::from_utf8_lossy(&out.stdout);
    assert_eq!(listed.lines().collect::<Vec<_>>(), vec![r.1.as_str(), r.0.as_str()]);
    assert!(exists(&r.0) && exists(&r.1));
    let before = fs::read_to_string(&log).unwrap();
    let out = run(&[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("2 件を消した"));
    assert!(!exists(&r.0) && !exists(&r.1));
    // Nothing is written to events.jsonl.
    assert_eq!(fs::read_to_string(&log).unwrap(), before);
    assert!(!run(&["--bogus"]).status.success());
}
