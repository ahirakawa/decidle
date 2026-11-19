//! Deleting old reports. `decidle serve` runs this at start and once a day after that, and
//! `decidle clean` runs it by hand. Only the files that a stop line recorded (report_path and
//! report_html) are deleted, and only when every session that recorded the path has been
//! resolved or ended for the configured number of days. See "報告の自動削除" in docs/spec-home.md.
//! Nothing is written to events.jsonl.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::read;
use crate::i18n::{self, Lang};
use super::state::{self, Event, Kind, Session, St};

const DAY_US: i64 = 24 * 3600 * 1_000_000;

#[derive(Clone, Debug)]
pub struct Settings {
    /// Days after a session is resolved or ended before its reports are deleted. 0 turns this off.
    pub after_days: u64,
    /// Days after a session ended before a report whose decisions nobody answered is deleted.
    /// 0 turns this off. Independent of `after_days`.
    pub unanswered_after_days: u64,
    /// The folder name the reports live in (config `report_dir`, `.decidle` by default).
    pub report_dir: String,
    /// Language of the reasons and of what `report` writes.
    pub lang: Lang,
}

/// A recorded path that the records allow to delete, with the time the last of the sessions
/// that recorded it was resolved or ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub path: String,
    pub closed_us: i64,
}

impl Settings {
    /// Both kinds are turned off.
    pub fn off(&self) -> bool {
        self.after_days == 0 && self.unanswered_after_days == 0
    }
}

fn closed(st: St) -> bool {
    matches!(st, St::Resolved | St::Answered | St::Ended)
}

#[derive(Default)]
struct Track {
    acc: Session,
    /// When the session became resolved or ended, if it still is.
    closed_since: Option<i64>,
    /// The paths of the current stop when it asks for decisions that nobody has answered:
    /// cleared by a prompt, by a resolved line that the reducer accepts, and by a newer stop.
    /// An accepted kill ends the session but does not answer the decisions.
    open: Vec<String>,
    recorded: Vec<String>,
}

fn paths_of(ev: &Event) -> Vec<String> {
    [&ev.report_path, &ev.report_html].into_iter().flatten().filter(|p| !p.is_empty()).cloned().collect()
}

/// Which recorded paths the records allow to delete. Pure: the file system is not looked at.
/// A path is kept when any session that recorded it is not resolved or ended, became so less than
/// the limit ago, or is shown on the Home page. The limit is `after_us` for a report without
/// unanswered decisions and `unanswered_us` for one with them (a session can only end with
/// unanswered decisions through an end line or an accepted kill; an accepted resolved answers them).
/// None turns that kind of report off.
pub fn candidates(events: &[Event], now_us: i64, after_us: Option<i64>, unanswered_us: Option<i64>) -> Vec<Candidate> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut tracks: Vec<Track> = Vec::new();
    for ev in events {
        let i = *index.entry(ev.session_id.as_str()).or_insert_with(|| {
            tracks.push(Track::default());
            tracks.len() - 1
        });
        let t = &mut tracks[i];
        let before = t.acc.st;
        t.acc = state::step(&t.acc, ev);
        let after = t.acc.st;
        if !closed(after) {
            t.closed_since = None;
        } else if !closed(before) {
            t.closed_since = Some(ev.ts_us);
        }
        match ev.kind {
            Some(Kind::Stop) => {
                let paths = paths_of(ev);
                t.recorded.extend(paths.iter().cloned());
                t.open = if ev.decisions > 0 { paths } else { Vec::new() };
            }
            Some(Kind::Prompt) => t.open.clear(),
            // An accepted resolve or answer: the human dealt with the decisions.
            Some(Kind::Resolved | Kind::Answer) if after != before => t.open.clear(),
            _ => {}
        }
    }
    // path -> Some(latest close) while every recording allows deletion, None once one does not.
    let mut verdict: BTreeMap<&str, Option<i64>> = BTreeMap::new();
    for t in &tracks {
        let shown = state::shown(&t.acc) && now_us - t.acc.last_us < state::HIDE_AFTER_US;
        let closed_at = t.closed_since.filter(|_| closed(t.acc.st) && !shown);
        for p in &t.recorded {
            let limit = if t.open.contains(p) { unanswered_us } else { after_us };
            let allow = match (closed_at, limit) {
                (Some(c), Some(l)) if now_us - c >= l => Some(c),
                _ => None,
            };
            let entry = verdict.entry(p.as_str()).or_insert(allow);
            *entry = match (*entry, allow) {
                (Some(a), Some(b)) => Some(a.max(b)),
                _ => None,
            };
        }
    }
    verdict
        .into_iter()
        .filter_map(|(path, v)| v.map(|closed_us| Candidate { path: path.to_string(), closed_us }))
        .collect()
}

/// Why a candidate is not deleted after looking at the file system. None: delete it.
/// Uses the same form of path as the report links of the Home page: absolute, without empty,
/// `.` or `..` segments. The file must sit directly in a folder named `report_dir`, the folder
/// and its parents must not be symbolic links (the real path equals the recorded one), the file
/// must be a regular file ending in .json or .html, and it must not have been written after the
/// last recording session closed (someone wrote it again, for a newer report).
pub fn refusal(c: &Candidate, report_dir: &str, lang: Lang) -> Option<String> {
    let say = |key: &str| Some(i18n::t(lang, key).to_string());
    let p = c.path.as_str();
    if !p.starts_with('/') || p[1..].split('/').any(|s| s.is_empty() || s == "." || s == "..") {
        return say("clean.not_absolute");
    }
    let path = Path::new(p);
    let (Some(parent), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
        return say("clean.no_name");
    };
    if parent.file_name().and_then(|n| n.to_str()) != Some(report_dir) {
        return Some(i18n::f(lang, "clean.not_in_dir", &[("dir", report_dir)]));
    }
    if !(name.ends_with(".json") || name.ends_with(".html")) {
        return say("clean.not_report");
    }
    match fs::canonicalize(parent) {
        Ok(real) if real == parent => {}
        Ok(_) => return say("clean.symlink"),
        Err(e) => return Some(i18n::f(lang, "clean.folder_unchecked", &[("e", &e.to_string())])),
    }
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => return Some(i18n::f(lang, "clean.file_unchecked", &[("e", &e.to_string())])),
    };
    if !meta.file_type().is_file() {
        return say("clean.not_file");
    }
    let mtime_us = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(i64::MAX, |d| d.as_micros() as i64);
    if mtime_us > c.closed_us {
        return say("clean.rewritten");
    }
    None
}

pub struct Outcome {
    /// Deleted (or, with dry_run, to be deleted).
    pub deleted: Vec<PathBuf>,
    /// Allowed by the records but not deleted, with the reason. Files already gone are left out.
    pub refused: Vec<(PathBuf, String)>,
}

fn now_us() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0)
}

/// Reads events.jsonl and deletes what may be deleted. With dry_run nothing is deleted.
pub fn run(events_path: &Path, settings: &Settings, dry_run: bool) -> Result<Outcome, String> {
    let mut out = Outcome { deleted: Vec::new(), refused: Vec::new() };
    if settings.off() {
        return Ok(out);
    }
    let loaded = read::load(events_path)?;
    let days = |d: u64| (d > 0).then(|| (d as i64).saturating_mul(DAY_US));
    for c in candidates(&loaded.events, now_us(), days(settings.after_days), days(settings.unanswered_after_days)) {
        let path = PathBuf::from(&c.path);
        if fs::symlink_metadata(&path).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) {
            continue;
        }
        if let Some(reason) = refusal(&c, &settings.report_dir, settings.lang) {
            out.refused.push((path, reason));
            continue;
        }
        if !dry_run {
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    out.refused.push((path, i18n::f(settings.lang, "clean.cannot_delete", &[("e", &e.to_string())])));
                    continue;
                }
            }
        }
        out.deleted.push(path);
    }
    Ok(out)
}

/// Runs once and writes the result to stderr. Used by `decidle serve`.
pub fn run_and_report(events_path: &Path, settings: &Settings) {
    match run(events_path, settings, false) {
        Ok(o) => report(&o, false, settings.lang),
        Err(e) => eprintln!("decidle clean: {e}"),
    }
}

/// Deleted paths and their count, and refused paths with the reason, to stderr.
/// With dry_run the paths that would be deleted go to stdout, one per line.
pub fn report(o: &Outcome, dry_run: bool, lang: Lang) {
    for (p, why) in &o.refused {
        eprintln!("{}", i18n::f(lang, "clean.refused", &[("path", &p.display().to_string()), ("why", why)]));
    }
    if dry_run {
        for p in &o.deleted {
            println!("{}", p.display());
        }
        eprintln!("{}", i18n::f(lang, "clean.dry_run", &[("n", &o.deleted.len().to_string())]));
    } else {
        for p in &o.deleted {
            eprintln!("{}", i18n::f(lang, "clean.deleted_one", &[("path", &p.display().to_string())]));
        }
        if !o.deleted.is_empty() {
            eprintln!("{}", i18n::f(lang, "clean.deleted", &[("n", &o.deleted.len().to_string())]));
        }
    }
}

/// For `decidle serve`: runs now, then once a day, on its own thread.
pub fn spawn_daily(events_path: PathBuf, settings: Settings) {
    if settings.off() {
        return;
    }
    std::thread::spawn(move || loop {
        run_and_report(&events_path, &settings);
        std::thread::sleep(Duration::from_secs(24 * 3600));
    });
}
