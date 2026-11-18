use serde::Deserialize;
use std::path::Path;

use crate::config::Config;
use crate::transcript::{Entry, Kind};
use crate::util::{clip, home, read_regular};

const MAX_ROLE_BYTES: u64 = 1024 * 1024;

/// The report an agent writes as JSON. The HTML page is rendered from this.
#[derive(Deserialize)]
pub struct Report {
    pub title: String,
    /// What the human was trying to do with this work, in one sentence. Required; missing reads as
    /// empty so that the check can say what to write instead of failing to parse.
    #[serde(default)]
    pub goal: String,
    pub conclusion: String,
    pub decisions: Vec<Decision>,
    pub verified: Vec<String>,
    pub unverified: Vec<String>,
    pub deliverables: Vec<Deliverable>,
}

#[derive(Deserialize)]
pub struct Decision {
    pub question: String,
    pub options: Vec<Choice>,
    pub recommend: String,
    pub reason: String,
}

#[derive(Deserialize)]
pub struct Choice {
    pub label: String,
    pub effect: String,
}

#[derive(Deserialize)]
pub struct Deliverable {
    pub path: String,
    pub summary: String,
    #[serde(default)]
    pub changes: Vec<Change>,
}

#[derive(Deserialize)]
pub struct Change {
    pub line: u32,
    pub before: String,
    pub after: String,
}

/// What the transcript says happened, independent of what the report claims.
pub struct TurnFacts {
    /// Absolute paths edited in the turn that still exist and are not ignored.
    pub edited_files: Vec<String>,
    /// Bash commands run in the turn.
    pub turn_commands: Vec<String>,
    pub tool_calls: usize,
    pub is_orchestrator: bool,
    /// The report JSON written last in the turn, if any.
    pub report_file: Option<String>,
}

fn absolute(path: &str, cwd: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest).to_string_lossy().into_owned()
    } else if Path::new(path).is_absolute() {
        path.to_string()
    } else {
        Path::new(cwd).join(path).to_string_lossy().into_owned()
    }
}

/// A session started by Maestri with a role runs in that role's folder, whose CLAUDE.md holds the role text.
/// The orchestrator role is the one whose text tells the agent to send drafts with the review marker.
fn has_orchestrator_role(entries: &[Entry], cwd: &str, cfg: &Config) -> bool {
    let marker = cfg.role_dir_marker.as_str();
    let mut role_dirs: Vec<&str> = Vec::new();
    for dir in std::iter::once(cwd).chain(entries.iter().map(|e| e.cwd.as_str())) {
        let Some(found) = dir.find(marker) else { continue };
        let at = found + marker.len();
        let id_len = dir[at..].find('/').unwrap_or(dir.len() - at);
        let role_dir = &dir[..at + id_len];
        if !role_dirs.contains(&role_dir) {
            role_dirs.push(role_dir);
        }
    }
    role_dirs.iter().any(|role_dir| {
        read_regular(&Path::new(role_dir).join("CLAUDE.md"), MAX_ROLE_BYTES)
            .is_ok_and(|text| text.contains(&cfg.review_marker))
    })
}

pub fn collect_facts(entries: &[Entry], start: usize, cwd: &str, cfg: &Config) -> TurnFacts {
    let mut edited_files: Vec<String> = Vec::new();
    let mut turn_commands = Vec::new();
    let mut tool_calls = 0;
    let mut report_file = None;
    let report_dir = format!("/{}/", cfg.report_dir.trim_matches('/'));
    for e in entries[start..].iter().filter(|e| e.kind == Kind::Assistant) {
        for t in &e.tools {
            tool_calls += 1;
            if let Some(c) = &t.command {
                turn_commands.push(c.clone());
            }
            if !cfg.edit_tools.contains(&t.name) {
                continue;
            }
            for p in &t.file_paths {
                let abs = absolute(p, cwd);
                if abs.contains(&report_dir) && abs.ends_with(&cfg.report_suffix) {
                    report_file = Some(abs);
                    continue;
                }
                let ignored = cfg.ignore_path_patterns.iter().any(|pat| abs.contains(pat.as_str()));
                if !ignored && Path::new(&abs).exists() && !edited_files.contains(&abs) {
                    edited_files.push(abs);
                }
            }
        }
    }
    let is_orchestrator = has_orchestrator_role(entries, cwd, cfg);
    TurnFacts { edited_files, turn_commands, tool_calls, is_orchestrator, report_file }
}

/// True if some command in the shell text starts with `program`, as opposed to merely mentioning it
/// inside a string, a heredoc or another command's arguments.
fn invokes(command: &str, program: &str) -> bool {
    command
        .split(['\n', ';', '|', '&'])
        .any(|part| part.trim_start().starts_with(program))
}

/// Small turns are left alone: a report for a one-line change costs the human more than it tells them.
pub fn should_check(facts: &TurnFacts, cfg: &Config) -> bool {
    facts.edited_files.len() >= cfg.edited_files_threshold
        || facts.report_file.is_some()
        || facts.tool_calls >= cfg.tool_call_threshold
        || facts
            .turn_commands
            .iter()
            .any(|c| cfg.trigger_commands.iter().any(|t| invokes(c, t)))
}

fn is_code_file(path: &str, cfg: &Config) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| cfg.code_extensions.iter().any(|c| c.eq_ignore_ascii_case(e)))
}

fn shown(abs: &str, cwd: &str) -> String {
    clip(abs.strip_prefix(&format!("{}/", cwd.trim_end_matches('/'))).unwrap_or(abs), 120)
}

/// Returns the list of problems found. An empty list means the report passes.
pub fn check_report(report: &Report, reply: &str, facts: &TurnFacts, cwd: &str, cfg: &Config) -> Vec<String> {
    let mut issues = Vec::new();

    if report.title.trim().is_empty() {
        issues.push("title が空。作業の名前を書く。".to_string());
    }
    if report.goal.trim().is_empty() {
        issues.push("goal が空。人がこの作業で何をしようとしていたかを 1 文で書く。".to_string());
    }
    let conclusion_len = report.conclusion.trim().chars().count();
    if conclusion_len == 0 {
        issues.push("conclusion が空。何が起きたか、何を変えたかを書く。".to_string());
    } else if conclusion_len > cfg.conclusion_max_chars {
        issues.push(format!(
            "conclusion が {conclusion_len} 字ある。{} 字以内にして、内訳や経過は verified か deliverables に移す。",
            cfg.conclusion_max_chars
        ));
    }
    let reply_len = reply.trim().chars().count();
    if reply_len > cfg.reply_max_chars {
        issues.push(format!(
            "ターミナルの返事が {reply_len} 字ある。{} 字以内にして、結論と、決めてほしいことの有無だけを書く。詳細は JSON に書いてある。",
            cfg.reply_max_chars
        ));
    }

    for (i, d) in report.decisions.iter().enumerate() {
        let question_len = d.question.trim().chars().count();
        let name = if question_len == 0 {
            issues.push(format!("decisions[{i}] の question が空。"));
            format!("decisions[{i}]")
        } else {
            format!("判断項目「{}」", clip(&d.question, 60))
        };
        if question_len > cfg.question_max_chars {
            issues.push(format!(
                "{name}の question が {question_len} 字ある。Home のカードに出すので {} 字以内にし、背景は reason か options の effect に移す。",
                cfg.question_max_chars
            ));
        }
        if d.options.len() < 2 {
            issues.push(format!("{name}の options が 2 つ未満。選べるものを全部書く。"));
        }
        if d.options.iter().any(|o| o.label.trim().is_empty() || o.effect.trim().is_empty()) {
            issues.push(format!("{name}の options に、label か effect が空のものがある。effect には選ぶと何が起きるかを書く。"));
        }
        if !d.options.iter().any(|o| o.label == d.recommend) {
            issues.push(format!("{name}の recommend が、options のどの label とも一致しない。"));
        }
        if d.reason.trim().is_empty() {
            issues.push(format!("{name}の reason が空。推奨の理由を書く。"));
        }
    }

    for (i, f) in report.deliverables.iter().enumerate() {
        if f.path.trim().is_empty() || f.summary.trim().is_empty() {
            issues.push(format!("deliverables[{i}] の path か summary が空。"));
        }
    }
    let listed: Vec<(String, &Deliverable)> = report
        .deliverables
        .iter()
        .map(|f| (absolute(f.path.trim().trim_end_matches('/'), cwd), f))
        .collect();
    for file in &facts.edited_files {
        let name = shown(file, cwd);
        let exact = listed.iter().find(|(p, _)| p == file).map(|(_, f)| *f);
        if is_code_file(file, cfg) {
            match exact {
                None => issues.push(format!(
                    "コードのファイル {name} が deliverables にない。path をこのファイルにした項目を作り、changes を書く。"
                )),
                Some(f) if f.changes.is_empty() => issues.push(format!(
                    "コードのファイル {name} の changes が空。line、before、after を 1 か所以上書く。"
                )),
                Some(f) => {
                    if f.changes.iter().any(|c| c.line == 0 || c.before.trim().is_empty() || c.after.trim().is_empty()) {
                        issues.push(format!(
                            "{name} の changes に、line が 0 か、before か after が空のものがある。新しいファイルの before は「なし（新規）」と書く。"
                        ));
                    }
                }
            }
            continue;
        }
        let in_listed_dir = listed
            .iter()
            .any(|(p, _)| Path::new(p).is_dir() && file.starts_with(&format!("{p}/")));
        if exact.is_none() && !in_listed_dir {
            issues.push(format!(
                "編集したファイル {name} が deliverables にない。ファイルの path か、それを含むフォルダの path を書く。"
            ));
        }
    }

    let verified = report.verified.join("\n").to_lowercase();
    let commands = facts.turn_commands.join("\n").to_lowercase();
    for rule in &cfg.verification_rules {
        let Some(word) = rule.words.iter().find(|w| verified.contains(&w.to_lowercase())) else {
            continue;
        };
        if !rule.commands.iter().any(|c| commands.contains(&c.to_lowercase())) {
            issues.push(format!(
                "verified に「{word}」とあるが、このターンに対応するコマンド({})の実行記録がない。実行するか、unverified へ移す。",
                rule.commands.join("、")
            ));
        }
    }

    if facts.is_orchestrator {
        let reviewed = facts
            .turn_commands
            .iter()
            .any(|c| invokes(c, &cfg.review_command) && c.contains(&cfg.review_marker));
        if !reviewed {
            issues.push(format!(
                "報告を出す前に、点検役へ下書きを渡していない。依頼文の先頭に「{}」を付けて {} で渡し、指摘を直してから報告する。",
                cfg.review_marker, cfg.review_command
            ));
        }
    }

    issues
}
