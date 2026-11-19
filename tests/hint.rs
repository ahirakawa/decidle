//! `decidle hint`, the PostToolUse hook that hands the report format over once per turn.

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    home: PathBuf,
    cwd: PathBuf,
}

impl Sandbox {
    fn new(language: &str) -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("decidle-hint-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let cwd = root.join("work");
        fs::create_dir_all(home.join(".config/decidle")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        // The default thresholds, with ignore patterns that do not exclude the temp folder.
        let config = json!({
            "language": language,
            "edited_files_threshold": 3,
            "tool_call_threshold": 20,
            "trigger_commands": ["maestri ask"],
            "ignore_path_patterns": [".decidle/"],
        });
        fs::write(home.join(".config/decidle/config.json"), config.to_string()).unwrap();
        Sandbox { home, cwd }
    }

    fn file(&self, rel: &str) -> String {
        let p = self.cwd.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "x").unwrap();
        p.to_string_lossy().into_owned()
    }

    /// One turn with prompt id `p1` and the given tool calls, with no stop feedback yet.
    fn transcript(&self, tools: &[Value]) -> PathBuf {
        let lines = [
            json!({"type":"user","uuid":"u1","promptId":"p1","message":{"content":"作業して"}}),
            json!({"type":"assistant","uuid":"a1","message":{"content":tools}}),
        ];
        let path = self.home.join("transcript.jsonl");
        fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        path
    }

    fn run(&self, sub: &str, stdin: &str, mode: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_decidle"));
        cmd.arg(sub)
            .env("HOME", &self.home)
            .env_remove("DECIDLE_CONFIG")
            .env_remove("DECIDLE_MODE")
            .env_remove("MAESTRI_SOCKET")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(m) = mode {
            cmd.env("DECIDLE_MODE", m);
        }
        let mut child = cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }

    fn input(&self, transcript: &Path, prompt_id: &str) -> String {
        json!({
            "session_id": "s1",
            "transcript_path": transcript,
            "cwd": self.cwd,
            "prompt_id": prompt_id,
            "hook_event_name": "PostToolUse",
            "tool_name": "Edit",
        })
        .to_string()
    }

    /// stdout of `decidle hint`, after checking that it exited 0.
    fn hint(&self, transcript: &Path, prompt_id: &str) -> String {
        self.hint_raw(&self.input(transcript, prompt_id), None)
    }

    fn hint_raw(&self, stdin: &str, mode: Option<&str>) -> String {
        let out = self.run("hint", stdin, mode);
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }

    /// Whether `decidle stop` blocks this turn for a missing report.
    fn stop_blocks(&self, transcript: &Path) -> bool {
        let mut input: Value = serde_json::from_str(&self.input(transcript, "p1")).unwrap();
        input["last_assistant_message"] = json!("終わった。");
        let out = self.run("stop", &input.to_string(), None);
        let text = String::from_utf8(out.stdout).unwrap();
        text.trim().parse::<Value>().is_ok_and(|v| v["decision"] == "block")
    }

    fn state_file(&self) -> PathBuf {
        self.home.join(".local/share/decidle/state/s1.hint")
    }
}

fn edit(path: &str) -> Value {
    json!({"type":"tool_use","name":"Edit","input":{"file_path":path}})
}

fn bash(command: &str) -> Value {
    json!({"type":"tool_use","name":"Bash","input":{"command":command}})
}

fn reads(n: usize) -> Vec<Value> {
    (0..n).map(|i| json!({"type":"tool_use","name":"Read","input":{"file_path":format!("/x/{i}")}})).collect()
}

fn edits(sb: &Sandbox, n: usize) -> Vec<Value> {
    (0..n).map(|i| edit(&sb.file(&format!("src/f{i}.rs")))).collect()
}

fn context(out: &str) -> String {
    let v: Value = serde_json::from_str(out.trim()).unwrap_or_else(|e| panic!("{e}: {out}"));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse", "{out}");
    v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string()
}

#[test]
fn first_post_tool_use_over_the_conditions_hands_the_format_over() {
    let sb = Sandbox::new("ja");
    let t = sb.transcript(&edits(&sb, 3));
    let ctx = context(&sb.hint(&t, "p1"));
    assert!(ctx.contains("報告の JSON を書くこと"), "{ctx}");
    assert!(ctx.contains(include_str!("../docs/report-format.ja.md")), "{ctx}");
    assert!(ctx.contains("編集したファイルが 3 個以上"), "{ctx}");
    let log = fs::read_to_string(sb.home.join(".local/share/decidle/gate.jsonl")).unwrap();
    assert!(log.contains(r#""verdict":"hint""#), "{log}");
}

#[test]
fn second_call_in_the_same_turn_prints_nothing() {
    let sb = Sandbox::new("ja");
    let t = sb.transcript(&edits(&sb, 3));
    assert!(!sb.hint(&t, "p1").is_empty());
    assert_eq!(sb.hint(&t, "p1"), "");
}

#[test]
fn a_new_turn_gets_the_format_again() {
    let sb = Sandbox::new("ja");
    let t = sb.transcript(&edits(&sb, 3));
    assert!(!sb.hint(&t, "p1").is_empty());
    let lines = [
        json!({"type":"user","uuid":"u1","promptId":"p1","message":{"content":"作業して"}}),
        json!({"type":"assistant","uuid":"a1","message":{"content":edits(&sb, 3)}}),
        json!({"type":"user","uuid":"u2","promptId":"p2","message":{"content":"次も"}}),
        json!({"type":"assistant","uuid":"a2","message":{"content":edits(&sb, 3)}}),
    ];
    fs::write(&t, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    assert!(!sb.hint(&t, "p2").is_empty());
}

#[test]
fn a_turn_under_the_conditions_prints_nothing() {
    let sb = Sandbox::new("ja");
    let mut tools = edits(&sb, 1);
    tools.extend(reads(4));
    let t = sb.transcript(&tools);
    assert_eq!(sb.hint(&t, "p1"), "");
    assert!(!sb.state_file().exists());
}

#[test]
fn a_turn_that_already_wrote_the_report_prints_nothing() {
    let sb = Sandbox::new("ja");
    let report = sb.cwd.join(".decidle/task.report.json");
    fs::create_dir_all(report.parent().unwrap()).unwrap();
    fs::write(&report, "{}").unwrap();
    let mut tools = edits(&sb, 3);
    tools.push(json!({"type":"tool_use","name":"Write","input":{"file_path":report}}));
    let t = sb.transcript(&tools);
    assert_eq!(sb.hint(&t, "p1"), "");
}

#[test]
fn hint_and_stop_agree_at_the_edges_of_the_conditions() {
    let cases: Vec<(&str, bool, Box<dyn Fn(&Sandbox) -> Vec<Value>>)> = vec![
        ("2 files", false, Box::new(|sb| edits(sb, 2))),
        ("3 files", true, Box::new(|sb| edits(sb, 3))),
        ("19 tools", false, Box::new(|_| reads(19))),
        ("20 tools", true, Box::new(|_| reads(20))),
        ("trigger command", true, Box::new(|_| vec![bash("maestri ask 'Terminal #2' 'review'")])),
        ("trigger mentioned only", false, Box::new(|_| vec![bash("echo maestri ask")])),
    ];
    for (name, expected, tools) in cases {
        let sb = Sandbox::new("ja");
        let t = sb.transcript(&tools(&sb));
        let hinted = !sb.hint(&t, "p1").is_empty();
        assert_eq!(hinted, expected, "{name}");
        assert_eq!(sb.stop_blocks(&t), expected, "{name}");
    }
}

#[test]
fn english_hint_has_no_japanese() {
    let sb = Sandbox::new("en");
    let t = sb.transcript(&edits(&sb, 3));
    let ctx = context(&sb.hint(&t, "p1"));
    assert!(ctx.contains("write the report JSON before you stop"), "{ctx}");
    let japanese = |c: char| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}');
    assert!(!ctx.chars().any(japanese), "{ctx}");
}

#[test]
fn broken_input_exits_0_and_prints_nothing() {
    let sb = Sandbox::new("ja");
    let missing = json!({"session_id":"s1","transcript_path":"/nonexistent/t.jsonl","prompt_id":"p1"}).to_string();
    for stdin in ["", "not json", missing.as_str()] {
        assert_eq!(sb.hint_raw(stdin, None), "", "{stdin}");
    }
}

#[test]
fn decidle_mode_off_does_nothing() {
    let sb = Sandbox::new("ja");
    let t = sb.transcript(&edits(&sb, 3));
    assert_eq!(sb.hint_raw(&sb.input(&t, "p1"), Some("off")), "");
    assert!(!sb.state_file().exists());
}

#[test]
fn unreadable_state_hands_nothing_over() {
    let sb = Sandbox::new("ja");
    let t = sb.transcript(&edits(&sb, 3));
    // A folder where the state file should be: reading fails for a reason other than "missing".
    fs::create_dir_all(sb.state_file()).unwrap();
    assert_eq!(sb.hint(&t, "p1"), "");
}
