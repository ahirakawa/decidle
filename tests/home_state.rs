//! The Rust reducer against a direct transcription of `Step` in docs/tla/Home.tla,
//! on random event sequences.

use decidle::home::note::Attention;
use decidle::home::state::{self, Event, Kind, Question, St};

/// `acc` of the model: st, lastStop (an index into the log, 0 = none), dec and att.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Acc {
    st: &'static str,
    last_stop: usize,
    dec: i64,
    att: &'static str,
}

const INIT: Acc = Acc { st: "none", last_stop: 0, dec: 0, att: "unknown" };

/// One event of the model log: Ev(s, e, d, res, bg, a).
#[derive(Clone, Copy, Debug)]
struct Ev {
    s: usize,
    e: &'static str,
    d: i64,
    res: usize,
    bg: bool,
    a: &'static str,
}

/// Step(acc, ev, i), line by line.
fn model_step(acc: Acc, ev: Ev, i: usize) -> Acc {
    match ev.e {
        "start" => Acc { st: "idle", last_stop: 0, dec: 0, att: "unknown" },
        "prompt" => Acc { st: "working", ..acc },
        "permission" => {
            if acc.st == "asking" {
                acc
            } else {
                Acc { st: "permission", ..acc }
            }
        }
        "ask" => Acc { st: "asking", ..acc },
        "asked" => {
            if acc.st == "asking" {
                Acc { st: "working", ..acc }
            } else {
                acc
            }
        }
        "stop" => Acc { st: if ev.bg { "background" } else { "stopped" }, last_stop: i, dec: ev.d, att: ev.a },
        "resolved" => {
            if acc.st == "stopped" && ev.res == acc.last_stop {
                Acc { st: "resolved", ..acc }
            } else {
                acc
            }
        }
        "kill" => {
            if acc.st == "stopped" && ev.res == acc.last_stop {
                Acc { st: "ended", ..acc }
            } else {
                acc
            }
        }
        "answer" => {
            if acc.st == "stopped" && ev.res == acc.last_stop {
                Acc { st: "answered", ..acc }
            } else {
                acc
            }
        }
        "end" => Acc { st: "ended", ..acc },
        "interrupt" => {
            if matches!(acc.st, "working" | "permission" | "asking" | "background") {
                Acc { st: "idle", ..acc }
            } else {
                acc
            }
        }
        _ => unreachable!(),
    }
}

/// Derive(s, L) via Fold.
fn model_derive(s: usize, log: &[Ev]) -> Acc {
    log.iter()
        .enumerate()
        .fold(INIT, |acc, (k, ev)| if ev.s == s { model_step(acc, *ev, k + 1) } else { acc })
}

fn model_shown(a: Acc) -> bool {
    matches!(a.st, "working" | "permission" | "asking" | "stopped" | "background")
}

/// WaitsTier(a) in Home.tla.
fn model_waits(a: Acc) -> bool {
    matches!(a.st, "permission" | "asking") || a.st == "stopped" && (a.dec > 0 || matches!(a.att, "yes" | "unknown"))
}

fn att_name(a: Attention) -> &'static str {
    a.as_str()
}

fn st_name(st: St) -> &'static str {
    match st {
        St::None => "none",
        St::Idle => "idle",
        St::Working => "working",
        St::Permission => "permission",
        St::Asking => "asking",
        St::Stopped => "stopped",
        St::Background => "background",
        St::Resolved => "resolved",
        St::Answered => "answered",
        St::Ended => "ended",
    }
}

/// xorshift64*, so that the test needs no extra crate and every run is reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const KINDS: [&str; 11] =
    ["start", "prompt", "permission", "stop", "resolved", "kill", "end", "interrupt", "answer", "ask", "asked"];
const SESSIONS: usize = 3;

fn ts_of(i: usize) -> String {
    format!("2026-11-18T00:00:00.{i:06}Z")
}

/// A random log. resolved and kill point at the current stop, an older stop of the session,
/// a stop of another session, or nothing, so that both accepted and ignored cases occur.
fn random_log(rng: &mut Rng, len: usize) -> Vec<Ev> {
    let mut log: Vec<Ev> = Vec::new();
    for _ in 0..len {
        let s = rng.below(SESSIONS);
        let e = KINDS[rng.below(KINDS.len())];
        let d = rng.below(3) as i64;
        let res = if matches!(e, "resolved" | "kill" | "answer") {
            let stops: Vec<usize> =
                log.iter().enumerate().filter(|(_, ev)| ev.e == "stop").map(|(k, _)| k + 1).collect();
            match rng.below(4) {
                0 => 0,
                1 if !stops.is_empty() => stops[rng.below(stops.len())],
                _ => model_derive(s, &log).last_stop,
            }
        } else {
            0
        };
        let bg = e == "stop" && rng.below(3) == 0;
        let a = if e == "stop" { ["yes", "no", "unknown"][rng.below(3)] } else { "unknown" };
        log.push(Ev { s, e, d, res, bg, a });
    }
    log
}

fn to_events(log: &[Ev]) -> Vec<Event> {
    log.iter()
        .enumerate()
        .map(|(k, ev)| Event {
            ts: ts_of(k + 1),
            ts_us: (k + 1) as i64,
            session_id: format!("s{}", ev.s),
            cwd: format!("/work/s{}", ev.s),
            kind: Kind::parse(ev.e),
            decisions: ev.d,
            resolves: (ev.res > 0).then(|| ts_of(ev.res)),
            background: ev.bg,
            attention: Attention::parse(ev.a),
            note: (ev.e == "stop" && ev.a != "unknown").then(|| format!("note {}", k + 1)),
            ..Event::default()
        })
        .collect()
}

#[test]
fn reducer_matches_model_on_random_logs() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut accepted = 0;
    let mut ignored = 0;
    // resolved and kill aimed at the current stop of a session waiting on background work.
    let mut refused_background = 0;
    for _ in 0..8000 {
        let len = 1 + rng.below(40);
        let log = random_log(&mut rng, len);
        // Every prefix, so that each step is compared, not only the final state.
        for n in 1..=log.len() {
            let prefix = &log[..n];
            let derived = state::derive(&to_events(prefix));
            for s in 0..SESSIONS {
                let want = model_derive(s, prefix);
                let got = derived.iter().find(|x| x.session_id == format!("s{s}"));
                let Some(got) = got else {
                    assert_eq!(want, INIT, "session s{s} missing in {prefix:?}");
                    continue;
                };
                assert_eq!(st_name(got.st), want.st, "st of s{s} after {prefix:?}");
                assert_eq!(got.last_stop, (want.last_stop > 0).then(|| ts_of(want.last_stop)), "lastStop of s{s} after {prefix:?}");
                assert_eq!(got.decisions, want.dec, "dec of s{s} after {prefix:?}");
                assert_eq!(state::shown(got), model_shown(want), "Shown of s{s} after {prefix:?}");
                assert_eq!(att_name(got.attention), want.att, "att of s{s} after {prefix:?}");
                if state::shown(got) {
                    assert_eq!(state::waits_for_human(got), model_waits(want), "WaitsTier of s{s} after {prefix:?}");
                }
                // The note shown is the one of the current stop.
                let want_note = (want.last_stop > 0 && want.att != "unknown").then(|| format!("note {}", want.last_stop));
                assert_eq!(got.note, want_note, "note of s{s} after {prefix:?}");
            }
            let last = prefix[n - 1];
            if matches!(last.e, "resolved" | "kill") {
                let before = model_derive(last.s, &prefix[..n - 1]);
                if before.st == "background" && last.res == before.last_stop {
                    refused_background += 1;
                }
                if model_derive(last.s, prefix) == before {
                    ignored += 1;
                } else {
                    accepted += 1;
                }
            }
        }
    }
    // The generator must reach both branches of the resolved and kill rules.
    assert!(accepted > 1000 && ignored > 1000, "accepted {accepted}, ignored {ignored}");
    assert!(refused_background > 300, "refused_background {refused_background}");
}

#[test]
fn visible_hides_old_and_sorts_by_rank_then_newest() {
    let ev = |sid: &str, kind: &str, us: i64, decisions: i64| Event {
        ts: format!("t{us}"),
        ts_us: us,
        session_id: sid.to_string(),
        kind: Kind::parse(kind),
        decisions,
        ..Event::default()
    };
    let now = state::HIDE_AFTER_US * 2;
    let events = vec![
        ev("old", "prompt", now - state::HIDE_AFTER_US, 0),
        ev("work1", "prompt", now - 10, 0),
        ev("work2", "prompt", now - 5, 0),
        ev("stop1", "stop", now - 20, 0),
        ev("perm", "permission", now - 30, 0),
        ev("ask", "ask", now - 35, 0),
        ev("decide", "stop", now - 40, 1),
        Event { background: true, ..ev("bg", "stop", now - 7, 1) },
        Event { attention: Attention::Needed, ..ev("need", "stop", now - 50, 0) },
        Event { attention: Attention::NotNeeded, ..ev("done", "stop", now - 3, 0) },
        Event { attention: Attention::NotNeeded, ..ev("done-decide", "stop", now - 60, 1) },
    ];
    let rows: Vec<String> = state::visible(state::derive(&events), now).into_iter().map(|s| s.session_id).collect();
    // Waiting for a human: decisions (whatever the Home line says), permission or a question, 要,
    // unknown. Below: 不要, then working. A stop waiting on background work sorts with the working rows.
    assert_eq!(rows, ["decide", "done-decide", "perm", "ask", "need", "stop1", "done", "work2", "bg", "work1"]);
    let waits: Vec<bool> = state::visible(state::derive(&events), now).iter().map(state::waits_for_human).collect();
    assert_eq!(waits, [true, true, true, true, true, true, false, false, false, false]);
}

#[test]
fn a_question_waits_for_its_answer() {
    let q = |text: &str| Question { question: text.into(), header: "h".into(), options: vec!["a".into(), "b".into()], multi: false };
    let ev = |i: i64, kind: &str| Event {
        ts: ts_of(i as usize),
        ts_us: i,
        session_id: "s".into(),
        kind: Kind::parse(kind),
        ..Event::default()
    };
    let ask = |i: i64, text: &str| Event { ask: vec![q(text)], ..ev(i, "ask") };
    let stop = ev(3, "stop");
    let stop_ts = stop.ts.clone();
    let resolve = |i: i64, kind: &str| Event { resolves: Some(stop_ts.clone()), ..ev(i, kind) };
    let mut log = vec![ev(1, "start"), ev(2, "prompt"), stop, ev(4, "prompt"), ask(5, "first")];
    let at = |log: &[Event]| state::derive(log).remove(0);

    let s = at(&log);
    assert_eq!(s.st, St::Asking);
    assert_eq!(s.ask, [q("first")]);
    assert!(state::shown(&s) && state::waits_for_human(&s));
    assert_eq!(state::rank(&s), 1);

    // The permission notification of the question, and resolve, kill and answer, change nothing.
    for (i, e) in [(6, ev(6, "permission")), (7, resolve(7, "resolved")), (8, resolve(8, "kill")), (9, resolve(9, "answer"))] {
        log.push(e);
        assert_eq!(at(&log).st, St::Asking, "after event {i}");
    }
    // A new ask replaces the questions.
    log.push(ask(10, "second"));
    assert_eq!(at(&log).ask, [q("second")]);
    // The answer brings it back to working, and the questions are gone.
    log.push(ev(11, "asked"));
    let s = at(&log);
    assert_eq!(s.st, St::Working);
    assert!(s.ask.is_empty());

    // An interrupt while asking goes to idle.
    log.push(ask(12, "third"));
    log.push(ev(13, "interrupt"));
    let s = at(&log);
    assert_eq!(s.st, St::Idle);
    assert!(s.ask.is_empty());

    // Closed with Esc: nothing is written, and the next prompt leaves the question.
    log.push(ask(14, "fourth"));
    log.push(ev(15, "prompt"));
    let s = at(&log);
    assert_eq!(s.st, St::Working);
    assert!(s.ask.is_empty());
}

#[test]
fn a_chosen_option_ends_the_question_it_names() {
    let q = Question { question: "q".into(), header: "h".into(), options: vec!["a".into(), "b".into()], multi: false };
    let ev = |i: i64, kind: &str| Event {
        ts: ts_of(i as usize),
        ts_us: i,
        session_id: "s".into(),
        kind: Kind::parse(kind),
        ..Event::default()
    };
    let ask = |i: i64| Event { ask: vec![q.clone()], ..ev(i, "ask") };
    let choose = |i: i64, ask: usize| Event { resolves: Some(ts_of(ask)), option: Some(0), ..ev(i, "choose") };
    let at = |log: &[Event]| state::derive(log).remove(0);

    let mut log = vec![ev(1, "start"), ev(2, "prompt"), ask(3)];
    assert_eq!(at(&log).ask_ts, Some(ts_of(3)));
    // A choose for another ask, or with no ask named, changes nothing.
    log.push(choose(4, 1));
    log.push(Event { resolves: None, ..choose(5, 3) });
    let s = at(&log);
    assert_eq!((s.st, s.ask_ts.clone()), (St::Asking, Some(ts_of(3))));
    // A new ask replaces the one a stale page names.
    log.push(ask(6));
    log.push(choose(7, 3));
    assert_eq!(at(&log).ask_ts, Some(ts_of(6)));

    log.push(choose(8, 6));
    let s = at(&log);
    assert_eq!(s.st, St::Working);
    assert!(s.ask.is_empty() && s.ask_ts.is_none());
    // The asked line that follows the key press keeps it working.
    log.push(ev(9, "asked"));
    assert_eq!(at(&log).st, St::Working);

    // Outside a question, a choose is ignored.
    log.push(ev(10, "stop"));
    log.push(choose(11, 6));
    assert_eq!(at(&log).st, St::Stopped);
}
