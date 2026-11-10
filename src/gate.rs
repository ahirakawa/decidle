use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::check::{check_report, collect_facts, should_check, Report};
use crate::config::Config;
use crate::render::{file_url, render};
use crate::transcript::{self, Kind};
use crate::util::{append_line, clip, data_dir, now_iso, read_regular, write_atomic};

const REPORT_FORMAT: &str = include_str!("../docs/report-format.md");
/// Upper bound for the hook input and for a report JSON. Real ones are a few kilobytes.
const MAX_INPUT_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize, Default)]
struct HookInput {
    session_id: Option<String>,
    transcript_path: Option<String>,
    cwd: Option<String>,
    prompt_id: Option<String>,
    last_assistant_message: Option<String>,
}

/// Rejection count for one turn. A different turn key means a new prompt, so the count restarts.
#[derive(Serialize, Deserialize, Default)]
struct State {
    turn_key: String,
    rounds: u32,
    /// Whether the agent has already been told once to write the report JSON.
    #[serde(default)]
    taught: bool,
}

fn state_path(data_dir: &Path, session_id: &str) -> PathBuf {
    let safe: String = session_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    let name = if safe.is_empty() { "no-session" } else { safe.as_str() };
    data_dir.join("state").join(format!("{name}.json"))
}

fn log(data_dir: &Path, session_id: &str, verdict: &str, issues: &[String], rounds: u32, error: Option<&str>) {
    let line = json!({
        "ts": now_iso(),
        "session_id": session_id,
        "verdict": verdict,
        "issues": issues,
        "rounds": rounds,
        "error": error,
    });
    let _ = append_line(&data_dir.join("gate.jsonl"), &line.to_string());
}

/// Where a report JSON really is and where its page goes.
struct ReportPaths {
    json: PathBuf,
    html: PathBuf,
}

/// Resolves symlinks and `..` before deciding anything, so that the page can only be written
/// into a real folder named `report_dir`, next to a regular file with the report suffix.
fn resolve_report(raw: &str, cfg: &Config) -> Result<ReportPaths, String> {
    let json = std::fs::canonicalize(raw).map_err(|e| format!("{raw}: {e}"))?;
    let meta = std::fs::metadata(&json).map_err(|e| format!("{raw}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{raw} は通常のファイルではない"));
    }
    if meta.len() > MAX_INPUT_BYTES {
        return Err(format!("{raw} が大きすぎる（{} バイト）", meta.len()));
    }
    let parent = json.parent().ok_or_else(|| format!("{raw} の親フォルダがない"))?;
    let dir_name = cfg.report_dir.trim_matches('/');
    if parent.file_name().and_then(|n| n.to_str()) != Some(dir_name) {
        return Err(format!("{raw} は {dir_name}/ の直下にない"));
    }
    let stem = json
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(&cfg.report_suffix))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{raw} の名前が {} で終わっていない", cfg.report_suffix))?;
    let html = parent.join(format!("{stem}.html"));
    Ok(ReportPaths { json, html })
}

fn read_report(paths: &ReportPaths) -> Result<Report, String> {
    let text = read_regular(&paths.json, MAX_INPUT_BYTES).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Renders the page next to the report. The page header carries only the report's file name,
/// so that a committed page does not reveal local paths.
fn write_page(report: &Report, paths: &ReportPaths, issues: &[String]) -> Result<String, String> {
    let fail = |e: String| format!("ページ {} を書けない: {e}", paths.html.display());
    let name = paths.json.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let html = render(report, &format!("{} ・ {name}", now_iso()), issues);
    // The folder was validated earlier; make sure it has not been swapped for a link since then.
    let parent = paths.html.parent().ok_or_else(|| fail("親フォルダがない".to_string()))?;
    if std::fs::canonicalize(parent).ok().as_deref() != Some(parent) {
        return Err(fail("報告のフォルダが入れ替わった".to_string()));
    }
    write_atomic(&paths.html, html.as_bytes(), false).map_err(|e| fail(e.to_string()))?;
    Ok(paths.html.to_string_lossy().into_owned())
}

/// Stop hook entry point. Any failure of the gate itself lets the agent stop.
pub fn run_stop() {
    if std::env::var("REPORT_GATE_MODE").is_ok_and(|m| m == "off") {
        return;
    }
    let mut raw = String::new();
    let _ = std::io::stdin().take(MAX_INPUT_BYTES).read_to_string(&mut raw);
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("report-gate: {e}");
            return;
        }
    };
    let data_dir = data_dir();
    let input: HookInput = match serde_json::from_str(&raw) {
        Ok(i) => i,
        Err(e) => {
            log(&data_dir, "", "error", &[], 0, Some(&format!("hook input: {e}")));
            return;
        }
    };
    let session_id = input.session_id.clone().unwrap_or_default();
    if let Err(e) = evaluate(&input, &cfg, &data_dir, &session_id) {
        log(&data_dir, &session_id, "error", &[], 0, Some(&e));
    }
}

fn evaluate(input: &HookInput, cfg: &Config, data_dir: &Path, session_id: &str) -> Result<(), String> {
    let transcript_path = input.transcript_path.as_deref().ok_or("transcript_path がない")?;
    let entries = transcript::load(Path::new(transcript_path))
        .map_err(|e| format!("会話記録 {transcript_path} を読めない: {e}"))?;
    let start = transcript::turn_start(&entries, input.prompt_id.as_deref())
        .ok_or("会話記録にプロンプトが見つからない")?;
    let cwd = input.cwd.clone().unwrap_or_default();
    let facts = collect_facts(&entries, start, &cwd, cfg);
    if !should_check(&facts, cfg) {
        return Ok(());
    }

    let reply = match input.last_assistant_message.as_deref().map(str::trim) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => entries[start..]
            .iter()
            .rev()
            .find(|e| e.kind == Kind::Assistant && !e.text.is_empty())
            .map(|e| e.text.clone())
            .unwrap_or_default(),
    };

    // A missing, misplaced or unreadable report is the agent's problem to fix, not a failure of the gate.
    let mut loaded: Option<(Report, ReportPaths)> = None;
    let issues = match &facts.report_file {
        None => vec![format!(
            "このターンで報告の JSON が書かれていない。{}/<作業名>{} に書く。",
            cfg.report_dir, cfg.report_suffix
        )],
        Some(raw) => match resolve_report(raw, cfg).and_then(|p| read_report(&p).map(|r| (r, p))) {
            Err(e) => vec![format!("報告の JSON を読めない: {e}")],
            Ok((report, paths)) => {
                let found = check_report(&report, &reply, &facts, &cwd, cfg);
                loaded = Some((report, paths));
                found
            }
        },
    };

    // Issue text can quote the report, so strip anything that could forge terminal output.
    let issues: Vec<String> = issues.iter().map(|i| clip(i, 300)).collect();

    let turn_key = input.prompt_id.clone().unwrap_or_else(|| entries[start].uuid.clone());
    let path = state_path(data_dir, session_id);
    // Only a missing state file means "no rejections yet". Any other failure must not reset the count.
    let mut state: State = match read_regular(&path, MAX_INPUT_BYTES) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("状態を読めない: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(e) => return Err(format!("状態を読めない: {e}")),
    };
    if state.turn_key != turn_key {
        state = State { turn_key, rounds: 0, taught: false };
    }

    let list = issues.iter().map(|i| format!("- {i}")).collect::<Vec<_>>().join("\n");
    // The first stop without a report only teaches the format, so it does not use up a round.
    let teaching = facts.report_file.is_none() && !state.taught;
    // Second, state-independent limit: the harness records each rejection in the transcript.
    let rejections = entries[start..]
        .iter()
        .filter(|e| e.kind == Kind::User && e.is_meta && e.text.starts_with("Stop hook feedback"))
        .count();
    let within_limit = rejections <= cfg.max_rounds as usize;
    if !issues.is_empty() && within_limit && (teaching || state.rounds < cfg.max_rounds) {
        if teaching {
            state.taught = true;
        } else {
            state.rounds += 1;
        }
        // Without a saved count the limit cannot be enforced, so do not block if saving fails.
        save_state(&path, &state)?;
        let reason = format!("報告が決まりに合っていない。次の点を直すこと。\n\n{list}\n\n{REPORT_FORMAT}");
        println!("{}", json!({ "decision": "block", "reason": reason }));
        log(data_dir, session_id, "block", &issues, state.rounds, None);
        return Ok(());
    }

    // Passed, or the rejection limit was reached: show the human whatever page can be rendered.
    let mut message = match &loaded {
        Some((report, paths)) => {
            format!("report-gate: 報告のページ {}", file_url(&write_page(report, paths, &issues)?))
        }
        None => "report-gate: 報告のページは作れなかった。".to_string(),
    };
    if issues.is_empty() {
        log(data_dir, session_id, "pass", &issues, state.rounds, None);
    } else {
        message.push_str(&format!("\n{}回差し戻しても次の点が残った。\n{list}", cfg.max_rounds));
        log(data_dir, session_id, "exhausted", &issues, state.rounds, None);
    }
    println!("{}", json!({ "systemMessage": message }));
    Ok(())
}

fn save_state(path: &Path, state: &State) -> Result<(), String> {
    let text = serde_json::to_string(state).map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes(), true).map_err(|e| format!("状態を保存できない: {e}"))
}

/// `report-gate render <report.json>`: writes the page without waiting for the Stop hook,
/// so that an agent can show it in a browser before it stops. Only the location and the JSON shape are checked here.
pub fn run_render(args: &[String]) -> i32 {
    let [json_path] = args else {
        eprintln!("usage: report-gate render <report.json>");
        return 1;
    };
    let result = Config::load().and_then(|cfg| {
        let paths = resolve_report(json_path, &cfg)?;
        let report = read_report(&paths).map_err(|e| format!("{json_path}: {e}"))?;
        write_page(&report, &paths, &[])
    });
    match result {
        Ok(html_path) => {
            println!("{}", file_url(&html_path));
            0
        }
        Err(e) => {
            eprintln!("report-gate: {e}");
            1
        }
    }
}
