use serde::Deserialize;
use std::path::Path;

use crate::config::Config;
use crate::transcript::{Entry, Kind};
use crate::util::{clip, home, read_regular};
use decidle::i18n::{self, f, t};

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
            .is_ok_and(|text| has_review_marker(&text, cfg))
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

/// Whether the text holds the configured review marker or any language's default one, so that
/// a role written before a language change still counts.
fn has_review_marker(text: &str, cfg: &Config) -> bool {
    text.contains(&cfg.review_marker) || i18n::all("review_marker").any(|m| text.contains(m))
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
    let lang = cfg.language;
    let n = |v: usize| v.to_string();
    let mut issues = Vec::new();

    if report.title.trim().is_empty() {
        issues.push(t(lang, "check.title_empty").to_string());
    }
    if report.goal.trim().is_empty() {
        issues.push(t(lang, "check.goal_empty").to_string());
    }
    let conclusion_len = report.conclusion.trim().chars().count();
    if conclusion_len == 0 {
        issues.push(t(lang, "check.conclusion_empty").to_string());
    } else if conclusion_len > cfg.conclusion_max_chars {
        issues.push(f(lang, "check.conclusion_long", &[("len", &n(conclusion_len)), ("max", &n(cfg.conclusion_max_chars))]));
    }
    let reply_len = reply.trim().chars().count();
    if reply_len > cfg.reply_max_chars {
        issues.push(f(lang, "check.reply_long", &[("len", &n(reply_len)), ("max", &n(cfg.reply_max_chars))]));
    }

    for (i, d) in report.decisions.iter().enumerate() {
        let question_len = d.question.trim().chars().count();
        let name = if question_len == 0 {
            issues.push(f(lang, "check.question_empty", &[("i", &n(i))]));
            format!("decisions[{i}]")
        } else {
            f(lang, "check.decision_name", &[("question", &clip(&d.question, 60))])
        };
        if question_len > cfg.question_max_chars {
            issues.push(f(
                lang,
                "check.question_long",
                &[("name", &name), ("len", &n(question_len)), ("max", &n(cfg.question_max_chars))],
            ));
        }
        if d.options.len() < 2 {
            issues.push(f(lang, "check.options_few", &[("name", &name)]));
        }
        if d.options.iter().any(|o| o.label.trim().is_empty() || o.effect.trim().is_empty()) {
            issues.push(f(lang, "check.option_empty", &[("name", &name)]));
        }
        if !d.options.iter().any(|o| o.label == d.recommend) {
            issues.push(f(lang, "check.recommend_mismatch", &[("name", &name)]));
        }
        if d.reason.trim().is_empty() {
            issues.push(f(lang, "check.reason_empty", &[("name", &name)]));
        }
    }

    for (i, d) in report.deliverables.iter().enumerate() {
        if d.path.trim().is_empty() || d.summary.trim().is_empty() {
            issues.push(f(lang, "check.deliverable_empty", &[("i", &n(i))]));
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
                None => issues.push(f(lang, "check.code_missing", &[("name", &name)])),
                Some(d) if d.changes.is_empty() => issues.push(f(lang, "check.changes_empty", &[("name", &name)])),
                Some(d) => {
                    if d.changes.iter().any(|c| c.line == 0 || c.before.trim().is_empty() || c.after.trim().is_empty()) {
                        issues.push(f(lang, "check.change_bad", &[("name", &name)]));
                    }
                }
            }
            continue;
        }
        let in_listed_dir = listed
            .iter()
            .any(|(p, _)| Path::new(p).is_dir() && file.starts_with(&format!("{p}/")));
        if exact.is_none() && !in_listed_dir {
            issues.push(f(lang, "check.file_missing", &[("name", &name)]));
        }
    }

    let verified = report.verified.join("\n").to_lowercase();
    let commands = facts.turn_commands.join("\n").to_lowercase();
    for rule in &cfg.verification_rules {
        let Some(word) = rule.words.iter().find(|w| verified.contains(&w.to_lowercase())) else {
            continue;
        };
        if !rule.commands.iter().any(|c| commands.contains(&c.to_lowercase())) {
            issues.push(f(
                lang,
                "check.unverified_word",
                &[("word", word), ("commands", &rule.commands.join(t(lang, "list_sep")))],
            ));
        }
    }

    if facts.is_orchestrator {
        let reviewed = facts
            .turn_commands
            .iter()
            .any(|c| invokes(c, &cfg.review_command) && has_review_marker(c, cfg));
        if !reviewed {
            issues.push(f(
                lang,
                "check.no_review",
                &[("marker", &cfg.review_marker), ("command", &cfg.review_command)],
            ));
        }
    }

    issues
}
