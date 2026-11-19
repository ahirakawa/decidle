//! Reading events.jsonl with an in-memory DuckDB. No database file is created, so that the
//! server never holds a lock that the hooks would collide with.

use duckdb::Connection;
use std::path::Path;

use super::note::Attention;
use super::state::{Event, Kind, Question};

/// The events in line order, and the number of lines that could not be read
/// (cut off, not JSON, or missing ts, session_id or a known event).
pub struct Loaded {
    pub events: Vec<Event>,
    pub skipped: usize,
}

const QUERY: &str = r#"
WITH f AS (SELECT string_split(content, chr(10)) AS ls FROM read_text(?)),
lines AS (SELECT unnest(generate_series(1, len(ls))) AS n, unnest(ls) AS line FROM f),
parsed AS (
    SELECT n, CASE WHEN json_valid(line) AND json_type(line) = 'OBJECT' THEN line::JSON END AS j
    FROM lines WHERE trim(line) <> ''
)
SELECT
    j->>'ts',
    epoch_us(try_cast(j->>'ts' AS TIMESTAMPTZ)),
    j->>'session_id',
    j->>'cwd',
    j->>'event',
    try_cast(j->>'pid' AS BIGINT),
    try_cast(j->>'pid_started' AS BIGINT),
    j->>'report_path',
    j->>'report_title',
    try_cast(j->>'decisions' AS BIGINT),
    j->>'resolves',
    j->>'transcript_path',
    j->>'report_html',
    try_cast(j->>'background' AS BOOLEAN),
    j->>'attention',
    j->>'note',
    CASE WHEN json_type(j->'questions') = 'ARRAY' THEN CAST(j->'questions' AS VARCHAR) END,
    j->>'report_goal',
    j->>'agent',
    CASE WHEN json_type(j->'recommends') = 'ARRAY' THEN CAST(j->'recommends' AS VARCHAR) END,
    j->>'term',
    j->>'term_id',
    j->>'term_ws',
    CASE WHEN json_type(j->'ask') = 'ARRAY' THEN CAST(j->'ask' AS VARCHAR) END,
    try_cast(j->>'option' AS BIGINT)
FROM parsed ORDER BY n
"#;

type Row = (
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<bool>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

fn to_event(r: Row) -> Option<Event> {
    let (ts, ts_us, session_id, cwd, event, pid, pid_started, report_path, report_title, decisions, resolves, transcript_path, report_html, background, attention, note, questions, report_goal, agent, recommends, term, term_id, term_ws, ask, option) = r;
    let kind = Kind::parse(event.as_deref()?)?;
    let (term, term_id, term_ws) = super::term::checked(term, term_id, term_ws);
    Some(Event {
        ts: ts?,
        ts_us: ts_us?,
        session_id: session_id.filter(|s| !s.is_empty())?,
        cwd: cwd.unwrap_or_default(),
        kind: Some(kind),
        pid,
        pid_started,
        transcript_path,
        report_path,
        report_html,
        report_title,
        decisions: decisions.unwrap_or(0),
        resolves,
        background: background.unwrap_or(false),
        attention: Attention::parse(attention.as_deref().unwrap_or_default()),
        note,
        questions: questions.as_deref().map(parse_questions).unwrap_or_default(),
        report_goal,
        agent,
        recommends: recommends.as_deref().map(parse_recommends).unwrap_or_default(),
        term,
        term_id,
        term_ws,
        // Only an ask line carries questions.
        ask: if kind == Kind::Ask { ask.as_deref().map(parse_ask).unwrap_or_default() } else { Vec::new() },
        option,
    })
}

/// The `ask` array of an ask line. An entry that is not an object is dropped; a field of the
/// wrong type reads as empty, and so does an option that is not a string.
fn parse_ask(text: &str) -> Vec<Question> {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(text) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(serde_json::Value::as_object)
        .map(|q| {
            let text = |key: &str| q.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
            Question {
                question: text("question"),
                header: text("header"),
                options: q
                    .get("options")
                    .and_then(serde_json::Value::as_array)
                    .map(|o| o.iter().map(|l| l.as_str().unwrap_or_default().to_string()).collect())
                    .unwrap_or_default(),
                multi: q.get("multi").and_then(serde_json::Value::as_bool).unwrap_or(false),
            }
        })
        .collect()
}

/// The `questions` array of a stop line. Entries that are not strings are dropped.
fn parse_questions(text: &str) -> Vec<String> {
    match serde_json::from_str::<Vec<serde_json::Value>>(text) {
        Ok(v) => v.into_iter().filter_map(|q| q.as_str().map(str::to_string)).collect(),
        Err(_) => Vec::new(),
    }
}

/// The `recommends` array of a stop line. An entry that is not a string reads as empty, so the
/// positions still match `questions`.
fn parse_recommends(text: &str) -> Vec<String> {
    match serde_json::from_str::<Vec<serde_json::Value>>(text) {
        Ok(v) => v.into_iter().map(|q| q.as_str().unwrap_or_default().to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn load(path: &Path) -> Result<Loaded, String> {
    if !path.exists() {
        return Ok(Loaded { events: Vec::new(), skipped: 0 });
    }
    let fail = |e: duckdb::Error| format!("DuckDB: {e}");
    let conn = Connection::open_in_memory().map_err(fail)?;
    // Nothing may be downloaded or spilled to disk.
    conn.execute_batch(
        "SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; SET temp_directory = '';",
    )
    .map_err(fail)?;
    let mut stmt = conn.prepare(QUERY).map_err(fail)?;
    let rows = stmt
        .query_map([path.to_string_lossy().as_ref()], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
                row.get(11)?,
                row.get(12)?,
                row.get(13)?,
                row.get(14)?,
                row.get(15)?,
                row.get(16)?,
                row.get(17)?,
                row.get(18)?,
                row.get(19)?,
                row.get(20)?,
                row.get(21)?,
                row.get(22)?,
                row.get(23)?,
                row.get(24)?,
            ))
        })
        .map_err(fail)?;
    let mut loaded = Loaded { events: Vec::new(), skipped: 0 };
    for row in rows {
        match to_event(row.map_err(fail)?) {
            Some(ev) => loaded.events.push(ev),
            None => loaded.skipped += 1,
        }
    }
    Ok(loaded)
}
