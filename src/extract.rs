use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::config::Config;
use crate::transcript::{is_real_prompt, parse_line, Kind};
use crate::util::home;

/// Writes one JSON line per prompt, paired with the assistant text that preceded it.
pub fn run(args: &[String]) -> i32 {
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("decidle: {e}");
            return 1;
        }
    };
    let projects_dir = match args {
        [] => home().join(".claude/projects"),
        [flag, dir] if flag == "--projects-dir" => PathBuf::from(dir),
        _ => {
            eprintln!("usage: decidle extract [--projects-dir DIR]");
            return 1;
        }
    };
    let mut files: Vec<PathBuf> = Vec::new();
    let Ok(projects) = std::fs::read_dir(&projects_dir) else {
        eprintln!("decidle: {} を読めない", projects_dir.display());
        return 1;
    };
    for project in projects.flatten() {
        let Ok(sessions) = std::fs::read_dir(project.path()) else { continue };
        for s in sessions.flatten() {
            if s.path().extension().is_some_and(|e| e == "jsonl") {
                files.push(s.path());
            }
        }
    }
    files.sort();

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for file in files {
        let project = file
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(f) = File::open(&file) else { continue };
        let mut prev_report = String::new();
        for line in BufReader::new(f).lines() {
            let Ok(line) = line else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let Some(e) = parse_line(&v) else { continue };
            if e.kind == Kind::Assistant {
                if !e.text.is_empty() {
                    prev_report = e.text;
                }
                continue;
            }
            if !is_real_prompt(&e) {
                continue;
            }
            let suspected_agent = cfg.agent_prompt_markers.iter().any(|m| e.text.contains(m.as_str()));
            let row = json!({
                "project": project,
                "session_id": e.session_id,
                "ts": e.timestamp,
                "prompt": e.text,
                "prev_report": prev_report,
                "suspected_agent": suspected_agent,
            });
            if writeln!(out, "{row}").is_err() {
                return 0;
            }
            prev_report.clear();
        }
    }
    0
}
