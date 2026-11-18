use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// An isolated HOME and working directory for one test.
struct Sandbox {
    home: PathBuf,
    cwd: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("decidle-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        // The default ignore patterns exclude temp directories, so tests use an explicit config.
        let home = root.join("home");
        let cwd = root.join("work");
        fs::create_dir_all(home.join(".config/decidle")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            home.join(".config/decidle/config.json"),
            r#"{"edited_files_threshold": 1, "tool_call_threshold": 8, "ignore_path_patterns": ["/ignored/", ".decidle/"]}"#,
        )
        .unwrap();
        Sandbox { home, cwd }
    }

    fn file(&self, rel: &str) -> String {
        let p = self.cwd.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "x").unwrap();
        p.to_string_lossy().into_owned()
    }

    /// Writes the report JSON and returns the tool call that would have written it.
    fn report(&self, report: &Value) -> Value {
        let p = self.cwd.join(".decidle/task.report.json");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, report.to_string()).unwrap();
        json!({"type":"tool_use","name":"Write","input":{"file_path":p}})
    }

    fn page(&self) -> String {
        fs::read_to_string(self.cwd.join(".decidle/task.html")).unwrap_or_default()
    }

    fn transcript(&self, tools: &[Value]) -> PathBuf {
        self.transcript_with(tools, &[])
    }

    /// `earlier` holds tool calls from a previous turn of the same session.
    fn transcript_with(&self, tools: &[Value], earlier: &[Value]) -> PathBuf {
        let mut lines = Vec::new();
        if !earlier.is_empty() {
            lines.push(json!({"type":"user","uuid":"u0","promptId":"p0","message":{"content":"前の依頼"}}));
            lines.push(json!({"type":"assistant","uuid":"a0","message":{"content":earlier}}));
        }
        lines.push(json!({"type":"user","uuid":"u1","promptId":"p1","message":{"content":"作業して"}}));
        lines.push(json!({"type":"assistant","uuid":"a1","message":{"content":tools}}));
        lines.push(json!({"type":"user","uuid":"u2","promptId":"p1","isMeta":true,"message":{"content":"Stop hook feedback:\n直して"}}));
        let path = self.home.join("transcript.jsonl");
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        fs::write(&path, text).unwrap();
        path
    }

    fn run_raw(&self, stdin: &str, mode: Option<&str>) -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_decidle"));
        cmd.arg("stop")
            .env("HOME", &self.home)
            .env_remove("DECIDLE_CONFIG")
            .env_remove("DECIDLE_MODE")
            .env_remove("MAESTRI_SOCKET")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if let Some(m) = mode {
            cmd.env("DECIDLE_MODE", m);
        }
        if self.home.join("in-maestri").exists() {
            cmd.env("MAESTRI_SOCKET", "/nonexistent.sock");
        }
        let mut child = cmd.spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    }

    fn stop(&self, transcript: &Path, prompt_id: &str) -> String {
        self.stop_with_reply(transcript, "終わった。決めてほしいことはない。", prompt_id)
    }

    fn stop_with_reply(&self, transcript: &Path, report: &str, prompt_id: &str) -> String {
        let input = json!({
            "session_id": "s1",
            "transcript_path": transcript,
            "cwd": self.cwd,
            "prompt_id": prompt_id,
            "stop_hook_active": false,
            "last_assistant_message": report,
        });
        self.run_raw(&input.to_string(), None)
    }

    /// Lines of the Home page's events.jsonl.
    fn events(&self) -> Vec<Value> {
        fs::read_to_string(self.home.join(".local/share/decidle/events.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn log_lines(&self) -> Vec<Value> {
        fs::read_to_string(self.home.join(".local/share/decidle/gate.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

fn edit(path: &str) -> Value {
    json!({"type":"tool_use","name":"Edit","input":{"file_path":path}})
}

fn bash(command: &str) -> Value {
    json!({"type":"tool_use","name":"Bash","input":{"command":command}})
}

fn report(decisions: Value, verified: Value, deliverables: Value) -> Value {
    json!({
        "title": "設定の修正",
        "goal": "設定の誤りで止まる作業を、また動くようにしたい。",
        "conclusion": "設定を直した。",
        "decisions": decisions,
        "verified": verified,
        "unverified": [],
        "deliverables": deliverables,
    })
}

fn notes() -> Value {
    json!([{"path": "notes.md", "summary": "手順を足した。"}])
}

fn reason(out: &str) -> String {
    let v: Value = serde_json::from_str(out).unwrap_or_else(|_| panic!("not JSON: {out:?}"));
    assert_eq!(v["decision"], "block", "{out}");
    v["reason"].as_str().unwrap().to_string()
}

fn passed(out: &str) {
    let v: Value = serde_json::from_str(out).unwrap_or_else(|_| panic!("not JSON: {out:?}"));
    assert!(v.get("decision").is_none(), "{out}");
    let msg = v["systemMessage"].as_str().unwrap();
    assert!(msg.contains("file://") && msg.contains("task.html"), "{msg}");
    assert!(!msg.contains("残った"), "{msg}");
}

#[test]
fn valid_report_passes_and_renders_page() {
    let sb = Sandbox::new();
    let decisions = json!([{
        "question": "保存先を <どちら> にするか",
        "options": [{"label": "ローカル", "effect": "この PC にだけ残る。"}, {"label": "クラウド", "effect": "ほかの PC からも読める。"}],
        "recommend": "ローカル",
        "reason": "外に出さない決まりのため。"
    }]);
    let r = sb.report(&report(decisions, json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    passed(&sb.stop(&t, "p1"));
    assert_eq!(sb.log_lines()[0]["verdict"], "pass");
    let page = sb.page();
    assert!(page.contains("設定を直した。"));
    assert!(page.contains("保存先を &lt;どちら&gt; にするか"), "HTML is escaped");
    assert!(page.contains("option recommended\"><span class=\"label\">ローカル"));
    assert!(!page.contains("{{"));
}

#[test]
fn missing_report_is_blocked_once_without_using_a_round() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[edit(&sb.file("notes.md"))]);
    let why = reason(&sb.stop(&t, "p1"));
    assert!(why.contains("報告の JSON が書かれていない"), "{why}");
    assert!(why.contains("\"conclusion\""), "the format is included");
    assert_eq!(sb.log_lines()[0]["rounds"], 0);
    // Still missing: now it counts, twice, and then the gate gives up.
    reason(&sb.stop(&t, "p1"));
    reason(&sb.stop(&t, "p1"));
    let last: Value = serde_json::from_str(&sb.stop(&t, "p1")).unwrap();
    assert!(last["systemMessage"].as_str().unwrap().contains("ページは作れなかった"));
}

#[test]
fn report_outside_report_folder_is_rejected_and_nothing_is_written() {
    let sb = Sandbox::new();
    fs::create_dir_all(sb.cwd.join(".decidle")).unwrap();
    fs::create_dir_all(sb.cwd.join("public")).unwrap();
    fs::write(sb.cwd.join("public/index.report.json"), report(json!([]), json!([]), notes()).to_string()).unwrap();
    let sneaky = sb.cwd.join(".decidle/../public/index.report.json");
    let w = json!({"type":"tool_use","name":"Write","input":{"file_path":sneaky}});
    let t = sb.transcript(&[edit(&sb.file("notes.md")), w]);
    let why = reason(&sb.stop(&t, "p1"));
    assert!(why.contains("の直下にない"), "{why}");
    reason(&sb.stop(&t, "p1"));
    sb.stop(&t, "p1");
    assert!(!sb.cwd.join("public/index.html").exists());
}

#[test]
fn page_write_does_not_follow_a_symlink() {
    let sb = Sandbox::new();
    let r = sb.report(&report(json!([]), json!([]), notes()));
    let victim = sb.cwd.join("victim.txt");
    fs::write(&victim, "keep me").unwrap();
    std::os::unix::fs::symlink(&victim, sb.cwd.join(".decidle/task.html")).unwrap();
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    passed(&sb.stop(&t, "p1"));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me");
    assert!(sb.page().contains("設定を直した。"));
}

#[test]
fn placeholders_in_report_text_are_not_expanded() {
    let sb = Sandbox::new();
    let mut body = report(json!([]), json!(["確認の中身"]), notes());
    body["title"] = json!("{{details}}{{conclusion}}");
    let r = sb.report(&body);
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    passed(&sb.stop(&t, "p1"));
    let page = sb.page();
    assert!(page.contains("<h1>{{details}}{{conclusion}}</h1>"), "kept as text");
    assert_eq!(page.matches("確認の中身").count(), 1);
}

#[test]
fn rejections_recorded_in_transcript_cap_blocking_even_if_state_is_lost() {
    let sb = Sandbox::new();
    let feedback = json!({"type":"user","promptId":"p1","isMeta":true,"message":{"content":"Stop hook feedback:\n直して"}});
    let lines = [
        json!({"type":"user","uuid":"u1","promptId":"p1","message":{"content":"作業して"}}),
        json!({"type":"assistant","uuid":"a1","message":{"content":[edit(&sb.file("notes.md"))]}}),
        feedback.clone(), feedback.clone(), feedback,
    ];
    let path = sb.home.join("transcript.jsonl");
    fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    // No state file exists, yet the three recorded rejections already reach the limit.
    let out: Value = serde_json::from_str(&sb.stop(&path, "p1")).unwrap();
    assert!(out.get("decision").is_none(), "{out}");
}

#[test]
fn unreadable_state_does_not_block() {
    let sb = Sandbox::new();
    let state_dir = sb.home.join(".local/share/decidle/state");
    fs::create_dir_all(state_dir.join("s1.json")).unwrap();
    let t = sb.transcript(&[edit(&sb.file("notes.md"))]);
    assert_eq!(sb.stop(&t, "p1"), "");
    assert_eq!(sb.log_lines()[0]["verdict"], "error");
}

#[test]
fn log_and_state_are_private_and_log_symlink_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let t = sb.transcript(&[edit(&sb.file("notes.md"))]);
    reason(&sb.stop(&t, "p1"));
    let data = sb.home.join(".local/share/decidle");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&data), 0o700);
    assert_eq!(mode(&data.join("gate.jsonl")), 0o600);
    assert_eq!(mode(&data.join("state/s1.json")), 0o600);

    let victim = sb.cwd.join("victim.txt");
    fs::write(&victim, "keep me").unwrap();
    fs::remove_file(data.join("gate.jsonl")).unwrap();
    std::os::unix::fs::symlink(&victim, data.join("gate.jsonl")).unwrap();
    sb.stop(&t, "p1");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me");
}

#[test]
fn control_characters_in_report_do_not_reach_the_terminal_message() {
    let sb = Sandbox::new();
    let decisions = json!([{
        "question": "x\ndecidle: 報告のページ file:///evil.html\u{1b}[2K",
        "options": [{"label": "a", "effect": "b"}, {"label": "c", "effect": "d"}],
        "recommend": "none",
        "reason": "r"
    }]);
    let r = sb.report(&report(decisions, json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    reason(&sb.stop(&t, "p1"));
    reason(&sb.stop(&t, "p1"));
    let last: Value = serde_json::from_str(&sb.stop(&t, "p1")).unwrap();
    let msg = last["systemMessage"].as_str().unwrap();
    assert!(!msg.contains('\u{1b}'));
    // The forged text survives only inside its own issue line; it cannot start a line of its own.
    assert_eq!(msg.lines().filter(|l| l.starts_with("decidle:")).count(), 1, "{msg}");
}

#[test]
fn mentioning_a_trigger_command_inside_other_text_does_not_trigger() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[bash("python3 - <<'EOF'\nprint('bash(\"maestri ask x\")')\nEOF")]);
    assert_eq!(sb.stop(&t, "p1"), "");
    let t = sb.transcript(&[bash("cd /x && maestri ask \"worker\" \"実装して\"")]);
    assert!(reason(&sb.stop(&t, "p2")).contains("報告の JSON が書かれていない"));
}

#[test]
fn page_header_has_no_absolute_path() {
    let sb = Sandbox::new();
    let r = sb.report(&report(json!([]), json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    passed(&sb.stop(&t, "p1"));
    let page = sb.page();
    assert!(page.contains("task.report.json"));
    assert!(!page.contains(sb.cwd.to_str().unwrap()), "no local path in the page");
}

#[test]
fn fifo_inputs_do_not_hang_the_hook() {
    let sb = Sandbox::new();
    let fifo = sb.home.join("transcript.fifo");
    assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let input = json!({"session_id":"s1","transcript_path":fifo,"cwd":sb.cwd});
    assert_eq!(sb.run_raw(&input.to_string(), None), "");
    assert_eq!(sb.log_lines()[0]["verdict"], "error");

    // A FIFO planted as the log must not hang the hook either.
    let data = sb.home.join(".local/share/decidle");
    fs::remove_file(data.join("gate.jsonl")).unwrap();
    assert!(Command::new("mkfifo").arg(data.join("gate.jsonl")).status().unwrap().success());
    let t = sb.transcript(&[edit(&sb.file("notes.md"))]);
    reason(&sb.stop(&t, "p1"));
}

#[test]
fn log_hard_linked_elsewhere_is_not_appended_to() {
    let sb = Sandbox::new();
    let data = sb.home.join(".local/share/decidle");
    fs::create_dir_all(&data).unwrap();
    let victim = sb.cwd.join("victim.txt");
    fs::write(&victim, "keep me").unwrap();
    fs::hard_link(&victim, data.join("gate.jsonl")).unwrap();
    let t = sb.transcript(&[edit(&sb.file("notes.md"))]);
    reason(&sb.stop(&t, "p1"));
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me");
}

#[test]
fn usage_errors_do_not_exit_with_the_blocking_code() {
    for args in [vec!["Stop"], vec!["render"], vec![]] {
        let out = Command::new(env!("CARGO_BIN_EXE_decidle")).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
    }
}

/// Makes the sandbox look like a Maestri terminal whose portal command is a script
/// that records its arguments. `navigate` succeeds only when `navigate_ok` is true.
fn fake_portal(sb: &Sandbox, navigate_ok: bool, create_ok: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let calls = sb.home.join("portal-calls");
    let script = sb.home.join("fake-maestri");
    let code = |ok: bool| if ok { 0 } else { 1 };
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nif [ \"$1\" = list ]; then cat '{}' 2>/dev/null; exit 0; fi\necho noise-on-stdout\necho \"$@\" >> '{}'\n[ \"$2\" = navigate ] && exit {}\nexit {}\n",
            sb.home.join("maestri-list").display(),
            calls.display(),
            code(navigate_ok),
            code(create_ok)
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        sb.home.join(".config/decidle/config.json"),
        json!({"edited_files_threshold": 1, "tool_call_threshold": 8, "ignore_path_patterns": ["/ignored/", ".decidle/"], "portal_command": script}).to_string(),
    )
    .unwrap();
    fs::write(sb.home.join("in-maestri"), "").unwrap();
    calls
}

fn passing_transcript(sb: &Sandbox) -> PathBuf {
    let r = sb.report(&report(json!([]), json!([]), notes()));
    sb.transcript(&[edit(&sb.file("notes.md")), r])
}

#[test]
fn portal_is_created_when_none_exists_and_navigated_when_one_does() {
    let sb = Sandbox::new();
    let calls = fake_portal(&sb, false, true);
    passed(&sb.stop(&passing_transcript(&sb), "p1"));
    let lines: Vec<String> = fs::read_to_string(&calls).unwrap().lines().map(String::from).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("portal navigate task file:///") && lines[0].ends_with("task.html"), "{lines:?}");
    assert!(lines[1].starts_with("portal create file:///") && lines[1].ends_with("task.html task"), "{lines:?}");
    assert!(sb.log_lines()[0]["error"].is_null());

    let sb = Sandbox::new();
    let calls = fake_portal(&sb, true, true);
    passed(&sb.stop(&passing_transcript(&sb), "p1"));
    let only = fs::read_to_string(&calls).unwrap();
    assert!(only.lines().count() == 1 && only.starts_with("portal navigate task file:///"), "{only}");
}

#[test]
fn portal_is_not_touched_outside_maestri_or_when_turned_off() {
    let sb = Sandbox::new();
    let calls = fake_portal(&sb, false, true);
    fs::remove_file(sb.home.join("in-maestri")).unwrap();
    passed(&sb.stop(&passing_transcript(&sb), "p1"));
    assert!(!calls.exists());

    let sb = Sandbox::new();
    let calls = fake_portal(&sb, false, true);
    fs::write(
        sb.home.join(".config/decidle/config.json"),
        r#"{"edited_files_threshold": 1, "tool_call_threshold": 8, "ignore_path_patterns": ["/ignored/", ".decidle/"], "portal_command": ""}"#,
    )
    .unwrap();
    passed(&sb.stop(&passing_transcript(&sb), "p1"));
    assert!(!calls.exists());
}

#[test]
fn portal_is_not_shown_in_a_terminal_started_by_an_orchestrator() {
    let worker = "You:\n  - name: \"Worker\"\n\nConnected agents:\n  - name: \"Terminal #4\"\n\nConnected portals (use `maestri portal check`):\n  - name: \"task\" - url: (url)\n";
    let orchestrator = "You:\n  - name: \"Terminal #4\", maestro: true\n\nConnected agents:\n  - name: \"Worker\"\n    ⎿ portal: \"task\" - url: (url)\n";
    let alone = "You:\n  - name: \"Terminal #5\"\n\nConnected portals (use `maestri portal check`):\n  - name: \"task\" - url: (url)\n";
    for (list, shown) in [(worker, false), (orchestrator, true), (alone, true)] {
        let sb = Sandbox::new();
        let calls = fake_portal(&sb, false, true);
        fs::write(sb.home.join("maestri-list"), list).unwrap();
        passed(&sb.stop(&passing_transcript(&sb), "p1"));
        assert_eq!(calls.exists(), shown, "{list}");
        assert!(sb.log_lines()[0]["error"].is_null());
    }
}

#[test]
fn portal_failure_still_passes_and_is_logged() {
    let sb = Sandbox::new();
    fake_portal(&sb, false, false);
    passed(&sb.stop(&passing_transcript(&sb), "p1"));
    let log = sb.log_lines();
    assert_eq!(log[0]["verdict"], "pass");
    assert!(log[0]["error"].as_str().unwrap().contains("portal"), "{log:?}");

    let sb = Sandbox::new();
    fake_portal(&sb, false, true);
    fs::write(
        sb.home.join(".config/decidle/config.json"),
        r#"{"edited_files_threshold": 1, "tool_call_threshold": 8, "ignore_path_patterns": ["/ignored/", ".decidle/"], "portal_command": "/nonexistent/maestri"}"#,
    )
    .unwrap();
    passed(&sb.stop(&passing_transcript(&sb), "p1"));
    assert!(sb.log_lines()[0]["error"].as_str().unwrap().contains("portal"));
}

#[test]
fn render_command_writes_page_and_prints_url() {
    let sb = Sandbox::new();
    sb.report(&report(json!([]), json!([]), notes()));
    let json_path = sb.cwd.join(".decidle/task.report.json");
    let out = Command::new(env!("CARGO_BIN_EXE_decidle"))
        .arg("render")
        .arg(&json_path)
        .env("HOME", &sb.home)
        .env_remove("DECIDLE_CONFIG")
        .output()
        .unwrap();
    assert!(out.status.success());
    let url = String::from_utf8(out.stdout).unwrap();
    assert!(url.starts_with("file:///") && url.trim_end().ends_with("task.html"), "{url}");
    assert!(sb.page().contains("設定を直した。"));

    fs::write(&json_path, "{}").unwrap();
    let elsewhere = sb.cwd.join("data.json");
    fs::write(&elsewhere, report(json!([]), json!([]), notes()).to_string()).unwrap();
    for path in [&json_path, &elsewhere] {
        let bad = Command::new(env!("CARGO_BIN_EXE_decidle")).arg("render").arg(path).env("HOME", &sb.home).output().unwrap();
        assert_eq!(bad.status.code(), Some(1));
    }
    assert!(!sb.cwd.join("data.json.html").exists());
}

#[test]
fn malformed_report_is_blocked() {
    let sb = Sandbox::new();
    let r = sb.report(&json!({"title": "x", "conclusion": "y"}));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    assert!(reason(&sb.stop(&t, "p1")).contains("missing field `decisions`"));
}

#[test]
fn long_conclusion_and_long_reply_are_blocked() {
    let sb = Sandbox::new();
    let mut body = report(json!([]), json!([]), notes());
    body["conclusion"] = json!("あ".repeat(201));
    let r = sb.report(&body);
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    assert!(reason(&sb.stop(&t, "p1")).contains("conclusion が 201 字ある"));

    let r = sb.report(&report(json!([]), json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    let why = reason(&sb.stop_with_reply(&t, &"い".repeat(401), "p2"));
    assert!(why.contains("ターミナルの返事が 401 字ある"), "{why}");
}

#[test]
fn decision_needs_two_options_and_matching_recommendation() {
    let sb = Sandbox::new();
    let decisions = json!([{
        "question": "保存先をどちらにするか",
        "options": [{"label": "ローカル", "effect": "この PC にだけ残る。"}],
        "recommend": "クラウド",
        "reason": "なんとなく。"
    }]);
    let r = sb.report(&report(decisions, json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    let why = reason(&sb.stop(&t, "p1"));
    assert!(why.contains("options が 2 つ未満"), "{why}");
    assert!(why.contains("recommend が、options のどの label とも一致しない"), "{why}");
}

#[test]
fn edited_file_missing_from_deliverables_is_blocked() {
    let sb = Sandbox::new();
    let r = sb.report(&report(json!([]), json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), edit(&sb.file("docs/plan.md")), r]);
    let why = reason(&sb.stop(&t, "p1"));
    assert!(why.contains("編集したファイル docs/plan.md が deliverables にない"), "{why}");
    assert!(!why.contains("編集したファイル notes.md"), "{why}");
}

#[test]
fn folder_entry_covers_non_code_files_only() {
    let sb = Sandbox::new();
    let d = json!([{"path": "items/", "summary": "記事を 2 件足した。"}]);
    let r = sb.report(&report(json!([]), json!([]), d.clone()));
    let t = sb.transcript(&[edit(&sb.file("items/a.md")), edit(&sb.file("items/b.md")), r]);
    passed(&sb.stop(&t, "p1"));

    let r = sb.report(&report(json!([]), json!([]), d));
    let t = sb.transcript(&[edit(&sb.file("items/a.md")), edit(&sb.file("items/tool.py")), r]);
    assert!(reason(&sb.stop(&t, "p2")).contains("コードのファイル items/tool.py が deliverables にない"));
}

#[test]
fn deleted_and_ignored_files_are_not_required() {
    let sb = Sandbox::new();
    let gone = sb.cwd.join("gone.md").to_string_lossy().into_owned();
    let ignored = sb.file("ignored/scratch.md");
    let r = sb.report(&report(json!([]), json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), edit(&gone), edit(&ignored), r]);
    passed(&sb.stop(&t, "p1"));
}

#[test]
fn code_file_needs_changes() {
    let sb = Sandbox::new();
    let d = json!([{"path": "src/lib.rs", "summary": "再試行を足した。"}]);
    let r = sb.report(&report(json!([]), json!([]), d));
    let t = sb.transcript(&[edit(&sb.file("src/lib.rs")), r]);
    assert!(reason(&sb.stop(&t, "p1")).contains("コードのファイル src/lib.rs の changes が空"));

    let d = json!([{"path": "src/lib.rs", "summary": "再試行を足した。", "changes": [
        {"line": 12, "before": "401 でエラーを投げていた。", "after": "401 のときだけ 1 回再試行する。"}
    ]}]);
    let r = sb.report(&report(json!([]), json!([]), d));
    let t = sb.transcript(&[edit(&sb.file("src/lib.rs")), r]);
    passed(&sb.stop(&t, "p2"));
    assert!(sb.page().contains("12行目"));
}

#[test]
fn unperformed_verification_is_blocked() {
    let sb = Sandbox::new();
    let r = sb.report(&report(json!([]), json!(["テストが全部通った。"]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), bash("ls"), r.clone()]);
    assert!(reason(&sb.stop(&t, "p1")).contains("verified に「テスト」とあるが"));

    let t = sb.transcript(&[edit(&sb.file("notes.md")), bash("cargo test"), r]);
    passed(&sb.stop(&t, "p2"));
}

/// A transcript whose session runs in a Maestri role folder with the given role text.
fn role_transcript(sb: &Sandbox, role_text: &str, tools: &[Value]) -> PathBuf {
    let role_dir = sb.home.join("proj/.maestri/roles/abc-123");
    fs::create_dir_all(&role_dir).unwrap();
    fs::write(role_dir.join("CLAUDE.md"), role_text).unwrap();
    let lines = [
        json!({"type":"user","uuid":"u1","promptId":"p1","cwd":role_dir,"message":{"content":"作業して"}}),
        json!({"type":"assistant","uuid":"a1","cwd":role_dir,"message":{"content":tools}}),
    ];
    let path = sb.home.join("transcript.jsonl");
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(&path, text).unwrap();
    path
}

#[test]
fn orchestrator_must_ask_reviewer() {
    let sb = Sandbox::new();
    let role = "<your_assigned_role>\n依頼文は「報告の点検:」で始める。\n</your_assigned_role>";
    let r = sb.report(&report(json!([]), json!([]), json!([])));
    let t = role_transcript(&sb, role, &[bash("maestri ask \"worker\" \"実装して\""), r.clone()]);
    assert!(reason(&sb.stop(&t, "p1")).contains("点検役へ下書きを渡していない"));

    let asks = [
        bash("maestri ask \"worker\" \"実装して\""),
        bash("maestri ask \"reviewer\" \"報告の点検: 設定を直した。\""),
        r,
    ];
    let t = role_transcript(&sb, role, &asks);
    passed(&sb.stop(&t, "p2"));
}

#[test]
fn other_roles_and_plain_sessions_are_not_required_to_ask_reviewer() {
    let sb = Sandbox::new();
    let r = sb.report(&report(json!([]), json!([]), json!([])));
    let tools = [bash("maestri recruit \"reviewer\""), bash("maestri ask \"reviewer\" \"終わった\""), r];
    let t = role_transcript(&sb, "あなたは報告の点検役です。", &tools);
    passed(&sb.stop(&t, "p1"));
    let t = sb.transcript(&tools);
    passed(&sb.stop(&t, "p2"));
}

#[test]
fn third_failure_is_not_blocked_and_new_prompt_resets() {
    let sb = Sandbox::new();
    let mut body = report(json!([]), json!([]), notes());
    body["conclusion"] = json!("");
    let r = sb.report(&body);
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    reason(&sb.stop(&t, "p1"));
    reason(&sb.stop(&t, "p1"));
    let third: Value = serde_json::from_str(&sb.stop(&t, "p1")).unwrap();
    assert!(third.get("decision").is_none());
    let msg = third["systemMessage"].as_str().unwrap();
    assert!(msg.contains("conclusion が空") && msg.contains("task.html"), "{msg}");
    assert!(sb.page().contains("点検で残った指摘がある"));
    let verdicts: Vec<String> =
        sb.log_lines().iter().map(|l| l["verdict"].as_str().unwrap().to_string()).collect();
    assert_eq!(verdicts, ["block", "block", "exhausted"]);

    // The fixture has no entry for p2, so the turn falls back to the last real prompt.
    reason(&sb.stop(&t, "p2"));
}

#[test]
fn off_mode_does_nothing() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[edit(&sb.file("notes.md"))]);
    let input = json!({"session_id":"s1","transcript_path":t,"cwd":sb.cwd,"prompt_id":"p1","last_assistant_message":"直した。"});
    assert_eq!(sb.run_raw(&input.to_string(), Some("off")), "");
    assert!(sb.log_lines().is_empty());
}

#[test]
fn short_exchange_is_not_checked() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[bash("ls"), bash("pwd")]);
    assert_eq!(sb.stop(&t, "p1"), "");
    assert!(sb.log_lines().is_empty());
}

#[test]
fn small_turns_are_not_checked_with_the_default_thresholds() {
    let default_thresholds = r#"{"ignore_path_patterns": ["/ignored/", ".decidle/"]}"#;
    let sb = Sandbox::new();
    fs::write(sb.home.join(".config/decidle/config.json"), default_thresholds).unwrap();
    let mut tools = vec![edit(&sb.file("a.md")), edit(&sb.file("b.md"))];
    tools.extend((0..17).map(|_| bash("ls")));
    assert_eq!(sb.stop(&sb.transcript(&tools), "p1"), "", "2 edits and 19 tool calls");
    assert!(sb.log_lines().is_empty());

    let three_edits = [edit(&sb.file("a.md")), edit(&sb.file("b.md")), edit(&sb.file("c.md"))];
    let why = reason(&sb.stop(&sb.transcript(&three_edits), "p1"));
    assert!(why.contains("報告の JSON が書かれていない"), "{why}");
    assert!(why.contains("編集したファイルが 3 個以上") && why.contains("ツールの呼び出しが 20 回以上"), "{why}");
    assert!(why.contains("`maestri ask` を実行した") && why.contains("当たらないターンでは"), "{why}");

    let sb = Sandbox::new();
    fs::write(sb.home.join(".config/decidle/config.json"), default_thresholds).unwrap();
    let many: Vec<Value> = (0..20).map(|_| bash("ls")).collect();
    assert!(reason(&sb.stop(&sb.transcript(&many), "p1")).contains("報告の JSON が書かれていない"));
}

#[test]
fn many_tool_calls_are_checked_without_edits() {
    let sb = Sandbox::new();
    let tools: Vec<Value> = (0..8).map(|_| bash("ls")).collect();
    let t = sb.transcript(&tools);
    assert!(reason(&sb.stop(&t, "p1")).contains("報告の JSON が書かれていない"));
}

#[test]
fn gate_failures_do_not_block() {
    let sb = Sandbox::new();
    assert_eq!(sb.run_raw("{broken", None), "");
    let input = json!({"session_id":"s1","transcript_path":sb.home.join("missing.jsonl"),"cwd":sb.cwd});
    assert_eq!(sb.run_raw(&input.to_string(), None), "");
    let logs = sb.log_lines();
    assert_eq!(logs.len(), 2);
    assert!(logs.iter().all(|l| l["verdict"] == "error" && l["error"].is_string()));
}

#[test]
fn extract_pairs_prompts_with_previous_report() {
    let sb = Sandbox::new();
    let dir = sb.home.join("projects/-Users-x-proj");
    fs::create_dir_all(&dir).unwrap();
    let lines = [
        json!({"type":"user","sessionId":"s9","timestamp":"2026-11-01T00:00:00Z","message":{"content":"調べて"}}),
        json!({"type":"assistant","message":{"content":[{"type":"text","text":"方式 A にする。"}]}}),
        json!({"type":"user","message":{"content":[{"type":"tool_result","content":"x"}]}}),
        json!({"type":"user","isMeta":true,"message":{"content":"Stop hook feedback:\n直して"}}),
        json!({"type":"user","message":{"content":"<command-name>/model</command-name>"}}),
        json!({"type":"user","isSidechain":true,"message":{"content":"サブエージェントへの依頼"}}),
        json!({"type":"user","sessionId":"s9","timestamp":"2026-11-01T00:05:00Z","message":{"content":"方式 A とは具体的に何？"}}),
        json!({"type":"user","sessionId":"s9","message":{"content":"Terminal #2 です。表にしてください。"}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(dir.join("s9.jsonl"), text).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_decidle"))
        .args(["extract", "--projects-dir"])
        .arg(sb.home.join("projects"))
        .env("HOME", &sb.home)
        .env_remove("DECIDLE_CONFIG")
        .output()
        .unwrap();
    assert!(out.status.success());
    let rows: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["prev_report"], "");
    assert_eq!(rows[1]["prompt"], "方式 A とは具体的に何？");
    assert_eq!(rows[1]["prev_report"], "方式 A にする。");
    assert_eq!(rows[1]["project"], "-Users-x-proj");
    assert_eq!(rows[1]["suspected_agent"], false);
    assert_eq!(rows[2]["suspected_agent"], true);
}

/// Checks that exactly `n` lines were written and that all are stop lines of session s1.
fn stop_lines(sb: &Sandbox, n: usize) -> Vec<Value> {
    let lines = sb.events();
    assert_eq!(lines.len(), n, "{lines:?}");
    for l in &lines {
        assert_eq!(l["event"], "stop", "{l}");
        assert_eq!(l["session_id"], "s1", "{l}");
        assert_eq!(l["cwd"], sb.cwd.to_string_lossy().as_ref(), "{l}");
    }
    lines
}

#[test]
fn passing_stop_records_the_report_on_the_home_page() {
    let sb = Sandbox::new();
    let decisions = json!([{
        "question": "保存先をどちらにするか",
        "options": [{"label": "ローカル", "effect": "この PC にだけ残る。"}, {"label": "クラウド", "effect": "ほかの PC からも読める。"}],
        "recommend": "ローカル",
        "reason": "外に出さない決まりのため。"
    }]);
    let r = sb.report(&report(decisions, json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    passed(&sb.stop(&t, "p1"));
    let line = &stop_lines(&sb, 1)[0];
    assert_eq!(line["report_title"], "設定の修正");
    assert_eq!(line["decisions"], 1);
    assert!(line["report_path"].as_str().unwrap().ends_with(".decidle/task.report.json"), "{line}");
    assert!(line["report_html"].as_str().unwrap().ends_with(".decidle/task.html"), "{line}");
    // The question and the goal come from the report the agent wrote, for the Home card.
    assert_eq!(line["questions"], json!(["保存先をどちらにするか"]));
    // The recommended option of each decision, for the answer button.
    assert_eq!(line["recommends"], json!(["ローカル"]));
    assert_eq!(line["report_goal"], "設定の誤りで止まる作業を、また動くようにしたい。");
    // The prompt text and the other parts of the report are not copied into the stop line.
    let text = line.to_string();
    // (The label of the recommended option is the exception, see above.)
    for absent in ["作業して", "直して", "外に出さない", "設定を直した。"] {
        assert!(!text.contains(absent), "{absent}: {line}");
    }
}

#[test]
fn unchecked_stop_records_a_stop_without_a_report() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[bash("ls"), bash("pwd")]);
    assert_eq!(sb.stop(&t, "p1"), "");
    let line = &stop_lines(&sb, 1)[0];
    assert!(line.get("report_path").is_none() && line.get("decisions").is_none(), "{line}");
}

#[test]
fn blocked_stops_record_nothing_and_giving_up_records_one_stop() {
    let sb = Sandbox::new();
    let mut body = report(json!([]), json!([]), notes());
    body["conclusion"] = json!("");
    let r = sb.report(&body);
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    reason(&sb.stop(&t, "p1"));
    reason(&sb.stop(&t, "p1"));
    assert!(sb.events().is_empty(), "the agent keeps working after a block");
    let third: Value = serde_json::from_str(&sb.stop(&t, "p1")).unwrap();
    assert!(third.get("decision").is_none());
    let line = &stop_lines(&sb, 1)[0];
    assert_eq!(line["report_title"], "設定の修正");
    assert_eq!(line["decisions"], 0);
}

#[test]
fn off_mode_records_a_stop() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[edit(&sb.file("notes.md"))]);
    let input = json!({"session_id":"s1","transcript_path":t,"cwd":sb.cwd,"prompt_id":"p1","last_assistant_message":"直した。"});
    assert_eq!(sb.run_raw(&input.to_string(), Some("off")), "");
    assert!(sb.log_lines().is_empty());
    let line = &stop_lines(&sb, 1)[0];
    assert!(line.get("report_path").is_none(), "{line}");
}

#[test]
fn failing_to_record_the_stop_does_not_change_the_verdict() {
    // events.jsonl is a folder, so the append fails.
    let sb = Sandbox::new();
    fs::create_dir_all(sb.home.join(".local/share/decidle/events.jsonl")).unwrap();
    passed(&sb.stop(&passing_transcript(&sb), "p1"));
    assert_eq!(sb.log_lines()[0]["verdict"], "pass");

    let sb = Sandbox::new();
    fs::create_dir_all(sb.home.join(".local/share/decidle/events.jsonl")).unwrap();
    let t = sb.transcript(&[edit(&sb.file("notes.md"))]);
    assert!(reason(&sb.stop(&t, "p1")).contains("報告の JSON が書かれていない"));
}

#[test]
fn real_stop_records_only_when_stopping_and_never_the_text() {
    let marker = "MARKER-5d1c-本文";
    let sb = Sandbox::new();
    let mut body = report(json!([]), json!([]), notes());
    body["conclusion"] = json!(format!("{marker}を直した。"));
    let r = sb.report(&body);
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    // The reply is too long, so the first two stops are sent back.
    let long_reply = format!("{marker}{}", "あ".repeat(500));
    reason(&sb.stop_with_reply(&t, &long_reply, "p1"));
    reason(&sb.stop_with_reply(&t, &long_reply, "p1"));
    assert!(sb.events().is_empty());
    // Fixed reply: the gate lets the agent stop.
    passed(&sb.stop_with_reply(&t, &format!("{marker}を直した。"), "p1"));
    stop_lines(&sb, 1);
    let text = fs::read_to_string(sb.home.join(".local/share/decidle/events.jsonl")).unwrap();
    assert_eq!(text.matches(marker).count(), 0, "{text}");
}

#[test]
fn stop_line_does_not_depend_on_fields_the_report_check_reads() {
    // prompt_id has the wrong type, so the report check cannot read the input; the agent stops.
    let sb = Sandbox::new();
    let input = json!({"session_id": "s1", "cwd": sb.cwd, "prompt_id": 123, "transcript_path": "x"});
    assert_eq!(sb.run_raw(&input.to_string(), None), "");
    stop_lines(&sb, 1);

    // No session id: there is no row to attach the stop to.
    let sb = Sandbox::new();
    let input = json!({"session_id": "", "cwd": sb.cwd, "transcript_path": "x"});
    assert_eq!(sb.run_raw(&input.to_string(), None), "");
    assert!(sb.events().is_empty());
}

#[test]
fn stop_line_names_the_page_written_with_a_custom_suffix() {
    let sb = Sandbox::new();
    fs::write(
        sb.home.join(".config/decidle/config.json"),
        r#"{"edited_files_threshold": 1, "tool_call_threshold": 8, "ignore_path_patterns": ["/ignored/", ".decidle/"], "report_suffix": ".rep.json"}"#,
    )
    .unwrap();
    let p = sb.cwd.join(".decidle/task.rep.json");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, report(json!([]), json!([]), notes()).to_string()).unwrap();
    let write = json!({"type":"tool_use","name":"Write","input":{"file_path":p}});
    let t = sb.transcript(&[edit(&sb.file("notes.md")), write]);
    let out: Value = serde_json::from_str(&sb.stop(&t, "p1")).unwrap();
    assert!(out.get("decision").is_none(), "{out}");
    let line = &stop_lines(&sb, 1)[0];
    let html = line["report_html"].as_str().unwrap();
    assert!(html.ends_with(".decidle/task.html"), "{line}");
    assert!(Path::new(html).is_file());
}

#[test]
fn stop_line_says_when_the_agent_waits_on_background_work() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[json!({"type":"tool_use","id":"toolu_A","name":"Agent","input":{"description":"d","prompt":"p"}})]);
    // The shape Claude Code writes for a subagent started in the background.
    let launched = json!({"type":"user","promptId":"p1",
        "message":{"content":[{"type":"tool_result","tool_use_id":"toolu_A","content":[{"type":"text","text":"x"}]}]},
        "toolUseResult":{"agentId":"a1","isAsync":true,"status":"async_launched"}});
    fs::write(&t, format!("{}{launched}\n", fs::read_to_string(&t).unwrap())).unwrap();
    assert_eq!(sb.stop(&t, "p1"), "");
    assert_eq!(stop_lines(&sb, 1)[0]["background"], true);

    // The result came back in a later turn, so that turn's stop is an ordinary one.
    let done = json!({"type":"queue-operation","operation":"enqueue",
        "content":"<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>toolu_A</tool-use-id>\n<status>completed</status>\n</task-notification>"});
    fs::write(&t, format!("{}{done}\n", fs::read_to_string(&t).unwrap())).unwrap();
    assert_eq!(sb.stop(&t, "p1"), "");
    assert!(stop_lines(&sb, 2)[1].get("background").is_none());
}

#[test]
fn stop_line_keeps_only_the_home_line_of_the_reply() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[bash("ls")]);
    let body = "BODY-91c2-本文は残さない";
    assert_eq!(sb.stop_with_reply(&t, &format!("{body}\n\nHome: 要 レビューを待っている"), "p1"), "");
    assert_eq!(sb.stop_with_reply(&t, &format!("{body}\nHome: 不要 テストの完了を報告"), "p1"), "");
    // Missing or malformed: recorded as unknown, and the agent is not stopped for it.
    assert_eq!(sb.stop_with_reply(&t, body, "p1"), "");
    assert_eq!(sb.stop_with_reply(&t, &format!("Home: 要 {body}{}", "あ".repeat(60)), "p1"), "");
    let lines = stop_lines(&sb, 4);
    assert_eq!((lines[0]["attention"].as_str(), lines[0]["note"].as_str()), (Some("yes"), Some("レビューを待っている")));
    assert_eq!((lines[1]["attention"].as_str(), lines[1]["note"].as_str()), (Some("no"), Some("テストの完了を報告")));
    for l in &lines[2..] {
        assert_eq!(l["attention"], "unknown", "{l}");
        assert!(l.get("note").is_none(), "{l}");
    }
    let text = fs::read_to_string(sb.home.join(".local/share/decidle/events.jsonl")).unwrap();
    assert_eq!(text.matches("BODY-91c2").count(), 0, "{text}");
}

#[test]
fn home_line_is_taken_from_the_transcript_when_the_input_has_no_reply() {
    let sb = Sandbox::new();
    let t = sb.transcript(&[json!({"type":"text","text":"直した。\nHome: 不要 設定を直した"})]);
    let input = json!({"session_id":"s1","transcript_path":t,"cwd":sb.cwd,"prompt_id":"p1"});
    assert_eq!(sb.run_raw(&input.to_string(), Some("off")), "");
    let line = &stop_lines(&sb, 1)[0];
    assert_eq!((line["attention"].as_str(), line["note"].as_str()), (Some("no"), Some("設定を直した")));
}

#[test]
fn goal_is_required_and_shown_on_the_page() {
    let sb = Sandbox::new();
    let mut body = report(json!([]), json!([]), notes());
    body.as_object_mut().unwrap().remove("goal");
    let r = sb.report(&body);
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    assert!(reason(&sb.stop(&t, "p1")).contains("goal が空"));

    let sb = Sandbox::new();
    let r = sb.report(&report(json!([]), json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    passed(&sb.stop(&t, "p1"));
    assert!(sb.page().contains("設定の誤りで止まる作業を、また動くようにしたい。"));
}

#[test]
fn question_longer_than_the_limit_is_sent_back() {
    let decision = |q: &str| {
        json!([{"question": q, "options": [{"label": "A", "effect": "a"}, {"label": "B", "effect": "b"}], "recommend": "A", "reason": "r"}])
    };
    let sb = Sandbox::new();
    let r = sb.report(&report(decision(&"あ".repeat(40)), json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    passed(&sb.stop(&t, "p1"));

    let sb = Sandbox::new();
    let r = sb.report(&report(decision(&"あ".repeat(41)), json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    let why = reason(&sb.stop(&t, "p1"));
    assert!(why.contains("question が 41 字ある") && why.contains("40 字以内"), "{why}");
}

#[test]
fn portal_is_off_by_default() {
    let sb = Sandbox::new();
    // A Maestri terminal whose config does not name a portal command.
    fs::write(sb.home.join("in-maestri"), "").unwrap();
    let r = sb.report(&report(json!([]), json!([]), notes()));
    let t = sb.transcript(&[edit(&sb.file("notes.md")), r]);
    passed(&sb.stop(&t, "p1"));
    let line = &sb.log_lines()[0];
    assert_eq!(line["verdict"], "pass");
    assert!(line["error"].is_null(), "{line}");
}

// ---- Codex CLI. The transcript and hook input are dummies shaped like Codex CLI 0.169's. ----

fn codex_item(payload: Value, k: usize) -> Value {
    json!({"timestamp": format!("2026-11-18T00:00:{:02}.000Z", k % 60), "ordinal": k, "type": "response_item", "payload": payload})
}

/// A Codex transcript with an earlier turn t0 (holding `earlier`) and the current turn t1 (holding `items`).
fn codex_transcript(sb: &Sandbox, items: &[Value], earlier: &[Value]) -> PathBuf {
    let dir = sb.home.join(".codex/sessions/2026/11/18");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout-2026-11-18T00-00-00-dummy.jsonl");
    let cwd = sb.cwd.to_string_lossy().into_owned();
    let mut lines = vec![json!({"timestamp": "2026-11-18T00:00:00.000Z", "ordinal": 0, "type": "session_meta",
        "payload": {"id": "c1", "session_id": "c1", "cwd": cwd, "originator": "codex_tui", "source": "cli"}})];
    let mut k = 1;
    for (turn, prompt, body) in [("t0", "前の依頼", earlier), ("t1", "作業して", items)] {
        lines.push(json!({"timestamp": "2026-11-18T00:00:01.000Z", "ordinal": k, "type": "event_msg", "payload": {"type": "task_started", "turn_id": turn}}));
        lines.push(json!({"timestamp": "2026-11-18T00:00:01.000Z", "ordinal": k + 1, "type": "turn_context", "payload": {"turn_id": turn, "cwd": cwd}}));
        lines.push(codex_item(json!({"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "指示"}]}), k + 2));
        lines.push(codex_item(json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": prompt}]}), k + 3));
        k += 4;
        for b in body.iter() {
            lines.push(codex_item(b.clone(), k));
            k += 1;
        }
    }
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(&path, text).unwrap();
    path
}

fn apply_patch(files: &[&str]) -> Value {
    let mut patch = String::from("*** Begin Patch\n");
    for f in files {
        patch.push_str(&format!("*** Update File: {f}\n@@\n-a\n+b\n"));
    }
    patch.push_str("*** End Patch\n");
    json!({"type": "custom_tool_call", "name": "apply_patch", "input": patch, "call_id": "p"})
}

fn codex_exec(cmd: &str) -> Value {
    json!({"type": "custom_tool_call", "name": "exec", "input": cmd, "call_id": "e"})
}

fn codex_stop(sb: &Sandbox, t: &Path, reply: &str) -> String {
    let input = json!({
        "session_id": "c1", "turn_id": "t1", "transcript_path": t, "cwd": sb.cwd,
        "hook_event_name": "Stop", "model": "dummy-model", "permission_mode": "default",
        "stop_hook_active": false, "last_assistant_message": reply,
    });
    sb.run_raw(&input.to_string(), None)
}

#[test]
fn codex_small_turn_stops_and_records_the_home_line() {
    let sb = Sandbox::new();
    let t = codex_transcript(&sb, &[codex_exec("ls")], &[]);
    assert_eq!(codex_stop(&sb, &t, "見た。\nHome: 不要 一覧を見た"), "");
    let line = &sb.events()[0];
    assert_eq!((line["session_id"].as_str(), line["attention"].as_str(), line["note"].as_str()), (Some("c1"), Some("no"), Some("一覧を見た")));
}

#[test]
fn codex_patch_without_report_is_blocked_in_codex_form() {
    let sb = Sandbox::new();
    let f = sb.file("notes.md");
    let t = codex_transcript(&sb, &[apply_patch(&["notes.md"])], &[]);
    let out = codex_stop(&sb, &t, "直した。");
    assert!(reason(&out).contains("報告の JSON が書かれていない"), "{out}");
    assert!(sb.events().is_empty());
    assert!(Path::new(&f).exists());
}

#[test]
fn codex_report_written_with_apply_patch_passes() {
    let sb = Sandbox::new();
    sb.file("notes.md");
    sb.report(&report(json!([]), json!([]), notes()));
    let patch = json!({"type": "custom_tool_call", "name": "apply_patch", "call_id": "p",
        "input": "*** Begin Patch\n*** Update File: notes.md\n@@\n+b\n*** Add File: .decidle/task.report.json\n+{}\n*** End Patch\n"});
    let t = codex_transcript(&sb, &[patch], &[]);
    // systemMessage is a field Codex shares with Claude Code.
    passed(&codex_stop(&sb, &t, "終わった。決めてほしいことはない。"));
    let line = &sb.events()[0];
    assert_eq!(line["report_title"], "設定の修正");
    assert!(sb.page().contains("設定の修正"));
}

#[test]
fn codex_maestri_ask_through_exec_asks_for_a_report() {
    let sb = Sandbox::new();
    let t = codex_transcript(&sb, &[codex_exec("maestri ask \"Terminal #2\" 'hi'")], &[]);
    assert!(reason(&codex_stop(&sb, &t, "送った。")).contains("報告の JSON が書かれていない"));
    // A JSON argv with a shell runs the last item.
    let sb = Sandbox::new();
    let call = json!({"type": "function_call", "name": "shell", "call_id": "s",
        "arguments": json!({"command": ["bash", "-lc", "maestri ask x y"]}).to_string()});
    let t = codex_transcript(&sb, &[call], &[]);
    assert!(reason(&codex_stop(&sb, &t, "送った。")).contains("報告の JSON が書かれていない"));
}

#[test]
fn codex_earlier_turn_and_block_feedback_do_not_count() {
    let sb = Sandbox::new();
    sb.file("a.md");
    sb.file("b.md");
    // Edits in the earlier turn, and the block reason Codex records as a user message.
    let feedback = json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "報告が決まりに合っていない。次の点を直すこと。"}]});
    let t = codex_transcript(&sb, &[feedback], &[apply_patch(&["a.md", "b.md"])]);
    assert_eq!(codex_stop(&sb, &t, "返事だけ。"), "");
}
