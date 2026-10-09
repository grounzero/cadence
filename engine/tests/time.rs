// SPDX-License-Identifier: GPL-3.0-or-later

//! The allocation is tested as arithmetic over a grid, then never losing on time as a property of
//! it, then against the binary with a clock the test keeps.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{Colour, Move, START_FEN, generate_legal, parse_uci};
use cadence_engine::position::Position;
use cadence_engine::search::{Limits, Search};
use cadence_engine::time::{Budget, MOVE_OVERHEAD_MS, another_iteration_fits, budget};
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

/// Nothing means no clock named at all; a clock for the other side alone is the next test's case.
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

/// A clock we were not told reads as zero, the rule `Limits::parse` already applies to a negative
/// one: the first iteration and no more. `None` here once let `go wtime 1000 winc 10` with Black to
/// move run until `stop`.
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

/// Including latency up to the overhead per move, covered by an increment.
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

/// Within the allowance a slow CI runner needs.
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

/// 1000 ms + 20 ms, timed around each `go` with pipe latency included. The property is the clock,
/// not the result.
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

/// Bench position 16, so nothing here is chosen to make a number come out.
const MIDDLEGAME: &str = "r2q1rk1/1b1nbppp/pp1ppn2/8/2PNP3/1PN1B3/P2QBPPP/R4RK1 w - - 0 13";

/// So the ladder is the shape the engine searches, not one a starved table produces.
const HASH_MB: usize = 16;

/// In process, so no part of the measurement is a binary starting up.
fn ladder(limits: Limits) -> (Vec<u64>, u64) {
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut board = Position::new(Board::from_fen(MIDDLEGAME).expect("the middlegame position"));
    let mut s = support::search(limits, &stop, &tt);
    let start = Instant::now();
    s.run(&mut board, &mut std::io::sink());
    let returned = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    (s.iterations_ms().to_vec(), returned)
}

/// Sudden death: `budget` takes a twenty-fifth of what is left after the overhead.
fn clock_for(soft: u64) -> Limits {
    let wtime = 25 * soft + MOVE_OVERHEAD_MS;
    Limits {
        time: [Some(wtime), Some(wtime)],
        ..Limits::default()
    }
}

/// Read off `budget`, not written as a multiple of `soft`: a copy of the allocation's choice would
/// go on passing against a budget the search does not have.
fn hard_for(soft: u64) -> u64 {
    budget(&clock_for(soft), Colour::White)
        .expect("a clock gives a budget")
        .hard
}

/// Under a depth limit nothing is recorded and no clock read, which is the bench contract at the
/// recording site.
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

/// Below it the trial measures scheduling noise rather than the rule.
const MEASURABLE_MS: u64 = 8;

/// Integer division takes the quarter's margin to zero under four milliseconds; this floor keeps
/// the slack whatever the rung.
const MIN_HEADROOM_MS: u64 = 3;

fn soft_for(cum: u64) -> u64 {
    cum + (cum / 4).max(MIN_HEADROOM_MS) + 1
}

/// An iteration predicted not to fit the hard budget must not be started: it runs to the limit and
/// returns the move the last one already found. The clock is derived from this machine's ladder,
/// and a closed window is verified over every rung rather than passed quietly.
#[test]
#[ignore = "machine-dependent: whether an iteration that cannot finish lands in the window \
            turns on the runner's speed and one position's ladder; run with --ignored"]
fn an_iteration_that_cannot_finish_is_not_started() {
    // Enough depth to see the window and a movetime bounding the calibration on any machine.
    // Twelve, not ten: under the fitted table depth ten finished in 28 ms with no earlier rung
    // measurable.
    let mut free = Limits::depth(12);
    free.movetime = Some(3_000);
    let (rungs, _) = ladder(free);
    assert!(rungs.len() >= 3, "no ladder to read: {rungs:?}");

    // The last completed iteration finished before `soft` and the next cannot finish before `hard`,
    // three times `soft`. A quarter of headroom on `soft` survives run-to-run variation; the
    // deepest such depth, capped near a second.
    let mut window = None;
    for d in 1..rungs.len() {
        let cum = rungs[d - 1];
        let next = rungs[d] - rungs[d - 1];
        let soft = soft_for(cum);
        let hard = hard_for(soft);
        if cum >= MEASURABLE_MS && hard <= 1_500 && cum + next > hard {
            window = Some((d, soft));
        }
    }
    let Some((depth, soft)) = window else {
        // `MEASURABLE_MS` excuses a rung below it, so one rung must clear it or the closure was
        // checked over nothing.
        let measurable = (1..rungs.len())
            .filter(|&d| rungs[d - 1] >= MEASURABLE_MS)
            .count();
        assert!(
            measurable > 0,
            "no rung reached {MEASURABLE_MS} ms, so the closed window was asserted over \
             nothing: {rungs:?}"
        );

        // A rung the cost cap alone excluded is a trial this gate should have run, and fails.
        for d in 1..rungs.len() {
            let cum = rungs[d - 1];
            let next = rungs[d] - rungs[d - 1];
            let soft = soft_for(cum);
            assert!(
                cum < MEASURABLE_MS || cum + next <= hard_for(soft),
                "a depth in the window exists at {d} and only the calibration cap hid it: {rungs:?}"
            );
        }
        println!("the window is closed on this ladder, at every rung: {rungs:?}");
        return;
    };

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
        hard_for(soft)
    );
}

/// Pinned as numbers on the middlegame's ladder: at a hard budget of 576 ms, elapsed 153 with 9 two
/// iterations back predicts 630 and is refused.
#[test]
fn an_iteration_is_started_only_when_it_is_predicted_to_finish() {
    let clocked = |hard: u64| Budget {
        soft: hard / 3,
        hard,
    };
    let ladder = [0, 0, 0, 2, 9, 46, 153];

    assert!(!another_iteration_fits(&ladder, clocked(576)));
    // The same ladder with room for the prediction starts it.
    assert!(another_iteration_fits(&ladder, clocked(700)));
    // And a larger hard budget never refuses where a smaller one started.
    let mut started = false;
    for hard in (100..1200).step_by(10) {
        let fits = another_iteration_fits(&ladder, clocked(hard));
        assert!(
            fits || !started,
            "hard {hard} refuses what a shorter one started"
        );
        started |= fits;
    }
    assert!(started, "no hard budget in the range started an iteration");
}

/// So the search always has a move and the early iterations are never refused.
#[test]
fn the_rule_starts_an_iteration_whenever_it_cannot_predict() {
    let tight = Budget { soft: 1, hard: 3 };
    for rungs in [&[][..], &[0][..], &[0, 0][..], &[5, 400][..]] {
        assert!(another_iteration_fits(rungs, tight), "{rungs:?}");
    }
    // Three rungs, but the one two back took under a millisecond, so there
    // is no ratio to read.
    assert!(another_iteration_fits(&[0, 40, 900], tight));
}

/// Under `movetime` nothing later gets what this move does not spend, so refusing only gives up the
/// chance the prediction was wrong.
#[test]
fn the_rule_is_inert_where_nothing_is_saved_by_stopping() {
    let ladder = [0, 0, 0, 2, 9, 46, 153];
    let movetime = budget(&limits("movetime 200"), Colour::White).expect("budget");
    assert_eq!(movetime.soft, movetime.hard);
    assert!(another_iteration_fits(&ladder, movetime));
    // The same numbers on a clock, where the time is transferable.
    assert!(!another_iteration_fits(
        &ladder,
        Budget {
            soft: movetime.hard / 3,
            hard: movetime.hard
        }
    ));
}

// ---------------------------------------------------------------------------
// The root move and score kept across iterations
// ---------------------------------------------------------------------------

// Kept once per completed iteration and read back as the run it is; no rule reading it is asserted
// here.

/// In process and with its own table, for `ladder`'s reason.
fn searched<T>(fen: &str, limits: Limits, read: impl FnOnce(&Search, Move) -> T) -> T {
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut board = Position::new(Board::from_fen(fen).expect("a position"));
    let mut s = support::search(limits, &stop, &tt);
    let best = s.run(&mut board, &mut std::io::sink());
    read(&s, best)
}

/// Stated as the two halves of the property, not by recomputing the count, which would only show
/// two copies of one loop agree.
fn assert_the_run_is_the_trailing_one(s: &Search) {
    let roots = s.iteration_roots();
    let run = s.stable_iterations();
    let Some(&(last, _)) = roots.last() else {
        assert_eq!(run, 0, "no iteration completed and the run is {run}");
        return;
    };
    assert!(
        (1..=roots.len()).contains(&run),
        "run {run} over {} entries: {roots:?}",
        roots.len()
    );
    assert!(
        roots[roots.len() - run..].iter().all(|&(m, _)| m == last),
        "the run covers a move that is not the last: {roots:?}"
    );
    assert!(
        run == roots.len() || roots[roots.len() - run - 1].0 != last,
        "the run stops short of an equal move: {roots:?}"
    );
}

/// Unlike the ladder this costs no clock read, so it is kept under every limit.
#[test]
fn the_root_of_every_completed_iteration_is_recorded() {
    searched(MIDDLEGAME, Limits::depth(6), |s, _| {
        assert_eq!(s.completed_depth(), 6);
        assert_eq!(
            s.iteration_roots().len(),
            6,
            "six iterations: {:?}",
            s.iteration_roots()
        );
        assert!(
            s.iterations_ms().is_empty(),
            "a depth limit read the clock: {:?}",
            s.iterations_ms()
        );
    });

    let mut limits = Limits::depth(6);
    limits.movetime = Some(30_000);
    searched(MIDDLEGAME, limits, |s, _| {
        assert_eq!(
            s.iteration_roots().len(),
            s.completed_depth() as usize,
            "{} entries for {} iterations",
            s.iteration_roots().len(),
            s.completed_depth()
        );
    });
}

/// An entry written before the abort check, or from a cut-off iteration, disagrees with what the
/// caller is handed.
#[test]
fn the_last_entry_is_what_the_search_returns() {
    for limits in [Limits::depth(7), {
        let mut l = Limits::depth(7);
        l.movetime = Some(30_000);
        l
    }] {
        searched(MIDDLEGAME, limits, |s, best| {
            assert_eq!(
                s.iteration_roots().last(),
                Some(&(best, s.score())),
                "the last entry is not the move returned: {:?}",
                s.iteration_roots()
            );
        });
    }
}

/// Under `infinite` only the abort exits, so the condition holds by construction; the coverage is
/// that an iteration completed before the flag.
#[test]
fn an_abandoned_iteration_leaves_no_entry() {
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut board = Position::new(Board::from_fen(MIDDLEGAME).expect("the middlegame position"));
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            stop.store(true, Ordering::Relaxed);
        });
        let mut s = support::search(Limits::infinite(), &stop, &tt);
        let best = s.run(&mut board, &mut std::io::sink());
        assert!(
            s.completed_depth() >= 1,
            "no iteration completed, so nothing was abandoned"
        );
        assert_eq!(
            s.iteration_roots().len(),
            s.completed_depth() as usize,
            "the abandoned iteration left an entry: {:?}",
            s.iteration_roots()
        );
        assert_eq!(s.iteration_roots().last(), Some(&(best, s.score())));
    });
}

/// The fallback move is not an iteration's result, and a run of one over it would claim the root
/// move stood an iteration.
#[test]
fn a_search_that_completes_no_iteration_keeps_nothing() {
    let stop = AtomicBool::new(true);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut board = Position::new(Board::from_fen(MIDDLEGAME).expect("the middlegame position"));
    let mut s = support::search(Limits::infinite(), &stop, &tt);
    let best = s.run(&mut board, &mut std::io::sink());
    assert_eq!(s.completed_depth(), 0);
    assert!(
        s.iteration_roots().is_empty(),
        "an iteration that did not complete was kept: {:?}",
        s.iteration_roots()
    );
    assert_eq!(s.stable_iterations(), 0);
    assert!(generate_legal(&board).iter().any(|m| m == best));
}

/// The position with one legal move: the root cannot change, so the run is
/// every iteration.
const FORCED: &str = "7k/8/8/8/8/8/6q1/K7 w - - 0 1";

/// A reader returning the whole length passes every stable position, so the set must hold one whose
/// root move moved.
#[test]
fn the_run_counts_the_iterations_that_kept_the_move() {
    searched(FORCED, Limits::depth(6), |s, _| {
        assert_eq!(s.completed_depth(), 6);
        assert_eq!(s.stable_iterations(), 6, "{:?}", s.iteration_roots());
        assert_the_run_is_the_trailing_one(s);
    });

    let mut changed = Vec::new();
    for fen in [START_FEN, KIWIPETE, ENDING, MIDDLEGAME] {
        searched(fen, Limits::depth(8), |s, _| {
            assert_the_run_is_the_trailing_one(s);
            assert!(s.stable_iterations() >= 1);
            if s.stable_iterations() < s.iteration_roots().len() {
                changed.push((fen, s.iteration_roots().to_vec()));
            }
        });
    }
    assert!(
        !changed.is_empty(),
        "no root move changed anywhere, so the run was never read short"
    );
}

/// Kiwipete and a pawn ending, both from the bench set, so the positions
/// the run is read on are ones this repository already searches.
const KIWIPETE: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
const ENDING: &str = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";

/// Cleared with the killers, history and correction table, so a carried run cannot claim the root
/// stood since a position never seen.
#[test]
fn the_entries_do_not_survive_into_the_next_search() {
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut first = Position::new(Board::from_fen(MIDDLEGAME).expect("the middlegame position"));
    let mut second = Position::new(Board::from_fen(FORCED).expect("the forced position"));
    let mut s = support::search(Limits::depth(6), &stop, &tt);
    s.run(&mut first, &mut std::io::sink());
    assert_eq!(s.iteration_roots().len(), 6);
    s.run(&mut second, &mut std::io::sink());
    assert_eq!(
        s.iteration_roots().len(),
        6,
        "the first search's entries are still there: {:?}",
        s.iteration_roots()
    );
    assert!(
        s.iteration_roots()
            .iter()
            .all(|&(m, _)| m == s.iteration_roots()[0].0),
        "the forced position kept more than one move: {:?}",
        s.iteration_roots()
    );
}

/// It carries the clock, and once parsed as an ordinary clocked search it answered a move nobody
/// had asked for.
#[test]
fn a_ponder_does_not_answer_on_its_own_budget() {
    let limits = Limits {
        ponder: true,
        time: [Some(60_000), Some(60_000)],
        ..Limits::default()
    };
    assert!(
        budget(&limits, Colour::White).is_none(),
        "a ponder was given a budget"
    );

    let parsed = Limits::parse("ponder wtime 60000 btime 60000 winc 600 binc 600".split(' '));
    assert!(parsed.ponder, "`ponder` did not parse");
    assert!(
        budget(&parsed, Colour::White).is_none(),
        "a parsed `go ponder` still took a budget from the clock"
    );

    // End to end: the clock here would end an ordinary search inside 100 ms, and a ponder is
    // still going when `stop` arrives.
    let mut e = support::Engine::spawn();
    e.send("position startpos");
    e.send("go ponder wtime 200 btime 200");
    let seen = e.sync();
    assert!(
        !seen.iter().any(|l| l.starts_with("bestmove")),
        "the ponder answered before it was told to: {seen:?}"
    );
    e.send("stop");
    let out = e.read_until("bestmove ");
    assert!(
        out.last().is_some_and(|l| l.starts_with("bestmove ")),
        "no move after stop"
    );
    e.quit();

    // And `ponderhit` is the other way it is told. The same ponder, released by the hit rather
    // than by `stop`, answers on the clock the `go ponder` carried.
    let out = Engine::within(Duration::from_secs(20), || {
        let mut e = Engine::spawn();
        e.send("position startpos");
        e.sync();
        e.send("go ponder wtime 20000 btime 20000");
        let seen = e.sync();
        assert!(
            !seen.iter().any(|l| l.starts_with("bestmove")),
            "the ponder answered before the hit: {seen:?}"
        );
        e.send("ponderhit");
        let out = e.read_until("bestmove ");
        e.quit();
        out
    });
    assert!(
        out.last().is_some_and(|l| l.starts_with("bestmove ")),
        "no move after ponderhit: {out:?}"
    );
}

/// A parallel `go` builds its own searches, so a primary without the flag plays on until `stop`: a
/// loss on time that every default-`Threads` test passes.
#[test]
fn a_ponderhit_is_answered_at_more_than_one_thread() {
    let out = Engine::within(Duration::from_secs(30), || {
        let mut e = Engine::spawn();
        e.send("setoption name Threads value 4");
        e.send("position startpos");
        e.sync();
        e.send("go ponder wtime 20000 btime 20000");
        let seen = e.sync();
        assert!(
            !seen.iter().any(|l| l.starts_with("bestmove")),
            "the ponder answered before the hit: {seen:?}"
        );
        e.send("ponderhit");
        let out = e.read_until("bestmove ");
        e.quit();
        out
    });
    assert!(
        out.last().is_some_and(|l| l.starts_with("bestmove ")),
        "no move after ponderhit at Threads=4: {out:?}"
    );
}

/// A ponder that has run out of iterations waits, and a hit arriving then must still be answered,
/// or the bridge waits on a move that never comes and the clock runs out.
#[test]
fn a_ponderhit_after_the_ponder_has_finished_is_answered() {
    for threads in [1, 2] {
        let out = Engine::within(Duration::from_secs(20), move || {
            let mut e = Engine::spawn();
            e.send(&format!("setoption name Threads value {threads}"));
            e.send("position startpos");
            e.sync();
            e.send("go ponder depth 1 wtime 20000 btime 20000");
            let seen = e.read_until("info depth 1 ");
            // Past the root loop's own read of the hit, so only the wait can answer it.
            std::thread::sleep(Duration::from_millis(200));
            assert!(
                !seen.iter().any(|l| l.starts_with("bestmove")),
                "the ponder answered before the hit: {seen:?}"
            );
            e.send("ponderhit");
            let out = e.read_until("bestmove ");
            e.quit();
            out
        });
        assert!(
            out.last().is_some_and(|l| l.starts_with("bestmove ")),
            "no move after a ponderhit on a finished ponder at Threads={threads}: {out:?}"
        );
    }
}

/// The gap between an `info` line's `time` and the whole exchange's wall clock is the pondering the
/// budget no longer counts.
#[test]
fn a_ponderhit_moves_the_clock_origin_to_the_hit() {
    const PONDERED_MS: u64 = 700;

    let (wall, lines) = Engine::within(Duration::from_secs(30), || {
        let mut e = Engine::spawn();
        e.send("position startpos");
        e.sync();
        let start = Instant::now();
        e.send("go ponder wtime 20000 btime 20000");
        std::thread::sleep(Duration::from_millis(PONDERED_MS));
        e.send("ponderhit");
        let lines = e.read_until("bestmove ");
        let wall = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        e.quit();
        (wall, lines)
    });

    // The last iteration line before the move: its `time` is what the search thinks it has
    // spent, and after a hit that is measured from the hit.
    let reported: u64 = lines
        .iter()
        .rev()
        .filter(|l| l.starts_with("info depth "))
        .find_map(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            let at = toks.iter().position(|t| *t == "time")?;
            toks.get(at + 1)?.parse().ok()
        })
        .unwrap_or_else(|| panic!("no info line with a time: {lines:?}"));

    assert!(
        wall >= reported + PONDERED_MS / 2,
        "the search reported {reported} ms of a {wall} ms exchange, so the clock still counts \
         the {PONDERED_MS} ms it spent pondering"
    );
}

/// The hit is raised before the run, so the whole search is what a `ponderhit` leaves: it records a
/// ladder and returns without `stop`.
#[test]
fn a_ponder_that_is_hit_becomes_a_clocked_search() {
    let stop = AtomicBool::new(false);
    let hit = AtomicBool::new(true);
    let tt = Table::new(16).expect("a table");
    let mut board = Position::new(Board::from_fen(START_FEN).expect("the start position"));
    let mut s = support::search(limits("ponder wtime 20000 btime 20000"), &stop, &tt);
    s.set_ponder_hit(&hit);
    let best = s.run(&mut board, &mut Vec::new());

    assert!(!best.is_null(), "no move from a hit ponder");
    assert!(
        !stop.load(Ordering::Relaxed),
        "the test raised stop, so this measured nothing"
    );
    assert!(
        !s.iterations_ms().is_empty(),
        "a hit ponder recorded no iteration, so it never took the clock"
    );
    let budget = budget(&limits("wtime 20000 btime 20000"), Colour::White).expect("a budget");
    assert!(
        s.iterations_ms()
            .last()
            .is_some_and(|&ms| ms <= budget.hard),
        "the ladder ran past the hard budget it took on the hit: {:?}",
        s.iterations_ms()
    );
}
