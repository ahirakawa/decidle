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
