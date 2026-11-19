//! The terminal a session runs in: what the start hook records, what a start line may carry,
//! and the one line the answer button types.

use decidle::home::state::Session;
use decidle::home::term::{self, Recorded};

fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
}

#[test]
fn start_records_cmux_or_herdr_from_the_environment() {
    assert_eq!(term::from_env(env(&[])), None);
    assert_eq!(
        term::from_env(env(&[("CMUX_SURFACE_ID", "SURF-1"), ("CMUX_WORKSPACE_ID", "WS-1")])),
        Some(Recorded { kind: "cmux", id: "SURF-1".into(), workspace: Some("WS-1".into()) })
    );
    // herdr inside a cmux terminal: the pane is the one to address.
    assert_eq!(
        term::from_env(env(&[("CMUX_SURFACE_ID", "SURF-1"), ("HERDR_PANE_ID", "w1:p2")])),
        Some(Recorded { kind: "herdr", id: "w1:p2".into(), workspace: None })
    );
    // An app terminal: hc. A cmux surface left in the environment of the shell that started
    // herdr-core does not win over it, and herdr run inside the app terminal does.
    assert_eq!(term::from_env(env(&[("HC_TERMINAL_ID", "t3")])), Some(Recorded { kind: "hc", id: "t3".into(), workspace: None }));
    assert_eq!(
        term::from_env(env(&[("CMUX_SURFACE_ID", "SURF-1"), ("CMUX_WORKSPACE_ID", "WS-1"), ("HC_TERMINAL_ID", "t3")])),
        Some(Recorded { kind: "hc", id: "t3".into(), workspace: None })
    );
    assert_eq!(
        term::from_env(env(&[("HC_TERMINAL_ID", "t3"), ("HERDR_PANE_ID", "w1:p2")])),
        Some(Recorded { kind: "herdr", id: "w1:p2".into(), workspace: None })
    );
    // An hc terminal on another machine is not recorded.
    assert_eq!(term::from_env(env(&[("HC_TERMINAL_ID", "host/t1")])), None);
    // A value that could be read as more than one argument is not recorded.
    assert_eq!(term::from_env(env(&[("CMUX_SURFACE_ID", "a b")])), None);
    assert_eq!(term::from_env(env(&[("CMUX_SURFACE_ID", "$(whoami)")])), None);
}

#[test]
fn a_start_line_with_an_unusable_terminal_reads_as_none() {
    let s = |v: &str| Some(v.to_string());
    let (id, ws) = ("11111111-2222-3333-4444-555555555555", "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    assert_eq!(term::checked(s("cmux"), s(id), s(ws)), (s("cmux"), s(id), s(ws)));
    assert_eq!(term::checked(s("herdr"), s("w1:p2"), None), (s("herdr"), s("w1:p2"), None));
    // cmux would read these as an index or a ref, which can come to name another terminal.
    for bad in ["1", "surface:2", "AB-1", "--force", "a;b"] {
        assert_eq!(term::checked(s("cmux"), s(bad), s(ws)), (None, None, None), "{bad}");
    }
    // Without its workspace cmux would fall back to the server's own.
    assert_eq!(term::checked(s("cmux"), s(id), None), (None, None, None));
    assert_eq!(term::checked(s("cmux"), s(id), s("2")), (None, None, None));
    assert_eq!(term::checked(s("iterm"), s(id), s(ws)), (None, None, None));
    assert_eq!(term::checked(s("hc"), s("t3"), None), (s("hc"), s("t3"), None));
    for bad in ["host/t1", "--force", "a b", ""] {
        assert_eq!(term::checked(s("hc"), s(bad), None), (None, None, None), "{bad}");
    }
    assert_eq!(term::checked(s("cmux"), None, None), (None, None, None));
}

fn stopped(questions: &[&str], recommends: &[&str]) -> Session {
    Session {
        questions: questions.iter().map(|q| q.to_string()).collect(),
        recommends: recommends.iter().map(|q| q.to_string()).collect(),
        term: Some("cmux".into()),
        term_id: Some("AB-1".into()),
        ..Session::default()
    }
}

#[test]
fn the_answer_is_built_from_questions_and_recommended_options_only() {
    assert_eq!(
        term::answer_text(&stopped(&["保存先をどちらにするか?", "ポートを変えるか"], &["ローカル", "7878 のまま"])).as_deref(),
        Some("回答: 保存先をどちらにするかは「ローカル」で進めて。 ポートを変えるかは「7878 のまま」で進めて。")
    );
    assert_eq!(term::answer_text(&stopped(&[], &[])), None);
    assert_eq!(term::answer_text(&stopped(&["問い"], &[""])), None);
    assert_eq!(term::answer_text(&stopped(&["問い", "問い2"], &["勧め"])), None);
    for bad in ["$(whoami)", "a`b`", "a; b", "a | b", "a\\n", "a > b", "it's", "!!"] {
        assert_eq!(term::answer_text(&stopped(&[bad], &["勧め"])), None, "{bad}");
        assert_eq!(term::answer_text(&stopped(&["問い"], &[bad])), None, "{bad}");
    }
}

#[test]
fn the_host_app_is_a_plain_bundle_id_or_nothing() {
    assert_eq!(term::host_app(env(&[("__CFBundleIdentifier", "com.mitchellh.ghostty")])).as_deref(), Some("com.mitchellh.ghostty"));
    assert_eq!(term::host_app(env(&[])), None);
    assert_eq!(term::host_app(env(&[("__CFBundleIdentifier", "-a Calculator")])), None);
    assert_eq!(term::host_app(env(&[("__CFBundleIdentifier", "a/b")])), None);
    assert_eq!(term::raise_command("com.x.y"), vec!["open", "-b", "com.x.y"]);
}

#[test]
fn the_answer_never_starts_with_a_word_the_agent_wrote() {
    let text = term::answer_text(&stopped(&["git push するか"], &["する"])).unwrap();
    assert!(text.starts_with("回答: "), "{text}");
}

#[test]
fn an_hc_terminal_is_focused_by_the_app_and_answered_by_hc_send_without_flags() {
    let s = Session { term: Some("hc".into()), term_id: Some("t3".into()), ..stopped(&["問い"], &["勧め"]) };
    let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    assert_eq!(term::focus_commands(&s), Some(vec![strs(&["herdr-app", "do", "pane.focus", "--terminal", "t3"])]));
    assert_eq!(term::answer_commands(&s, "回答: x"), Some(vec![strs(&["hc", "send", "t3", "回答: x"])]));
    assert_eq!(term::occupant_command(&s), Some(strs(&["hc", "get", "t3"])));
}

#[test]
fn an_hc_terminal_holds_the_session_only_while_its_foreground_is_the_recorded_pid() {
    let s = Session { term: Some("hc".into()), term_id: Some("t3".into()), pid: Some(4242), ..Session::default() };
    let get = |pid: i64| serde_json::json!({"address": "t3", "foreground": {"name": "claude", "pid": pid}, "pid": 100}).to_string();
    assert!(term::occupant_is(&get(4242), &s));
    // Claude Code ended and the shell is in front, or another program.
    assert!(!term::occupant_is(&get(100), &s));
    assert!(!term::occupant_is(r#"{"address":"t3","pid":4242}"#, &s));
    assert!(!term::occupant_is("not json", &s));
    // Without a recorded pid nothing can match.
    let unrecorded = Session { pid: None, ..s.clone() };
    assert!(!term::occupant_is(r#"{"foreground":{"pid":null}}"#, &unrecorded));
}

#[test]
fn only_one_single_choice_question_in_an_hc_terminal_can_be_pressed() {
    use decidle::home::state::{Question, St};
    let q = |n: usize, multi: bool| Question { question: "q".into(), header: "h".into(), options: (0..n).map(|i| i.to_string()).collect(), multi };
    let s = |term: &str, ask: Vec<Question>| Session {
        st: St::Asking,
        term: Some(term.into()),
        term_id: Some("t3".into()),
        ask,
        ..Session::default()
    };
    assert!(term::choosable(&s("hc", vec![q(1, false)])));
    assert!(term::choosable(&s("hc", vec![q(8, false)])));
    assert!(!term::choosable(&s("hc", vec![q(0, false)])));
    assert!(!term::choosable(&s("hc", vec![q(9, false)])));
    assert!(!term::choosable(&s("hc", vec![q(2, true)])));
    assert!(!term::choosable(&s("hc", vec![q(2, false), q(2, false)])));
    // An empty question, or one the ask line cut (200 characters and an added `…`).
    let text = |t: String| Question { question: t, ..q(2, false) };
    assert!(term::choosable(&s("hc", vec![text("あ".repeat(200))])));
    assert!(!term::choosable(&s("hc", vec![text(format!("{}…", "あ".repeat(200)))])));
    assert!(!term::choosable(&s("hc", vec![text(String::new())])));
    assert!(!term::choosable(&s("herdr", vec![q(2, false)])));
    assert!(!term::choosable(&s("cmux", vec![q(2, false)])));
    assert!(!term::choosable(&Session { st: St::Working, ..s("hc", vec![q(2, false)]) }));
    assert!(!term::choosable(&Session { term_id: None, ..s("hc", vec![q(2, false)]) }));

    assert_eq!(term::choose_command(&s("hc", vec![q(2, false)]), 1), Some(vec!["hc".into(), "keys".into(), "t3".into(), "2".into()]));
    assert_eq!(term::choose_command(&s("herdr", vec![q(2, false)]), 1), None);
}

#[test]
fn the_question_prompt_is_open_only_when_hc_shows_it_blocked_on_that_question() {
    let get = |status: serde_json::Value| serde_json::json!({"address": "t3", "foreground": {"pid": 7}, "status": status}).to_string();
    let open = serde_json::json!({"app": "claude-code", "kind": "question", "msg": "answer: 試しの問いです。好きな色はどちらですか？ (赤 · 青)", "state": "blocked"});
    let q = "試しの問いです。好きな色はどちらですか？";
    assert!(term::question_open(&get(open.clone()), q));
    assert!(!term::question_open(&get(open.clone()), "別の問い"));
    let with = |k: &str, v: &str| {
        let mut s = open.clone();
        s[k] = serde_json::json!(v);
        get(s)
    };
    assert!(!term::question_open(&with("state", "working"), q));
    assert!(!term::question_open(&with("kind", "permission"), q));
    assert!(!term::question_open(&get(serde_json::json!({"app": "claude-code", "state": "blocked", "kind": "question"})), q));
    assert!(!term::question_open(&get(serde_json::Value::Null), q));
    assert!(!term::question_open("not json", q));
}
