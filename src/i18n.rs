//! The words decidle shows to humans and agents, one table per language. See docs/spec-i18n.md.
//! Callers look a text up by its name and the configured language. Every table must have the
//! same names (checked by a test), so adding a language means adding one table.
//! `{name}` in a text is a placeholder, filled by `fill`.

use serde::Deserialize;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    Ja,
    #[default]
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Ja, Lang::En];

    /// The code used in the config and in `<html lang>`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::Ja => "ja",
            Lang::En => "en",
        }
    }

    fn table(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Lang::Ja => JA,
            Lang::En => EN,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(1);

/// Sets the language of messages that are made far from the config (file errors and the like).
/// The binary calls this once, right after it reads the config.
pub fn set_current(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Lang::Ja,
        _ => Lang::En,
    }
}

/// The text named `key`. An unknown name is a bug: tests panic on it, a release build returns
/// an empty text instead of failing inside a hook.
pub fn t(lang: Lang, key: &str) -> &'static str {
    match lang.table().iter().find(|(k, _)| *k == key) {
        Some((_, v)) => v,
        None => {
            debug_assert!(false, "unknown text name {key}");
            ""
        }
    }
}

/// The text named `key` in every language, for readers that must accept either.
pub fn all(key: &str) -> impl Iterator<Item = &'static str> + '_ {
    Lang::ALL.into_iter().map(move |l| t(l, key))
}

/// The text named `key` with its `{name}` placeholders filled in one pass.
/// Inserted values are never scanned again.
pub fn f(lang: Lang, key: &str, values: &[(&str, &str)]) -> String {
    fill(t(lang, key), values)
}

pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let known = after
            .find('}')
            .and_then(|close| values.iter().find(|(k, _)| *k == &after[..close]).map(|(_, v)| (close, *v)));
        match known {
            Some((close, value)) => {
                out.push_str(value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

const JA: &[(&str, &str)] = &[
    // The Home line (src/home/note.rs).
    ("note.instruction", "## Home への一言（decidle）\n\
返事の最後の 1 行を、Home ページ用の一言にする。形は「Home: 要 <一言>」か「Home: 不要 <一言>」。\n\
- 要: 人の対応（判断、確認、返事、許可）が要る。不要: 要らない（完了の報告や、裏の作業を待っているだけのとき）。\n\
- 一言は 60 字以内で、何を待っているか、何を終えたかを書く。例: 「Home: 要 レビューを待っている」「Home: 不要 テストの完了を報告」\n\
- この行は Home ページに表示される。返事のほかの本文は保存されない。"),
    ("note.yes", "要"),
    ("note.no", "不要"),
    // Review marker (default of config review_marker).
    ("review_marker", "報告の点検:"),
    // Stop hook (src/gate.rs).
    ("gate.block_prefix", "報告が決まりに合っていない"),
    ("gate.block", "報告が決まりに合っていない。次の点を直すこと。\n\n{list}\n\n{when}\n\n{format}"),
    ("gate.when", "報告を書くのは、次のどれかに当たるターンだけにする。当たらないターンでは、人間に頼まれないかぎり報告の JSON を書かない。"),
    ("gate.when_files", "- 編集したファイルが {n} 個以上ある。"),
    ("gate.when_tools", "- ツールの呼び出しが {n} 回以上ある。"),
    ("gate.when_command", "- `{command}` を実行した。"),
    ("gate.no_report", "このターンで報告の JSON が書かれていない。{dir}/<作業名>{suffix} に書く。"),
    ("gate.unreadable_report", "報告の JSON を読めない: {e}"),
    ("gate.not_regular", "{raw} は通常のファイルではない"),
    ("gate.too_large", "{raw} が大きすぎる（{n} バイト）"),
    ("gate.no_parent", "{raw} の親フォルダがない"),
    ("gate.not_in_dir", "{raw} は {dir}/ の直下にない"),
    ("gate.bad_suffix", "{raw} の名前が {suffix} で終わっていない"),
    ("gate.page_unwritable", "ページ {path} を書けない: {e}"),
    ("gate.page_no_parent", "親フォルダがない"),
    ("gate.folder_replaced", "報告のフォルダが入れ替わった"),
    ("gate.timeout", "{command} が時間切れになった"),
    ("gate.portal_bad_name", "portal: 名前 {name} は使えない"),
    ("gate.portal_create_failed", "portal: create が失敗した"),
    ("gate.background_unreadable", "decidle: 裏の作業を確かめるための会話記録を読めない: {e}"),
    ("gate.stop_unwritable", "decidle: events.jsonl に停止を書けない: {e}"),
    ("gate.no_transcript_path", "transcript_path がない"),
    ("gate.transcript_unreadable", "会話記録 {path} を読めない: {e}"),
    ("gate.no_prompt", "会話記録にプロンプトが見つからない"),
    ("gate.state_unreadable", "状態を読めない: {e}"),
    ("gate.state_unsaved", "状態を保存できない: {e}"),
    ("gate.page", "decidle: 報告のページ {url}"),
    ("gate.no_page", "decidle: 報告のページは作れなかった。"),
    ("gate.exhausted", "\n{n}回差し戻しても次の点が残った。\n{list}"),
    // Report check (src/check.rs).
    ("check.title_empty", "title が空。作業の名前を書く。"),
    ("check.goal_empty", "goal が空。人がこの作業で何をしようとしていたかを 1 文で書く。"),
    ("check.conclusion_empty", "conclusion が空。何が起きたか、何を変えたかを書く。"),
    ("check.conclusion_long", "conclusion が {len} 字ある。{max} 字以内にして、内訳や経過は verified か deliverables に移す。"),
    ("check.reply_long", "ターミナルの返事が {len} 字ある。{max} 字以内にして、結論と、決めてほしいことの有無だけを書く。詳細は JSON に書いてある。"),
    ("check.question_empty", "decisions[{i}] の question が空。"),
    ("check.decision_name", "判断項目「{question}」"),
    ("check.question_long", "{name}の question が {len} 字ある。Home のカードに出すので {max} 字以内にし、背景は reason か options の effect に移す。"),
    ("check.options_few", "{name}の options が 2 つ未満。選べるものを全部書く。"),
    ("check.option_empty", "{name}の options に、label か effect が空のものがある。effect には選ぶと何が起きるかを書く。"),
    ("check.recommend_mismatch", "{name}の recommend が、options のどの label とも一致しない。"),
    ("check.reason_empty", "{name}の reason が空。推奨の理由を書く。"),
    ("check.deliverable_empty", "deliverables[{i}] の path か summary が空。"),
    ("check.code_missing", "コードのファイル {name} が deliverables にない。path をこのファイルにした項目を作り、changes を書く。"),
    ("check.changes_empty", "コードのファイル {name} の changes が空。line、before、after を 1 か所以上書く。"),
    ("check.change_bad", "{name} の changes に、line が 0 か、before か after が空のものがある。新しいファイルの before は「なし（新規）」と書く。"),
    ("check.file_missing", "編集したファイル {name} が deliverables にない。ファイルの path か、それを含むフォルダの path を書く。"),
    ("check.unverified_word", "verified に「{word}」とあるが、このターンに対応するコマンド({commands})の実行記録がない。実行するか、unverified へ移す。"),
    ("check.no_review", "報告を出す前に、点検役へ下書きを渡していない。依頼文の先頭に「{marker}」を付けて {command} で渡し、指摘を直してから報告する。"),
    ("list_sep", "、"),
    // Report page (templates/report.html, src/render.rs).
    ("page.none", "なし"),
    ("page.warn", "この報告には、点検で残った指摘がある。"),
    ("page.recommended", "推奨"),
    ("page.line", "{n}行目"),
    ("page.before", "変更前"),
    ("page.after", "変更後"),
    ("page.deliverables", "成果物"),
    ("page.unverified", "確かめていないこと"),
    ("page.verified", "確かめたこと"),
    ("page.goal", "目的: "),
    ("page.reason", "理由: "),
    ("page.decisions", "決めてほしいこと"),
    ("page.meta_sep", " ・ "),
    // Config (src/config.rs).
    ("config.report_dir", "config: report_dir はフォルダ名 1 つにする"),
    ("config.report_suffix", "config: report_suffix は .json で終わる、.json より長い文字列にする"),
    ("config.empty", "config: role_dir_marker、review_marker、review_command は空にできない"),
    // CLI (src/main.rs, src/home/clean.rs, src/extract.rs).
    ("cli.port_zero", "decidle serve: config の home_port に 0 は使えない"),
    ("cli.clean_off", "decidle clean: config の clean_after_days と clean_unanswered_after_days が 0 なので消さない"),
    ("cli.unreadable", "decidle: {path} を読めない"),
    ("clean.not_absolute", "絶対パスの形でない"),
    ("clean.no_name", "フォルダかファイル名がない"),
    ("clean.not_in_dir", "{dir}/ の中でない"),
    ("clean.not_report", ".json か .html でない"),
    ("clean.symlink", "途中にシンボリックリンクがある"),
    ("clean.folder_unchecked", "フォルダを確かめられない: {e}"),
    ("clean.file_unchecked", "ファイルを確かめられない: {e}"),
    ("clean.not_file", "普通のファイルでない"),
    ("clean.rewritten", "セッションが終わったあとに書き直されている"),
    ("clean.cannot_delete", "消せない: {e}"),
    ("clean.refused", "decidle clean: 消さない {path}: {why}"),
    ("clean.dry_run", "decidle clean: 消すもの {n} 件(--dry-run なので消していない)"),
    ("clean.deleted_one", "decidle clean: 消した {path}"),
    ("clean.deleted", "decidle clean: {n} 件を消した"),
    // File errors (src/util.rs, src/transcript.rs).
    ("file.not_regular", "{path}: 通常のファイルではない"),
    ("file.not_folder", "{path}: フォルダではない"),
    ("file.too_large", "大きすぎる"),
];

const EN: &[(&str, &str)] = &[
    ("note.instruction", "## One line for Home (decidle)\n\
End every reply with one line for the Home page: \"Home: yes <note>\" or \"Home: no <note>\".\n\
- yes: a human needs to act (decide, check, reply or allow). no: nothing to do (a finished report, or only waiting for background work).\n\
- Keep the note within 120 characters. Say what you are waiting for or what you finished. Examples: \"Home: yes waiting for a review\" \"Home: no reported that the tests pass\"\n\
- This line is shown on the Home page. The rest of the reply is not stored."),
    ("note.yes", "yes"),
    ("note.no", "no"),
    ("review_marker", "Report review:"),
    ("gate.block_prefix", "The report does not follow the rules"),
    ("gate.block", "The report does not follow the rules. Fix these points.\n\n{list}\n\n{when}\n\n{format}"),
    ("gate.when", "Write a report only for a turn that meets one of these. In other turns, do not write the report JSON unless the human asks for it."),
    ("gate.when_files", "- It edited {n} or more files."),
    ("gate.when_tools", "- It made {n} or more tool calls."),
    ("gate.when_command", "- It ran `{command}`."),
    ("gate.no_report", "No report JSON was written in this turn. Write it to {dir}/<task-name>{suffix}."),
    ("gate.unreadable_report", "Cannot read the report JSON: {e}"),
    ("gate.not_regular", "{raw} is not a regular file"),
    ("gate.too_large", "{raw} is too large ({n} bytes)"),
    ("gate.no_parent", "{raw} has no parent folder"),
    ("gate.not_in_dir", "{raw} is not directly in {dir}/"),
    ("gate.bad_suffix", "The name of {raw} does not end with {suffix}"),
    ("gate.page_unwritable", "Cannot write the page {path}: {e}"),
    ("gate.page_no_parent", "no parent folder"),
    ("gate.folder_replaced", "the report folder was replaced"),
    ("gate.timeout", "{command} timed out"),
    ("gate.portal_bad_name", "portal: cannot use the name {name}"),
    ("gate.portal_create_failed", "portal: create failed"),
    ("gate.background_unreadable", "decidle: cannot read the transcript to check background work: {e}"),
    ("gate.stop_unwritable", "decidle: cannot write the stop to events.jsonl: {e}"),
    ("gate.no_transcript_path", "no transcript_path"),
    ("gate.transcript_unreadable", "cannot read the transcript {path}: {e}"),
    ("gate.no_prompt", "no prompt found in the transcript"),
    ("gate.state_unreadable", "cannot read the state: {e}"),
    ("gate.state_unsaved", "cannot save the state: {e}"),
    ("gate.page", "decidle: report page {url}"),
    ("gate.no_page", "decidle: could not make the report page."),
    ("gate.exhausted", "\nThese points were left after {n} rejections.\n{list}"),
    ("check.title_empty", "title is empty. Write the name of the work."),
    ("check.goal_empty", "goal is empty. Write in one sentence what the human was trying to do with this work."),
    ("check.conclusion_empty", "conclusion is empty. Write what happened or what you changed."),
    ("check.conclusion_long", "conclusion has {len} characters. Keep it within {max}, and move the breakdown or the steps to verified or deliverables."),
    ("check.reply_long", "The reply in the terminal has {len} characters. Keep it within {max}, and write only the conclusion and whether there is anything to decide. The details are in the JSON."),
    ("check.question_empty", "decisions[{i}] has an empty question."),
    ("check.decision_name", "Decision \"{question}\""),
    ("check.question_long", "{name}: question has {len} characters. It is shown on a Home card, so keep it within {max}, and move the background to reason or to the effect of options."),
    ("check.options_few", "{name}: options has fewer than 2 items. Write every choice."),
    ("check.option_empty", "{name}: an option has an empty label or effect. In effect, write what happens if it is chosen."),
    ("check.recommend_mismatch", "{name}: recommend does not match any label in options."),
    ("check.reason_empty", "{name}: reason is empty. Write why you recommend it."),
    ("check.deliverable_empty", "deliverables[{i}] has an empty path or summary."),
    ("check.code_missing", "The code file {name} is not in deliverables. Add an item whose path is this file, and write changes."),
    ("check.changes_empty", "The code file {name} has empty changes. Write line, before and after for at least one place."),
    ("check.change_bad", "{name}: a change has line 0, or an empty before or after. For a new file, write \"none (new file)\" as before."),
    ("check.file_missing", "The edited file {name} is not in deliverables. Write the path of the file, or of a folder that holds it."),
    ("check.unverified_word", "verified says \"{word}\", but this turn has no record of running a matching command ({commands}). Run it, or move the item to unverified."),
    ("check.no_review", "You did not give the draft to the reviewer before reporting. Send it with {command}, starting the request with \"{marker}\", fix what the reviewer points out, then report."),
    ("list_sep", ", "),
    ("page.none", "None"),
    ("page.warn", "This report has points left from the review."),
    ("page.recommended", "Recommended"),
    ("page.line", "Line {n}"),
    ("page.before", "Before"),
    ("page.after", "After"),
    ("page.deliverables", "Deliverables"),
    ("page.unverified", "Not verified"),
    ("page.verified", "Verified"),
    ("page.goal", "Goal: "),
    ("page.reason", "Reason: "),
    ("page.decisions", "Decisions for you"),
    ("page.meta_sep", " · "),
    ("config.report_dir", "config: report_dir must be one folder name"),
    ("config.report_suffix", "config: report_suffix must end with .json and be longer than .json"),
    ("config.empty", "config: role_dir_marker, review_marker and review_command cannot be empty"),
    ("cli.port_zero", "decidle serve: home_port in the config cannot be 0"),
    ("cli.clean_off", "decidle clean: clean_after_days and clean_unanswered_after_days in the config are 0, so nothing is deleted"),
    ("cli.unreadable", "decidle: cannot read {path}"),
    ("clean.not_absolute", "not an absolute path"),
    ("clean.no_name", "no folder or file name"),
    ("clean.not_in_dir", "not in {dir}/"),
    ("clean.not_report", "not .json or .html"),
    ("clean.symlink", "a symbolic link on the way"),
    ("clean.folder_unchecked", "cannot check the folder: {e}"),
    ("clean.file_unchecked", "cannot check the file: {e}"),
    ("clean.not_file", "not a regular file"),
    ("clean.rewritten", "written again after the session ended"),
    ("clean.cannot_delete", "cannot delete: {e}"),
    ("clean.refused", "decidle clean: not deleting {path}: {why}"),
    ("clean.dry_run", "decidle clean: {n} to delete (not deleted because of --dry-run)"),
    ("clean.deleted_one", "decidle clean: deleted {path}"),
    ("clean.deleted", "decidle clean: deleted {n}"),
    ("file.not_regular", "{path}: not a regular file"),
    ("file.not_folder", "{path}: not a folder"),
    ("file.too_large", "too large"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn names(lang: Lang) -> BTreeSet<&'static str> {
        lang.table().iter().map(|(k, _)| *k).collect()
    }

    #[test]
    fn every_language_has_the_same_names() {
        for lang in Lang::ALL {
            assert_eq!(names(lang).len(), lang.table().len(), "duplicate name in {lang:?}");
            assert_eq!(names(lang), names(Lang::Ja), "{lang:?}");
        }
    }

    #[test]
    fn english_has_no_japanese() {
        let japanese = |c: char| matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}');
        for (k, v) in EN {
            assert!(!v.chars().any(japanese), "{k}: {v}");
        }
    }

    #[test]
    fn fill_replaces_known_names_once() {
        assert_eq!(fill("{a} and {b} {c}", &[("a", "{b}"), ("b", "x")]), "{b} and x {c}");
    }

    #[test]
    fn block_starts_with_its_prefix() {
        for lang in Lang::ALL {
            assert!(t(lang, "gate.block").starts_with(t(lang, "gate.block_prefix")));
        }
    }
}
