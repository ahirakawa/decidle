//! Codex CLI: telling it apart, the hooks it records, its transcript for the background check
//! and the kill check. Inputs are dummies shaped like Codex CLI 0.169's.

use decidle::home::agent::{self, Agent};
use decidle::home::state::{self, Event, Kind, St};
use decidle::home::{activity, background};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

fn temp_dir() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("decidle-codex-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

const CODEX_PATH: &str = "/Users/u/.codex/sessions/2026/11/18/rollout-2026-11-18T00-00-00-dummy.jsonl";

#[test]
fn hook_inputs_are_told_apart_by_the_transcript_path() {
    assert_eq!(agent::detect(&json!({"transcript_path": CODEX_PATH, "hook_event_name": "SessionStart"})), Agent::Codex);
    assert_eq!(agent::detect(&json!({"transcript_path": CODEX_PATH, "hook_event_name": "SessionEnd"})), Agent::Codex);
    assert_eq!(agent::detect(&json!({"transcript_path": "/Users/u/.claude/projects/p/abc.jsonl", "turn_id": "t"})), Agent::Claude);
    assert_eq!(agent::detect(&json!({"turn_id": "t1"})), Agent::Codex);
    assert_eq!(agent::detect(&json!({"session_id": "s"})), Agent::Claude);
    // A file named rollout outside a sessions folder is not taken for Codex.
    assert_eq!(agent::detect(&json!({"transcript_path": "/tmp/rollout-x.jsonl"})), Agent::Claude);
}

fn hook(home: &Path, input: &Value) -> Output {
    // The existing wording is checked in Japanese.
    let config = home.join(".config/decidle/config.json");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, r#"{"language": "ja"}"#).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_decidle"))
        .arg("event")
        .env("HOME", home)
        .env_remove("DECIDLE_CONFIG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.to_string().as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

fn lines(home: &Path) -> Vec<Value> {
    fs::read_to_string(home.join(".local/share/decidle/events.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn codex_hooks_record_start_prompt_permission_interrupt_and_end() {
    let home = temp_dir();
    let base = json!({"session_id": "c1", "cwd": "/w", "transcript_path": CODEX_PATH, "model": "dummy", "permission_mode": "default"});
    let with = |extra: Value| {
        let mut v = base.clone();
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    let out = hook(&home, &with(json!({"hook_event_name": "SessionStart", "source": "startup"})));
    let ctx: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(ctx["hookSpecificOutput"]["additionalContext"], decidle::home::note::instruction(decidle::i18n::Lang::Ja));
    hook(&home, &with(json!({"hook_event_name": "UserPromptSubmit", "turn_id": "t1", "prompt": "秘密の依頼文"})));
    hook(&home, &with(json!({"hook_event_name": "PermissionRequest", "turn_id": "t1", "tool_name": "Bash",
        "tool_input": {"command": "touch probe.txt", "description": "作る"}})));
    hook(&home, &with(json!({"hook_event_name": "Interrupt", "turn_id": "t1"})));
    hook(&home, &with(json!({"hook_event_name": "PreToolUse", "turn_id": "t1", "tool_name": "Bash"})));
    hook(&home, &with(json!({"hook_event_name": "SessionEnd", "reason": "other"})));
    let got = lines(&home);
    let events: Vec<&str> = got.iter().map(|l| l["event"].as_str().unwrap()).collect();
    assert_eq!(events, vec!["start", "prompt", "permission", "interrupt", "end"]);
    assert_eq!(got[0]["agent"], "codex");
    let text = fs::read_to_string(home.join(".local/share/decidle/events.jsonl")).unwrap();
    assert!(!text.contains("秘密の依頼文") && !text.contains("probe.txt"));
}

fn ev(kind: &str, us: i64) -> Event {
    Event { ts: format!("t{us}"), ts_us: us, session_id: "c1".into(), kind: Kind::parse(kind), ..Event::default() }
}

#[test]
fn interrupt_moves_a_turn_to_idle_and_is_ignored_elsewhere() {
    let fold = |kinds: &[&str]| {
        let evs: Vec<Event> = kinds.iter().enumerate().map(|(i, k)| ev(k, i as i64 + 1)).collect();
        state::derive(&evs)[0].st
    };
    assert_eq!(fold(&["start", "prompt", "permission", "interrupt"]), St::Idle);
    assert_eq!(fold(&["start", "prompt", "interrupt"]), St::Idle);
    assert_eq!(fold(&["start", "prompt", "stop", "interrupt"]), St::Stopped);
    assert_eq!(fold(&["start", "interrupt", "prompt"]), St::Working);
    let mut bg = vec![ev("start", 1), ev("prompt", 2), Event { background: true, ..ev("stop", 3) }];
    bg.push(ev("interrupt", 4));
    assert_eq!(state::derive(&bg)[0].st, St::Idle);
    // An idle session is not on the page.
    assert!(!state::shown(&state::derive(&[ev("start", 1), ev("prompt", 2), ev("interrupt", 3)])[0]));
}

fn item(payload: Value, ts: &str) -> String {
    json!({"timestamp": ts, "ordinal": 1, "type": "response_item", "payload": payload}).to_string()
}

fn spawn() -> Value {
    json!({"type": "function_call", "name": "spawn_agent", "namespace": "collaboration", "call_id": "a1",
        "arguments": "{\"task_name\":\"x\",\"message\":\"y\"}"})
}

#[test]
fn background_subagents_in_a_codex_transcript() {
    let ts = "2026-11-18T00:00:00.000Z";
    let wait = json!({"type": "function_call", "name": "wait_agent", "call_id": "w1", "arguments": "{\"timeout_ms\":1000}"});
    let answer = json!({"type": "agent_message", "author": "/root/x", "recipient": "/root", "content": [{"type": "text", "text": "z"}]});
    let close = json!({"type": "function_call", "name": "close_agent", "call_id": "c", "arguments": "{}"});
    let scan = |payloads: &[Value]| {
        let ls: Vec<String> = payloads.iter().map(|p| item(p.clone(), ts)).collect();
        background::scan(ls.iter().map(String::as_str)).tasks
    };
    assert_eq!(scan(&[spawn()]), 1);
    assert_eq!(scan(&[spawn(), wait.clone()]), 1);
    assert_eq!(scan(&[spawn(), wait, answer.clone()]), 0);
    assert_eq!(scan(&[spawn(), spawn(), answer.clone()]), 1);
    assert_eq!(scan(&[spawn(), close]), 0);
    assert_eq!(scan(&[answer]), 0);
}

#[test]
fn kill_check_reads_codex_conversation_lines() {
    let dir = temp_dir();
    let path = dir.join("rollout-dummy.jsonl");
    let stop_us = decidle::home::events::parse_rfc3339_us("2026-11-18T00:00:10.000000Z").unwrap();
    let msg = |role: &str| json!({"type": "message", "role": role, "content": [{"type": "input_text", "text": "x"}]});
    let write = |extra: &[String]| {
        let mut ls = vec![item(msg("user"), "2026-11-18T00:00:01.000Z"), item(msg("assistant"), "2026-11-18T00:00:05.000Z")];
        ls.extend_from_slice(extra);
        fs::write(&path, ls.join("\n") + "\n").unwrap();
    };
    write(&[]);
    assert_eq!(activity::worked_since(&path, stop_us), Ok(false));
    // Bookkeeping after the stop does not count; developer messages do not either.
    write(&[
        json!({"timestamp": "2026-11-18T00:00:20.000Z", "ordinal": 9, "type": "event_msg", "payload": {"type": "task_complete"}}).to_string(),
        item(msg("developer"), "2026-11-18T00:00:21.000Z"),
    ]);
    assert_eq!(activity::worked_since(&path, stop_us), Ok(false));
    write(&[item(spawn(), "2026-11-18T00:00:30.000Z")]);
    assert_eq!(activity::worked_since(&path, stop_us), Ok(true));
    write(&[item(msg("user"), "2026-11-18T00:00:30.000Z")]);
    assert_eq!(activity::worked_since(&path, stop_us), Ok(true));
}
