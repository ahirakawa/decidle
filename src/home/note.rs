//! The one line an agent writes at the end of every reply for the Home page:
//! `Home: 要 <note>` / `Home: yes <note>` (the human has something to do) or
//! `Home: 不要 <note>` / `Home: no <note>` (nothing to do). Every language's form is read,
//! whatever the configured language, so that a reply written just before a language change
//! still counts. yes and no are read in any case.
//! `decidle stop` keeps only this line; the rest of the reply is never stored.
//! The SessionStart hook gives the agent `instruction`, so the human's CLAUDE.md need not change.

use crate::i18n::{self, Lang};

/// Longest note, in characters, for the language of the attention word. English takes about
/// twice the characters of Japanese for the same content (and half the width per character).
pub fn note_max_chars(lang: Lang) -> usize {
    match lang {
        Lang::Ja => 60,
        Lang::En => 120,
    }
}

/// What `decidle event` adds to the context at session start (and after a compaction).
pub fn instruction(lang: Lang) -> &'static str {
    i18n::t(lang, "note.instruction")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Attention {
    /// The line is missing or does not have the form. Shown with the sessions that wait for a human.
    #[default]
    Unknown,
    Needed,
    NotNeeded,
}

impl Attention {
    pub fn as_str(self) -> &'static str {
        match self {
            Attention::Needed => "yes",
            Attention::NotNeeded => "no",
            Attention::Unknown => "unknown",
        }
    }

    pub fn parse(text: &str) -> Attention {
        match text {
            "yes" => Attention::Needed,
            "no" => Attention::NotNeeded,
            _ => Attention::Unknown,
        }
    }
}

/// Reads the last non-empty line of the reply. Returns the attention and the note, or
/// (Unknown, None) when the line does not have the form.
pub fn parse(reply: &str) -> (Attention, Option<String>) {
    let Some(last) = reply.lines().map(str::trim).rfind(|l| !l.is_empty()) else {
        return (Attention::Unknown, None);
    };
    let Some(rest) = last.strip_prefix("Home:").or_else(|| last.strip_prefix("Home：")) else {
        return (Attention::Unknown, None);
    };
    let rest = rest.trim_start();
    let Some((attention, lang, rest)) = strip_word(rest) else {
        return (Attention::Unknown, None);
    };
    // The word must be followed by a space (half or full width), then the note.
    if !rest.starts_with([' ', '\u{3000}', '\t']) {
        return (Attention::Unknown, None);
    }
    let note = rest.trim();
    let len = note.chars().count();
    if len == 0 || len > note_max_chars(lang) || note.chars().any(char::is_control) {
        return (Attention::Unknown, None);
    }
    (attention, Some(note.to_string()))
}

/// Strips the attention word of any language, ignoring ASCII case, and tells which language
/// the word belongs to, so that the note's limit follows the form the agent wrote.
fn strip_word(text: &str) -> Option<(Attention, Lang, &str)> {
    // Longer words first, so that a word that begins another word does not cut it short.
    let mut words: Vec<(Attention, Lang, &str)> = Lang::ALL
        .iter()
        .flat_map(|&lang| {
            [(Attention::NotNeeded, lang, i18n::t(lang, "note.no")), (Attention::Needed, lang, i18n::t(lang, "note.yes"))]
        })
        .collect();
    words.sort_by_key(|(_, _, w)| std::cmp::Reverse(w.len()));
    words.into_iter().find_map(|(attention, lang, word)| {
        let head = text.get(..word.len())?;
        head.eq_ignore_ascii_case(word).then(|| (attention, lang, &text[word.len()..]))
    })
}
