// SPDX-License-Identifier: GPL-3.0-or-later

//! A ponderhit reproduced in process, alone in a test binary of its own so no other test's search
//! competes for the cores it is timed on. The search must stop where a fresh search with the same
//! iteration times would, stepped through each iteration, its deadlines moved by the time pondered.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadence_core::Colour;
use cadence_core::position::Board;
use cadence_engine::position::Position;
use cadence_engine::search::{Limits, PonderHit};
use cadence_engine::time::{
    Budget, another_iteration_fits, drop_scale, hit_budget, soft_for_drop, soft_for_run,
};
use cadence_engine::tt::Table;

/// Bench position 16, so nothing here is chosen to make a number come out.
const MIDDLEGAME: &str = "r2q1rk1/1b1nbppp/pp1ppn2/8/2PNP3/1PN1B3/P2QBPPP/R4RK1 w - - 0 13";

/// Sudden death, an increment, a move count and a fixed time, each pondered.
const PONDERS: [&str; 4] = [
    "ponder wtime 6000 btime 6000",
    "ponder wtime 6000 btime 6000 winc 60 binc 60",
    "ponder wtime 6000 btime 6000 movestogo 10",
    "ponder movetime 900",
];

/// Short of the soft budget, inside one iteration, and well past the hard one.
const PONDERED_MS: [u64; 4] = [150, 600, 1500, 3000];

/// The hit is timed when it is read, just before the raiser reads the clock, so the offset the
/// search used lies this far before the raise at most: two readings, each rounded to milliseconds.
const TIMED_WITHIN_MS: u64 = 2;

/// How far past a deadline the search may answer: its clock checks and a stalled runner.
const LATE_MS: u64 = 150;

/// An info line's `time` and the search's own reading at that iteration differ by this at most.
const READ_APART_MS: u64 = 20;

fn limits(s: &str) -> Limits {
    Limits::parse(s.split_whitespace())
}

fn since(origin: Instant) -> u64 {
    u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn info_times(out: &[u8]) -> Vec<u64> {
    String::from_utf8_lossy(out)
        .lines()
        .filter(|l| l.starts_with("info depth "))
        .map(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            toks.iter()
                .position(|t| *t == "time")
                .and_then(|at| toks.get(at + 1)?.parse().ok())
                .unwrap_or_else(|| panic!("no time in {l:?}"))
        })
        .collect()
}

/// What one reproduced hit leaves to compare.
struct Hit {
    /// From the search's own record, in ms from when pondering began.
    ladder: Vec<u64>,
    /// The `time` of each iteration's `info` line, from the same origin.
    reported: Vec<u64>,
    /// Completed iterations in a row on one move, at each completed iteration.
    runs: Vec<usize>,
    /// Thousandths of the soft budget the scores so far earn, at each completed iteration.
    scales: Vec<u64>,
    /// When the flag was raised, in ms from the origin.
    raised: u64,
    /// When the search returned, in ms from the origin.
    returned: u64,
}

fn reproduce(ponder: &str, pondered: u64) -> Hit {
    let stop = AtomicBool::new(false);
    let hit = PonderHit::new();
    let tt = Table::new(16).expect("a table");
    let mut board = Position::new(Board::from_fen(MIDDLEGAME).expect("the middlegame position"));
    let mut s = support::search(limits(ponder), &stop, &tt);
    s.set_ponder_hit(&hit);
    let origin = Instant::now();
    s.set_clock_origin(origin);
    let mut out = Vec::new();
    let raised = std::thread::scope(|scope| {
        let raiser = scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(pondered));
            hit.hit();
            since(origin)
        });
        let _ = s.run(&mut board, &mut out);
        raiser.join().expect("the hit")
    });
    let returned = since(origin);
    assert!(
        !stop.load(Ordering::Relaxed),
        "the test raised stop, so this measured nothing"
    );
    let roots = s.iteration_roots();
    let runs = (1..=roots.len())
        .map(|k| {
            let last = roots[k - 1].0;
            roots[..k].iter().rev().take_while(|r| r.0 == last).count()
        })
        .collect();
    let scores: Vec<_> = roots.iter().map(|r| r.1).collect();
    let scales = (1..=scores.len())
        .map(|k| drop_scale(&scores[..k]))
        .collect();
    Hit {
        ladder: s.iterations_ms().to_vec(),
        reported: info_times(&out),
        runs,
        scales,
        raised,
        returned,
    }
}

/// How a fresh search over the same iteration times would end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// At the hit, with this many completed: one of the iterations pondered would not have started
    /// the next, and the deepest costs nothing more.
    AtHit(usize),
    /// Between iterations, after this many.
    After(usize),
    /// Inside the next iteration, at the hard deadline, after this many.
    AtHard(usize),
}

/// The fresh search's decisions over `ladder`, its deadlines moved by `offset`, taken one iteration
/// at a time in order: every one pondered, at the hit, then one after each completed iteration.
fn fresh(
    clocked: &Limits,
    ladder: &[u64],
    runs: &[usize],
    scales: &[u64],
    b: Budget,
    offset: u64,
) -> End {
    let moved = Budget {
        soft: b.soft + offset,
        hard: b.hard + offset,
    };
    let goes_on = |k: usize| {
        let soft = soft_for_run(clocked, b, runs[k - 1]);
        let soft = soft_for_drop(clocked, b, soft, scales[k - 1]) + offset;
        ladder[k - 1] < soft && another_iteration_fits(&ladder[..k], moved)
    };
    let pondered = ladder.iter().take_while(|&&t| t <= offset).count();
    for k in 1..=pondered {
        if !goes_on(k) {
            return End::AtHit(pondered);
        }
    }
    for k in pondered + 1..=ladder.len() {
        if ladder[k - 1] >= moved.hard || !goes_on(k) {
            return End::After(k);
        }
    }
    End::AtHard(ladder.len())
}

/// The record is the search's, timed from when pondering began, so it is the input a fresh search
/// with the same iteration times would decide on.
#[test]
fn a_ponderhit_keeps_the_iteration_times_from_the_ponder() {
    for pondered in PONDERED_MS {
        let hit = reproduce(PONDERS[0], pondered);
        assert_eq!(
            hit.ladder.len(),
            hit.reported.len(),
            "pondered {pondered} ms: the record holds {:?} against the iterations reported at {:?}",
            hit.ladder,
            hit.reported
        );
        for (own, reported) in hit.ladder.iter().zip(&hit.reported) {
            assert!(
                own.abs_diff(*reported) <= READ_APART_MS,
                "pondered {pondered} ms: the record {:?} is not timed from the ponder's start, as \
                 the reports {:?} are",
                hit.ladder,
                hit.reported
            );
        }
    }
}

/// The search stops where that fresh search would and when: at once where an iteration pondered
/// would not have started the next, else at the iteration or the moved hard deadline it ends on.
#[test]
fn a_ponderhit_stops_where_and_when_a_fresh_search_with_the_same_times_would() {
    for ponder in PONDERS {
        let b = hit_budget(&limits(ponder), Colour::White).expect("a ponder with a clock");
        let clocked = limits(ponder.trim_start_matches("ponder "));
        for pondered in PONDERED_MS {
            let hit = reproduce(ponder, pondered);
            let completed = hit.runs.len();
            assert_eq!(
                hit.ladder.len(),
                completed,
                "`{ponder}` pondered {pondered} ms: {} times recorded for {completed} completed \
                 iterations",
                hit.ladder.len()
            );
            let ends =
                |offset: u64| fresh(&clocked, &hit.ladder, &hit.runs, &hit.scales, b, offset);
            let matches =
                (hit.raised.saturating_sub(TIMED_WITHIN_MS)..=hit.raised).any(|offset| match ends(
                    offset,
                ) {
                    End::AtHit(n) => n == completed && hit.returned <= hit.raised + LATE_MS,
                    End::After(n) => {
                        n == completed && hit.returned <= hit.ladder[n - 1].max(offset) + LATE_MS
                    }
                    End::AtHard(n) => {
                        n == completed
                            && hit.returned + LATE_MS >= offset + b.hard
                            && hit.returned <= offset + b.hard + LATE_MS
                    }
                });
            assert!(
                matches,
                "`{ponder}` pondered {pondered} ms, hit at {} ms, returned at {} ms after \
                 {completed} iterations at {:?}; a fresh search with these times and {b:?} from \
                 the hit ends {:?}",
                hit.raised,
                hit.returned,
                hit.ladder,
                ends(hit.raised)
            );
        }
    }
}
