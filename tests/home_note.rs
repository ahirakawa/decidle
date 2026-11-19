//! The agent's `Home:` line at the end of a reply.

use decidle::home::note::{self, Attention};
use decidle::i18n::Lang;

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
    let ja = note::instruction(Lang::Ja);
    assert!(ja.contains("Home: 要 "));
    assert!(ja.contains("Home: 不要 "));
    assert!(ja.contains(&format!("{} 字以内", note::NOTE_MAX_CHARS)));
    let en = note::instruction(Lang::En);
    assert!(en.contains("Home: yes "));
    assert!(en.contains("Home: no "));
    assert!(en.contains(&format!("within {} characters", note::NOTE_MAX_CHARS)));
}

#[test]
fn english_form_is_read_in_any_case() {
    let p = |s: &str| note::parse(s);
    assert_eq!(p("Done.\n\nHome: yes waiting for a review"), (Attention::Needed, Some("waiting for a review".into())));
    assert_eq!(p("Home: no reported that the tests pass\n"), (Attention::NotNeeded, Some("reported that the tests pass".into())));
    assert_eq!(p("Home: YES check this"), (Attention::Needed, Some("check this".into())));
    assert_eq!(p("Home：No\u{3000}done"), (Attention::NotNeeded, Some("done".into())));
    let sixty = "a".repeat(note::NOTE_MAX_CHARS);
    assert_eq!(p(&format!("Home: yes {sixty}")), (Attention::Needed, Some(sixty)));
    for reply in [
        "Home: yes",
        "Home: yesplease look",
        "Home: nothing to do",
        "Home: maybe yes look",
        "home: yes look",
        &format!("Home: no {}", "a".repeat(note::NOTE_MAX_CHARS + 1)),
    ] {
        assert_eq!(note::parse(reply), (Attention::Unknown, None), "{reply:?}");
    }
}
