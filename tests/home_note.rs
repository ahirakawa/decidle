//! The agent's `Home:` line at the end of a reply.

use decidle::home::note::{self, Attention};

#[test]
fn reads_the_last_line_in_the_form() {
    let p = |s: &str| note::parse(s);
    assert_eq!(p("直した。\n\nHome: 要 レビューを待っている"), (Attention::Needed, Some("レビューを待っている".into())));
    assert_eq!(p("Home: 不要 テストの完了を報告\n"), (Attention::NotNeeded, Some("テストの完了を報告".into())));
    // Full-width colon and space, and spaces around.
    assert_eq!(p("  Home：不要\u{3000}完了  "), (Attention::NotNeeded, Some("完了".into())));
    let sixty = "あ".repeat(note::NOTE_MAX_CHARS);
    assert_eq!(p(&format!("Home: 要 {sixty}")), (Attention::Needed, Some(sixty)));
}

#[test]
fn anything_else_is_unknown_and_keeps_no_text() {
    let unknown = (Attention::Unknown, None);
    for reply in [
        "",
        "直した。",
        // Not the last line.
        "Home: 要 見てほしい\n以上。",
        "Home: 要",
        "Home: 要  ",
        "Home: 要見てほしい",
        "Home: たぶん要 見てほしい",
        "home: 要 見てほしい",
        "**Home:** 要 見てほしい",
        &format!("Home: 不要 {}", "あ".repeat(note::NOTE_MAX_CHARS + 1)),
        "Home: 要 制御\u{7}文字",
    ] {
        assert_eq!(note::parse(reply), unknown, "{reply:?}");
    }
}

#[test]
fn instruction_names_the_form_and_the_limit() {
    assert!(note::INSTRUCTION.contains("Home: 要 "));
    assert!(note::INSTRUCTION.contains("Home: 不要 "));
    assert!(note::INSTRUCTION.contains(&format!("{} 字以内", note::NOTE_MAX_CHARS)));
}
