//! `decidle event`: concurrent appends, failure to write, and what is never recorded.

use decidle::home::{events, read, state::Kind};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_home() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("decidle-home-event-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn log_of(home: &Path) -> PathBuf {
    home.join(".local/share/decidle/events.jsonl")
}

fn hook(home: &Path, input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_decidle"))
        .arg("event")
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn lines(home: &Path) -> Vec<Value> {
    fs::read_to_string(log_of(home))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn concurrent_hooks_do_not_lose_or_mix_lines() {
    let home = temp_home();
    let workers: Vec<_> = (0..10)
        .map(|w| {
            let home = home.clone();
            std::thread::spawn(move || {
                for i in 0..1000 {
                    let input = json!({
                        "session_id": format!("w{w}"),
                        "cwd": format!("/work/w{w}/{i}"),
                        "hook_event_name": "UserPromptSubmit",
                        "prompt": "x".repeat(200),
                    });
                    let out = hook(&home, &input.to_string());
                    assert!(out.status.success());
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    let text = fs::read_to_string(log_of(&home)).unwrap();
    let parsed: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).expect(l)).collect();
    assert_eq!(parsed.len(), 10_000);
    for w in 0..10 {
        let mine: Vec<&Value> = parsed.iter().filter(|v| v["session_id"] == format!("w{w}")).collect();
        assert_eq!(mine.len(), 1000, "w{w}");
        // One worker's lines stay in its own order.
        for (i, v) in mine.iter().enumerate() {
            assert_eq!(v["cwd"], format!("/work/w{w}/{i}"));
        }
    }
    let loaded = read::load(&log_of(&home)).unwrap();
    assert_eq!((loaded.events.len(), loaded.skipped), (10_000, 0));
}

#[test]
fn unwritable_log_still_exits_zero() {
    let home = temp_home();
    let input = json!({"session_id": "s", "cwd": "/w", "hook_event_name": "UserPromptSubmit", "prompt": "p"});
    // The folder above the data folder is read-only, so the data folder cannot be made.
    let share = home.join(".local/share");
    fs::create_dir_all(&share).unwrap();
    fs::set_permissions(&share, fs::Permissions::from_mode(0o500)).unwrap();
    let out = hook(&home, &input.to_string());
    fs::set_permissions(&share, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(!log_of(&home).exists());

    // The log itself is read-only.
    fs::create_dir_all(log_of(&home).parent().unwrap()).unwrap();
    fs::write(log_of(&home), "").unwrap();
    fs::set_permissions(log_of(&home), fs::Permissions::from_mode(0o400)).unwrap();
    let out = hook(&home, &input.to_string());
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert_eq!(fs::metadata(log_of(&home)).unwrap().len(), 0);
    fs::remove_file(log_of(&home)).unwrap();

    // A log that is a folder, broken input and empty input end the same way.
    fs::create_dir_all(log_of(&home)).unwrap();
    for input in [input.to_string(), "{not json".to_string(), String::new()] {
        let out = hook(&home, &input);
        assert_eq!(out.status.code(), Some(0), "{input}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn prompt_and_message_text_are_not_recorded() {
    let home = temp_home();
    let secret = "SECRET-7f3a91-prompt-text";
    let inputs = [
        json!({"session_id": "s", "cwd": "/w", "hook_event_name": "SessionStart", "source": "startup"}),
        json!({"session_id": "s", "cwd": "/w", "hook_event_name": "UserPromptSubmit", "prompt": secret}),
        json!({"session_id": "s", "cwd": "/w", "hook_event_name": "Notification",
               "notification_type": "permission_prompt", "message": format!("needs permission {secret}")}),
        json!({"session_id": "s", "cwd": "/w", "hook_event_name": "Notification",
               "notification_type": "idle_prompt", "message": "waiting for your input"}),
        json!({"session_id": "s", "cwd": "/w", "hook_event_name": "Stop", "last_assistant_message": secret}),
        json!({"session_id": "s", "cwd": "/w", "hook_event_name": "SessionEnd", "reason": "exit"}),
    ];
    for input in &inputs {
        assert_eq!(hook(&home, &input.to_string()).status.code(), Some(0));
    }
    let text = fs::read_to_string(log_of(&home)).unwrap();
    assert_eq!(text.matches(secret).count(), 0);
    let events: Vec<String> = lines(&home).iter().map(|v| v["event"].as_str().unwrap().to_string()).collect();
    // The idle notification and the Stop hook are not recorded by `event`.
    assert_eq!(events, ["start", "prompt", "permission", "end"]);
    let start = &lines(&home)[0];
    assert!(start.get("pid").is_some() && start.get("pid_started").is_some());
    assert!(start.get("transcript_path").is_some());
    let mode = fs::metadata(log_of(&home)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_cut_off_line_does_not_swallow_the_next_prompt() {
    let home = temp_home();
    let log = log_of(&home);
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    // A stop, then a line cut off by a crash in the middle of an append.
    let stop = json!({"ts": "2026-11-18T00:00:00.000000Z", "session_id": "s", "cwd": "/w", "event": "stop"});
    fs::write(&log, format!("{stop}\n{{\"ts\": \"2026-11-18T00:00:01.000000Z\", \"session_id\": \"s\", \"ev")).unwrap();
    let input = json!({"session_id": "s", "cwd": "/w", "hook_event_name": "UserPromptSubmit", "prompt": "p"});
    assert_eq!(hook(&home, &input.to_string()).status.code(), Some(0));
    let loaded = read::load(&log).unwrap();
    assert_eq!(loaded.skipped, 1, "only the cut-off line is lost");
    let kinds: Vec<_> = loaded.events.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [Kind::parse("stop"), Kind::parse("prompt")]);
}

#[test]
fn ts_are_unique_and_follow_the_line_order() {
    let home = temp_home();
    let log = log_of(&home);
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    // The last line is stamped an hour ahead (a clock that was set back since).
    let ahead = events::rfc3339_at(SystemTime::now() + Duration::from_secs(3600));
    fs::write(&log, format!("{}\n", json!({"ts": ahead, "session_id": "s", "cwd": "/w", "event": "prompt"}))).unwrap();
    for _ in 0..50 {
        events::append_stop(&log, "s", "/w", None, &events::StopInfo::default()).unwrap();
    }
    let ts: Vec<String> = lines(&home).iter().map(|v| v["ts"].as_str().unwrap().to_string()).collect();
    assert_eq!(ts.len(), 51);
    for pair in ts.windows(2) {
        assert!(pair[0] < pair[1], "{} then {}", pair[0], pair[1]);
    }
    assert_eq!(
        events::parse_rfc3339_us(&ts[1]).unwrap() - events::parse_rfc3339_us(&ahead).unwrap(),
        1,
        "moved just past the line before"
    );
}

#[test]
fn rfc3339_round_trips_and_reads_transcript_timestamps() {
    for us in [0i64, 951_782_400_123_456, 1_794_962_470_817_535, 4_102_444_799_999_999] {
        assert_eq!(events::parse_rfc3339_us(&events::rfc3339_from_us(us)), Some(us));
    }
    assert_eq!(events::parse_rfc3339_us("2026-11-18T01:29:49.356Z"), events::parse_rfc3339_us("2026-11-18T01:29:49.356000Z"));
    assert_eq!(events::parse_rfc3339_us("2026-11-18T01:29:49Z"), events::parse_rfc3339_us("2026-11-18T01:29:49.000000Z"));
    for bad in ["", "2026-11-18 01:29:49Z", "2026-11-18T01:29:49+09:00", "2026-13-18T01:29:49Z", "2026-11-18T01:29:49.Z"] {
        assert_eq!(events::parse_rfc3339_us(bad), None, "{bad}");
    }
}

#[test]
fn session_start_gives_the_home_line_instruction_as_additional_context() {
    let home = temp_home();
    let start = |source: &str| json!({"session_id": "s", "cwd": "/w", "hook_event_name": "SessionStart", "source": source});
    let context = |out: &Output| -> String {
        let v: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{:?}", out.stdout));
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string()
    };
    let out = hook(&home, &start("startup").to_string());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(context(&out), decidle::home::note::INSTRUCTION);
    assert_eq!(lines(&home).len(), 1);

    // After a compaction the instruction is given again, but no start line is appended:
    // the same claude keeps running, and a start would hide the session.
    let out = hook(&home, &start("compact").to_string());
    assert_eq!(context(&out), decidle::home::note::INSTRUCTION);
    assert_eq!(lines(&home).len(), 1);

    // When the append fails, the instruction is still given.
    fs::remove_file(log_of(&home)).unwrap();
    fs::create_dir_all(log_of(&home)).unwrap();
    let out = hook(&home, &start("startup").to_string());
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(context(&out), decidle::home::note::INSTRUCTION);
}

#[test]
fn compaction_does_not_hide_a_working_session() {
    let home = temp_home();
    let ev = |name: &str, extra: Value| {
        let mut v = json!({"session_id": "s", "cwd": "/w", "hook_event_name": name});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert_eq!(hook(&home, &v.to_string()).status.code(), Some(0));
    };
    ev("SessionStart", json!({"source": "startup"}));
    ev("UserPromptSubmit", json!({"prompt": "p"}));
    ev("SessionStart", json!({"source": "compact"}));
    let loaded = read::load(&log_of(&home)).unwrap();
    let kinds: Vec<Option<Kind>> = loaded.events.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [Some(Kind::Start), Some(Kind::Prompt)], "compact appended a line");
    let now = loaded.events.last().unwrap().ts_us + 1;
    let rows = decidle::home::state::visible(decidle::home::state::derive(&loaded.events), now);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].st, decidle::home::state::St::Working);
}

#[test]
fn a_question_is_recorded_without_descriptions_or_answers() {
    let home = temp_home();
    let secret = "SECRET-answer-どれも微妙";
    let question = |n: usize| {
        let options: Vec<Value> =
            (0..10).map(|k| json!({"label": format!("q{n}o{k}"), "description": format!("DESC-{n}-{k}")})).collect();
        json!({"question": format!("q{n}\nwhich?"), "header": format!("h{n}"), "options": options, "multiSelect": n == 1})
    };
    let mut questions: Vec<Value> = (0..5).map(question).collect();
    questions[0]["question"] = json!("問".repeat(250));
    questions[0]["header"] = json!("見".repeat(50));
    questions[0]["options"][0]["label"] = json!("択".repeat(100));
    let base = |name: &str, tool: &str| json!({"session_id": "s", "cwd": "/w", "hook_event_name": name, "tool_name": tool});
    let mut pre = base("PreToolUse", "AskUserQuestion");
    pre["tool_input"] = json!({"questions": questions});
    let mut post = base("PostToolUse", "AskUserQuestion");
    post["tool_input"] = json!({"questions": [question(0)], "answers": {"q0": secret}});
    post["tool_response"] = json!({"questions": [question(0)], "answers": {"q0": secret}});
    let mut other_pre = base("PreToolUse", "Bash");
    other_pre["tool_input"] = json!({"command": "ls"});
    let other_post = base("PostToolUse", "Bash");
    for input in [&pre, &other_pre, &other_post, &post] {
        let out = hook(&home, &input.to_string());
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.is_empty());
    }
    let text = fs::read_to_string(log_of(&home)).unwrap();
    assert!(!text.contains("DESC-") && !text.contains(secret), "{text}");
    let lines = lines(&home);
    let events: Vec<&str> = lines.iter().map(|v| v["event"].as_str().unwrap()).collect();
    assert_eq!(events, ["ask", "asked"]);
    assert!(lines[1].get("ask").is_none());

    let ask = lines[0]["ask"].as_array().unwrap();
    assert_eq!(ask.len(), 4, "only the first four questions");
    assert_eq!(ask[0]["question"], format!("{}…", "問".repeat(200)));
    assert_eq!(ask[0]["header"], format!("{}…", "見".repeat(40)));
    let options = ask[0]["options"].as_array().unwrap();
    assert_eq!(options.len(), 8, "only the first eight options");
    assert_eq!(options[0], format!("{}…", "択".repeat(80)));
    assert_eq!(options[7], "q0o7");
    assert_eq!(ask[1]["question"], "q1 which?", "line breaks become spaces");
    assert_eq!(ask[1]["header"], "h1");
    assert_eq!((ask[0]["multi"].clone(), ask[1]["multi"].clone()), (json!(false), json!(true)));

    // The server reads the questions back.
    let loaded = read::load(&log_of(&home)).unwrap();
    assert_eq!(loaded.events[0].kind, Some(Kind::Ask));
    assert_eq!(loaded.events[0].ask.len(), 4);
    assert_eq!(loaded.events[0].ask[1].options[0], "q1o0");
    assert!(loaded.events[0].ask[1].multi);
    assert_eq!(loaded.events[1].kind, Some(Kind::Asked));
}

#[test]
fn ask_values_of_the_wrong_shape_read_as_empty() {
    let home = temp_home();
    let log = log_of(&home);
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    let line = |i: usize, event: &str, ask: Value| {
        json!({"ts": format!("2026-11-19T00:00:00.{i:06}Z"), "session_id": "s", "cwd": "/w", "event": event, "ask": ask})
            .to_string()
    };
    let text = [
        line(1, "ask", json!("not an array")),
        line(2, "ask", json!([1, {"question": 5, "header": "h", "options": ["a", 2], "multi": "yes"}])),
        // Only an ask line carries questions.
        line(3, "prompt", json!([{"question": "q", "header": "h", "options": ["a"], "multi": true}])),
    ]
    .join("\n");
    fs::write(&log, text + "\n").unwrap();
    let loaded = read::load(&log).unwrap();
    assert_eq!(loaded.skipped, 0);
    assert!(loaded.events[0].ask.is_empty());
    let ask = &loaded.events[1].ask;
    assert_eq!(ask.len(), 1);
    assert_eq!((ask[0].question.as_str(), ask[0].header.as_str(), ask[0].multi), ("", "h", false));
    assert_eq!(ask[0].options, ["a", ""]);
    assert!(loaded.events[2].ask.is_empty());
}

#[test]
fn start_records_the_app_terminal_from_hc_terminal_id() {
    let start = json!({"session_id": "s", "cwd": "/w", "hook_event_name": "SessionStart", "source": "startup"}).to_string();
    let run = |vars: &[(&str, &str)]| {
        let home = temp_home();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_decidle"));
        cmd.arg("event").env("HOME", &home).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for k in ["HERDR_PANE_ID", "HC_TERMINAL_ID", "CMUX_SURFACE_ID", "CMUX_WORKSPACE_ID"] {
            cmd.env_remove(k);
        }
        cmd.envs(vars.iter().copied());
        let mut child = cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(start.as_bytes()).unwrap();
        assert_eq!(child.wait_with_output().unwrap().status.code(), Some(0));
        let line = lines(&home).pop().unwrap();
        (line["term"].clone(), line["term_id"].clone())
    };
    let cmux = [("CMUX_SURFACE_ID", "11111111-2222-3333-4444-555555555555"), ("CMUX_WORKSPACE_ID", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")];
    assert_eq!(run(&[("HC_TERMINAL_ID", "t3")]), (json!("hc"), json!("t3")));
    assert_eq!(run(&[cmux[0], cmux[1], ("HC_TERMINAL_ID", "t3")]), (json!("hc"), json!("t3")));
    assert_eq!(run(&[("HC_TERMINAL_ID", "t3"), ("HERDR_PANE_ID", "w1:p2")]), (json!("herdr"), json!("w1:p2")));
    assert_eq!(run(&cmux), (json!("cmux"), json!(cmux[0].1)));
    assert_eq!(run(&[("HC_TERMINAL_ID", "host/t1")]), (Value::Null, Value::Null));
}
