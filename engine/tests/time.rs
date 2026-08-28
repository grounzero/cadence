// SPDX-License-Identifier: GPL-3.0-or-later

//! Time management: the budget is a pure function of the limits, and the
//! engine honours it.
//!
//! The allocation is tested as arithmetic first, over a grid of clocks,
//! increments and moves-to-go: a budget exists exactly when something
//! constrains the time; the hard limit never exceeds half of what is left
//! after the overhead; more time or increment never shortens it; few moves
//! to go lengthens it. Then "never lose on time" as a property of that
//! arithmetic -- a game whose every move spends the whole hard budget does
//! not run the clock out -- and finally against the binary, with a clock
//! the test keeps itself.

mod support;

use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{Colour, START_FEN, generate_legal, parse_uci};
use cadence_engine::search::{Limits, Search};
use cadence_engine::time::{Budget, MOVE_OVERHEAD_MS, budget};
use cadence_engine::tt::Table;
use support::Engine;

fn limits(s: &str) -> Limits {
    Limits::parse(s.split_whitespace())
}

fn clock(time: u64, inc: u64, movestogo: Option<u32>) -> Budget {
    let mut l = limits(&format!("wtime {time} btime {time} winc {inc} binc {inc}"));
    l.movestogo = movestogo;
    budget(&l, Colour::White).expect("a clock gives a budget")
}

const TIMES: [u64; 9] = [1, 20, 21, 50, 100, 500, 1000, 8000, 3_600_000];
const INCS: [u64; 5] = [0, 10, 80, 400, 1000];
const MTG: [Option<u32>; 5] = [None, Some(1), Some(5), Some(20), Some(40)];

/// Nothing constrains the time, so the clock is never read. "Nothing" is
/// exactly "the `go` named no clock at all": see the test below for the case
/// that used to be filed here and is not the same thing.
#[test]
fn no_constraint_means_no_budget() {
    for s in ["", "depth 6", "nodes 1000", "infinite", "depth 3 nodes 5"] {
        assert_eq!(budget(&limits(s), Colour::White), None, "{s:?}");
        assert_eq!(budget(&limits(s), Colour::Black), None, "{s:?}");
    }
    // Fixed depth is what `bench` runs under, and it must never consult a
    // clock; this is the contract at the allocation.
    assert_eq!(budget(&Limits::depth(7), Colour::White), None);
}

/// A `go` that named a clock always yields a budget, even when it did not
/// name *ours*.
///
/// Regression: `None` used to mean two things:
/// "nothing constrains the time", which is what `bench`, `go depth` and `go
/// infinite` need and which makes the node count a function of the code
/// alone, and "the GUI told us the opponent's clock and not our own", which
/// is not a licence to search forever. Only the first is safe, and the
/// engine took the second reading: `go wtime 1000 winc 10` with Black to
/// move ran until `stop`.
///
/// The safe reading of a clock we were not told is zero, which is already
/// the rule `Limits::parse` applies to a negative one -- a GUI sends that
/// when a side has overstepped, and it reads as zero rather than as "no
/// clock", so the search still hurries. Here it means soft and hard of zero:
/// the first iteration is returned and no more.
#[test]
fn a_clock_for_the_other_side_only_is_a_budget_of_zero() {
    for (line, us) in [
        ("wtime 1000 winc 10", Colour::Black),
        ("btime 1000 binc 10", Colour::White),
        // Neither clock, but an increment: still a clocked `go`, still
        // nothing said about our own time.
        ("winc 10", Colour::Black),
        ("movestogo 40", Colour::White),
    ] {
        assert_eq!(
            budget(&limits(line), us),
            Some(Budget { soft: 0, hard: 0 }),
            "go {line} for {us:?}"
        );
    }
    // `movetime` and `infinite` are not clocks and are unaffected: the first
    // governs, and the second is refused a budget by the search itself.
    assert_eq!(
        budget(&limits("movetime 300 wtime 1000"), Colour::Black),
        budget(&limits("movetime 300"), Colour::Black)
    );
}

#[test]
fn movetime_is_the_budget_less_the_overhead() {
    let b = budget(&limits("movetime 500"), Colour::White).expect("budget");
    assert_eq!(b.soft, 500 - MOVE_OVERHEAD_MS);
    assert_eq!(b.hard, 500 - MOVE_OVERHEAD_MS);
    // Under the overhead: nothing to spend, but still a budget -- the search
    // returns its first iteration and no more.
    let b = budget(&limits("movetime 5"), Colour::Black).expect("budget");
    assert_eq!(b, Budget { soft: 0, hard: 0 });
    // movetime is the same for either side.
    assert_eq!(
        budget(&limits("movetime 300"), Colour::White),
        budget(&limits("movetime 300"), Colour::Black)
    );
}

#[test]
fn a_clock_gives_a_budget_bounded_by_half_of_what_is_left() {
    for &t in &TIMES {
        for &inc in &INCS {
            for &mtg in &MTG {
                let b = clock(t, inc, mtg);
                let avail = t.saturating_sub(MOVE_OVERHEAD_MS);
                assert!(b.soft <= b.hard, "{t}+{inc} mtg {mtg:?}: {b:?}");
                assert!(
                    b.hard <= avail / 2,
                    "{t}+{inc} mtg {mtg:?}: {b:?} vs avail {avail}"
                );
                if t <= MOVE_OVERHEAD_MS {
                    assert_eq!(b, Budget { soft: 0, hard: 0 }, "{t}+{inc} mtg {mtg:?}");
                }
                if t >= 1000 {
                    assert!(b.soft >= 10, "{t}+{inc} mtg {mtg:?}: {b:?} is stingy");
                }
            }
        }
    }
    // And Black's clock is read for Black.
    let l = limits("wtime 100 btime 8000 winc 0 binc 80");
    let w = budget(&l, Colour::White).expect("w");
    let b = budget(&l, Colour::Black).expect("b");
    assert!(b.hard > w.hard, "white {w:?} black {b:?}");
}

#[test]
fn more_time_or_increment_never_shortens_the_budget() {
    for &mtg in &MTG {
        for &inc in &INCS {
            let mut prev = clock(TIMES[0], inc, mtg);
            for &t in &TIMES[1..] {
                let b = clock(t, inc, mtg);
                assert!(
                    b.soft >= prev.soft && b.hard >= prev.hard,
                    "{t}+{inc}: {prev:?} then {b:?}"
                );
                prev = b;
            }
        }
        for &t in &TIMES {
            let mut prev = clock(t, INCS[0], mtg);
            for &inc in &INCS[1..] {
                let b = clock(t, inc, mtg);
                assert!(
                    b.soft >= prev.soft && b.hard >= prev.hard,
                    "{t}+{inc}: {prev:?} then {b:?}"
                );
                prev = b;
            }
        }
    }
}

#[test]
fn the_increment_is_spent() {
    let without = clock(10_000, 0, None);
    let with = clock(10_000, 1000, None);
    assert!(with.soft > without.soft, "{without:?} vs {with:?}");
    assert!(with.hard >= without.hard, "{without:?} vs {with:?}");
}

#[test]
fn few_moves_to_go_means_more_per_move() {
    let sudden_death = clock(10_000, 0, None);
    let one = clock(10_000, 0, Some(1));
    let forty = clock(10_000, 0, Some(40));
    assert!(one.soft > forty.soft, "{one:?} vs {forty:?}");
    assert!(one.soft > sudden_death.soft, "{one:?} vs {sudden_death:?}");
    assert!(one.hard >= forty.hard, "{one:?} vs {forty:?}");
}

/// Search time alone never exhausts the clock: a game of 1,000 moves whose
/// every move spends the whole hard budget, from clocks large and small,
/// stays positive. And with latency up to the overhead per move and an
/// increment that covers it, the clock stays positive too.
#[test]
fn a_game_that_spends_every_hard_budget_does_not_run_out() {
    for &start in &[21u64, 100, 1000, 8000, 60_000] {
        let mut t = start;
        for mv in 0..1000 {
            let b = clock(t, 0, None);
            assert!(b.hard < t, "move {mv} from {start}: {b:?} with {t} left");
            t -= b.hard;
            assert!(t > 0, "move {mv} from {start}: clock ran out");
        }
        let mut t = start;
        for mv in 0..1000 {
            let b = clock(t, MOVE_OVERHEAD_MS, None);
            let spend = b.hard + MOVE_OVERHEAD_MS;
            assert!(
                spend < t + MOVE_OVERHEAD_MS,
                "move {mv} from {start}: {b:?} with {t} left"
            );
            t = t + MOVE_OVERHEAD_MS - spend;
            assert!(
                t > 0,
                "move {mv} from {start}: clock ran out with increment"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Against the binary
// ---------------------------------------------------------------------------

/// `movetime` is honoured: the search uses most of it and does not overrun
/// it by more than the generous allowance a slow CI runner needs.
#[test]
fn movetime_is_used_and_not_overrun() {
    let mut e = Engine::spawn();
    e.send("position startpos");
    e.sync();
    let start = Instant::now();
    e.send("go movetime 300");
    let lines = e.read_until("bestmove ");
    let elapsed = start.elapsed();
    e.quit();
    assert!(
        elapsed >= Duration::from_millis(200),
        "returned after only {elapsed:?}: {lines:?}"
    );
    assert!(elapsed <= Duration::from_millis(1500), "took {elapsed:?}");
}

/// A clock below the overhead still yields a legal move, at once.
#[test]
fn a_clock_below_the_overhead_yields_a_move_at_once() {
    let mut e = Engine::spawn();
    e.send("position startpos");
    e.sync();
    let start = Instant::now();
    e.send("go wtime 5 btime 5");
    let lines = e.read_until("bestmove ");
    let elapsed = start.elapsed();
    e.quit();
    let mv = lines
        .last()
        .unwrap()
        .strip_prefix("bestmove ")
        .unwrap()
        .to_string();
    let board = Board::from_fen(START_FEN).unwrap();
    assert!(parse_uci(&generate_legal(&board), &mv).is_some(), "{mv}");
    assert!(elapsed <= Duration::from_secs(1), "took {elapsed:?}");
}

/// A whole game on a clock the test keeps: 1000 ms + 20 ms each, measured
/// as wall-clock around each `go`, pipe latency included. Neither side may
/// run out. The game is played to its end or to a ply cap; the property is
/// the clock, not the result.
#[test]
fn a_game_on_the_clock_never_runs_out_of_time() {
    let mut e = Engine::spawn();
    let mut board = Board::from_fen(START_FEN).unwrap();
    let mut moves: Vec<String> = Vec::new();
    let mut time = [1000u64, 1000u64];
    let inc = 20u64;
    let mut plies = 0;
    let mut max_overrun_ms = 0u64;
    while plies < 120 {
        let legal = generate_legal(&board);
        if legal.is_empty() || board.halfmove_clock() >= 100 {
            break;
        }
        let us = board.side_to_move().index();
        e.send(&format!("position startpos moves {}", moves.join(" ")));
        e.sync();
        let start = Instant::now();
        e.send(&format!(
            "go wtime {} btime {} winc {inc} binc {inc}",
            time[0], time[1]
        ));
        let lines = e.read_until("bestmove ");
        let spent = u64::try_from(start.elapsed().as_millis()).unwrap();
        let mv = lines
            .last()
            .unwrap()
            .strip_prefix("bestmove ")
            .unwrap()
            .to_string();
        let m = parse_uci(&legal, &mv).unwrap_or_else(|| panic!("illegal bestmove {mv}"));
        assert!(
            spent < time[us],
            "ply {plies}: {spent} ms spent with {} ms on the clock",
            time[us]
        );
        // The engine's own hard cap is half of what is left after the
        // overhead; record the worst overrun past that, for the log.
        let cap = time[us].saturating_sub(MOVE_OVERHEAD_MS) / 2;
        max_overrun_ms = max_overrun_ms.max(spent.saturating_sub(cap));
        time[us] = time[us] - spent + inc;
        board.play(m);
        moves.push(mv);
        plies += 1;
    }
    e.quit();
    println!("{plies} plies, clocks {time:?}, worst overrun past the hard cap {max_overrun_ms} ms");
    assert!(plies >= 40, "only {plies} plies");
}

/// The same, end to end: a `go` carrying only the opponent's clock comes
/// back, and quickly.
#[test]
fn a_go_with_only_the_other_side_s_clock_comes_back() {
    // 1.e4, so it is Black to move and `wtime`/`winc` are White's. Measured
    // against the unfixed engine, this searched past four seconds and
    // returned only on `quit`.
    let (elapsed, lines) = Engine::go_within(
        &["position startpos moves e2e4"],
        "go wtime 1000 winc 10",
        Duration::from_secs(10),
    );
    assert!(
        elapsed <= Duration::from_secs(2),
        "took {elapsed:?}: {lines:?}"
    );
    let mv = lines
        .last()
        .expect("a bestmove line")
        .strip_prefix("bestmove ")
        .expect("a bestmove line");
    let mut board = Board::from_fen(START_FEN).expect("start position");
    let legal = generate_legal(&board);
    board.play(parse_uci(&legal, "e2e4").expect("e2e4 is legal"));
    assert!(
        parse_uci(&generate_legal(&board), mv).is_some(),
        "bestmove {mv} is not legal after 1.e4"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("info string go:")),
        "the missing clock is not reported: {lines:?}"
    );
}

// ---------------------------------------------------------------------------
// The iteration that is started and never finished
// ---------------------------------------------------------------------------

/// The position the waste is measured on: bench position 16, a middlegame,
/// so the ladder is one this repository already searches and nothing here is
/// chosen to make a number come out.
const MIDDLEGAME: &str = "r2q1rk1/1b1nbppp/pp1ppn2/8/2PNP3/1PN1B3/P2QBPPP/R4RK1 w - - 0 13";

/// The table size the bench runs with, so the ladder is the shape the
/// engine actually searches rather than one a starved table produces.
const HASH_MB: usize = 16;

/// One search of `MIDDLEGAME` under `limits`, against a table of its own.
///
/// Returns the elapsed milliseconds at the end of each completed iteration
/// and the elapsed milliseconds at the move. In process, so no part of what
/// is measured is a binary starting up -- the class of fault a timing test
/// that measured process start-up already cost this project once.
fn ladder(limits: Limits) -> (Vec<u64>, u64) {
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut board = Board::from_fen(MIDDLEGAME).expect("the middlegame position");
    let mut s = Search::new(limits, &stop, &tt);
    let start = Instant::now();
    s.run(&mut board, &mut std::io::sink());
    let returned = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    (s.iterations_ms().to_vec(), returned)
}

/// The clock that spends `soft` on a move, in sudden death with no
/// increment: `budget` takes a twenty-fifth of what is left after the
/// overhead, and the half-clock cap is nowhere near three times that.
fn clock_for(soft: u64) -> Limits {
    let wtime = 25 * soft + MOVE_OVERHEAD_MS;
    Limits {
        time: [Some(wtime), Some(wtime)],
        ..Limits::default()
    }
}

/// The seam the gate below reads: one elapsed reading per completed
/// iteration, and none at all where there is no budget.
///
/// The second half is the bench contract at the recording site. A depth
/// limit yields no budget, the clock is never read, and nothing is
/// recorded; if that ever stops being true the node count has stopped being
/// a function of the code alone.
#[test]
fn the_ladder_is_recorded_under_a_clock_and_not_under_a_depth() {
    let (rungs, _) = ladder(Limits::depth(6));
    assert!(rungs.is_empty(), "a depth limit read the clock: {rungs:?}");

    let mut limits = Limits::depth(6);
    limits.movetime = Some(30_000);
    let (rungs, _) = ladder(limits);
    assert_eq!(rungs.len(), 6, "six iterations, {} rungs", rungs.len());
    assert!(
        rungs.windows(2).all(|w| w[1] >= w[0]),
        "elapsed went backwards: {rungs:?}"
    );
}

/// **The waste.** An iteration whose cost the search can already predict
/// will not fit inside the hard budget must not be started: it runs to the
/// hard limit and returns the move the last completed iteration had
/// already produced.
///
/// The clock is derived from this machine and not written down, because a
/// clock that puts one machine in the window puts another either side of
/// it, and a gate for a condition that cannot be triggered passes without
/// covering anything. So: measure the ladder here, find a depth whose
/// successor cannot fit, and build the clock that lands on it. Two coverage
/// assertions carry that:
///
/// - a depth in the window exists on this machine at all, from the free
///   ladder;
/// - and the search under that clock stopped for some reason other than
///   the soft budget, from its own ladder. Without the second, a machine
///   whose ladder had drifted 25% between the two runs would pass this by
///   never reaching the window.
///
/// The property itself is scale-free and is read off the run that asserts
/// it: the time spent after the last completed iteration may not exceed
/// what that iteration itself cost. Today it is two and a half times it.
#[test]
fn an_iteration_that_cannot_finish_is_not_started() {
    // The free ladder: enough depth to see the window, and a movetime that
    // bounds the calibration whatever machine this is.
    let mut free = Limits::depth(10);
    free.movetime = Some(3_000);
    let (rungs, _) = ladder(free);
    assert!(rungs.len() >= 3, "no ladder to read: {rungs:?}");

    // The window: the last completed iteration finished before `soft`, so
    // the next is started, and it cannot finish before `hard`, which is
    // three times `soft`. A quarter of headroom on `soft` so the clock
    // survives the run-to-run variation between this ladder and the next.
    // The deepest such depth, capped so the gate costs about a second.
    let mut window = None;
    for d in 1..rungs.len() {
        let cum = rungs[d - 1];
        let next = rungs[d] - rungs[d - 1];
        let soft = cum + cum / 4 + 1;
        if cum > 0 && 3 * soft <= 1_500 && cum + next > 3 * soft {
            window = Some((d, soft));
        }
    }
    let (depth, soft) = window.unwrap_or_else(|| {
        panic!("no depth on this machine starts an iteration it cannot finish: {rungs:?}")
    });

    let (run, returned) = ladder(clock_for(soft));
    assert!(
        !run.is_empty(),
        "not one iteration completed under {soft} ms"
    );
    let last = *run.last().expect("a completed iteration");
    let cost = last
        - if run.len() >= 2 {
            run[run.len() - 2]
        } else {
            0
        };
    let wasted = returned.saturating_sub(last);

    assert!(
        last < soft,
        "the soft budget ended this search, so the window was never reached: \
         calibrated on depth {depth} of {rungs:?}, ran {run:?} against soft {soft}"
    );
    assert!(
        wasted <= cost,
        "{wasted} ms spent after the last completed iteration, which cost {cost} ms: \
         calibrated on depth {depth} of {rungs:?}, ran {run:?}, \
         soft {soft}, hard {}, returned at {returned}",
        3 * soft
    );
}
