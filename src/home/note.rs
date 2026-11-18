//! The one line an agent writes at the end of every reply for the Home page:
//! `Home: 要 <note>` (the human has something to do) or `Home: 不要 <note>` (nothing to do).
//! `report-gate stop` keeps only this line; the rest of the reply is never stored.
//! The SessionStart hook gives the agent `INSTRUCTION`, so the human's CLAUDE.md need not change.

/// Longest note, in characters.
pub const NOTE_MAX_CHARS: usize = 60;

/// What `report-gate event` adds to the context at session start (and after a compaction).
pub const INSTRUCTION: &str = "## Home への一言（report-gate）\n\
返事の最後の 1 行を、Home ページ用の一言にする。形は「Home: 要 <一言>」か「Home: 不要 <一言>」。\n\
- 要: 人の対応（判断、確認、返事、許可）が要る。不要: 要らない（完了の報告や、裏の作業を待っているだけのとき）。\n\
- 一言は 60 字以内で、何を待っているか、何を終えたかを書く。例: 「Home: 要 レビューを待っている」「Home: 不要 テストの完了を報告」\n\
- この行は Home ページに表示される。返事のほかの本文は保存されない。";

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
    // 不要 first: 要 is its suffix, not its prefix, but keep the order explicit.
    let (attention, rest) = if let Some(r) = rest.strip_prefix("不要") {
        (Attention::NotNeeded, r)
    } else if let Some(r) = rest.strip_prefix("要") {
        (Attention::Needed, r)
    } else {
        return (Attention::Unknown, None);
    };
    // The word must be followed by a space (half or full width), then the note.
    if !rest.starts_with([' ', '\u{3000}', '\t']) {
        return (Attention::Unknown, None);
    }
    let note = rest.trim();
    let len = note.chars().count();
    if len == 0 || len > NOTE_MAX_CHARS || note.chars().any(char::is_control) {
        return (Attention::Unknown, None);
    }
    (attention, Some(note.to_string()))
}
