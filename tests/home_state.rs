//! The Rust reducer against a direct transcription of `Step` in docs/tla/Home.tla,
//! on random event sequences.

use decidle::home::note::Attention;
use decidle::home::state::{self, Event, Kind, St};

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
        "permission" => Acc { st: "permission", ..acc },
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
        "end" => Acc { st: "ended", ..acc },
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
    matches!(a.st, "working" | "permission" | "stopped" | "background")
}

/// WaitsTier(a) in Home.tla.
fn model_waits(a: Acc) -> bool {
    a.st == "permission" || a.st == "stopped" && (a.dec > 0 || matches!(a.att, "yes" | "unknown"))
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
        St::Stopped => "stopped",
        St::Background => "background",
        St::Resolved => "resolved",
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

const KINDS: [&str; 7] = ["start", "prompt", "permission", "stop", "resolved", "kill", "end"];
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
        let res = if matches!(e, "resolved" | "kill") {
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
    for _ in 0..5000 {
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
        ev("decide", "stop", now - 40, 1),
        Event { background: true, ..ev("bg", "stop", now - 7, 1) },
        Event { attention: Attention::Needed, ..ev("need", "stop", now - 50, 0) },
        Event { attention: Attention::NotNeeded, ..ev("done", "stop", now - 3, 0) },
        Event { attention: Attention::NotNeeded, ..ev("done-decide", "stop", now - 60, 1) },
    ];
    let rows: Vec<String> = state::visible(state::derive(&events), now).into_iter().map(|s| s.session_id).collect();
    // Waiting for a human: decisions (whatever the Home line says), permission, 要, unknown.
    // Below: 不要, then working. A stop waiting on background work sorts with the working rows.
    assert_eq!(rows, ["decide", "done-decide", "perm", "need", "stop1", "done", "work2", "bg", "work1"]);
    let waits: Vec<bool> = state::visible(state::derive(&events), now).iter().map(state::waits_for_human).collect();
    assert_eq!(waits, [true, true, true, true, true, false, false, false, false]);
}
