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

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{Colour, Move, START_FEN, generate_legal, parse_uci};
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
/// overhead, and the half-clock cap is nowhere near the hard budget.
fn clock_for(soft: u64) -> Limits {
    let wtime = 25 * soft + MOVE_OVERHEAD_MS;
    Limits {
        time: [Some(wtime), Some(wtime)],
        ..Limits::default()
    }
}

/// The hard budget the engine will actually use for that clock.
///
/// Read off `budget` rather than written here as a multiple of `soft`. The
/// multiple is the allocation's to choose, and a copy of it in the gate goes
/// on passing while asserting against a budget the search does not have,
/// which is a gate that has stopped discriminating rather than one that
/// fails. It is a copy today and this removes it before it is one that is
/// wrong.
fn hard_for(soft: u64) -> u64 {
    budget(&clock_for(soft), Colour::White)
        .expect("a clock gives a budget")
        .hard
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
/// what that iteration itself cost. When the rule landed it was two and a
/// half times it.
///
/// **The window can be closed, and a closed window is verified rather
/// than failed.** The waste exists only where an iteration can cost more
/// than the hard budget leaves: with hard at three times soft, that needs
/// the next iteration to outweigh roughly 2.75 times everything searched
/// so far, which the derivation behind the rule states as the window
/// closing at a branching factor of three. Null-move pruning brought this
/// tree's ladder under that everywhere on this machine, so the free
/// ladder can present no depth to build the trial on. The gate then
/// asserts the closure off the whole ladder, cap ignored, and passes with
/// the ladder printed: a rung the calibration cap alone excluded still
/// fails loudly, and any tree that reopens the window re-arms the trial
/// by itself. What must not happen is the third option, a quiet pass that
/// looked and found nothing to ask.
/// The cheapest rung this gate will calibrate on, in milliseconds.
///
/// Below it the clock has no resolution to spare and the trial measures
/// scheduling noise rather than the rule. The gate's precondition is a rung
/// the clock can measure, and stating it is better than widening a margin
/// until the noise fits underneath.
const MEASURABLE_MS: u64 = 8;

/// The least headroom `soft` carries over the rung it was derived from, in
/// milliseconds.
///
/// The quarter below is the intended margin and integer division takes it to
/// zero under four milliseconds, which left the whole of the slack as the
/// `+ 1`. This floor closes that whatever the rung, so a ladder that slips
/// under `MEASURABLE_MS` in some later tree cannot produce degenerate slack.
const MIN_HEADROOM_MS: u64 = 3;

/// The soft budget a rung implies: a quarter of headroom, never less than
/// [`MIN_HEADROOM_MS`].
fn soft_for(cum: u64) -> u64 {
    cum + (cum / 4).max(MIN_HEADROOM_MS) + 1
}

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
        let soft = soft_for(cum);
        let hard = hard_for(soft);
        if cum >= MEASURABLE_MS && hard <= 1_500 && cum + next > hard {
            window = Some((d, soft));
        }
    }
    let Some((depth, soft)) = window else {
        // The precondition's own coverage. `MEASURABLE_MS` excuses a rung
        // from the assertion below, so a ladder whose every rung is under
        // it would leave this branch asserting nothing at all and printing
        // that the window is closed: the quiet pass this gate's doc says
        // must not happen, arriving through the floor that was added to
        // stop it failing wrongly. One rung has to clear the floor for the
        // closure to have been checked over anything.
        let measurable = (1..rungs.len())
            .filter(|&d| rungs[d - 1] >= MEASURABLE_MS)
            .count();
        assert!(
            measurable > 0,
            "no rung reached {MEASURABLE_MS} ms, so the closed window was asserted over \
             nothing: {rungs:?}"
        );

        // The closed window, verified over every rung with the cost cap
        // ignored: a rung the cap alone excluded is a trial this gate
        // should have run and did not, and fails rather than passing over
        // it.
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

/// The rule as arithmetic, which is how the rest of this module is tested.
///
/// The ladder is the one measured on the middlegame position above, so the
/// case the gate below covers end to end is pinned here as numbers too: at
/// a hard budget of 576 ms, elapsed 153 with 9 two iterations back predicts
/// 630 and the iteration is refused.
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

/// Every way of not knowing answers "start it", so the search always has a
/// move and the early iterations are never refused.
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

/// `movetime` makes the hard budget the soft one, and there the rule is
/// inert: nothing later gets what this move does not spend, so an
/// abandoned iteration costs nothing and refusing to start one only gives
/// up the chance that the prediction was wrong.
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

// The state a rule that spends on how long the root move has stood reads,
// and the state a rule reading the same shape off the score reads with it.
// These gates say it is kept once per completed iteration and read back as
// the run it is, not that a search holding it runs.

/// One search of `fen` under `limits`, handing the finished search and the
/// move it returned to `read`. In process and against a table of its own,
/// for the reason [`ladder`] is.
fn searched<T>(fen: &str, limits: Limits, read: impl FnOnce(&Search, Move) -> T) -> T {
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut board = Board::from_fen(fen).expect("a position");
    let mut s = Search::new(limits, &stop, &tt);
    let best = s.run(&mut board, &mut std::io::sink());
    read(&s, best)
}

/// The run is the trailing one: every entry it covers holds the last move,
/// and the entry before it does not. Stated as the two halves of that
/// property rather than by recomputing the count, which would only assert
/// that two copies of one loop agree.
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

/// One entry per completed iteration, under a clock and under a depth
/// limit alike.
///
/// **This is where it parts from the ladder above.** An elapsed reading
/// costs a clock read, so `iterations_ms` stays empty where there is no
/// budget and the bench contract needs it to; a move and a score cost no
/// clock read at all, so this is kept under every limit and the same
/// assertion holds either side.
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

/// The last entry is the move and score the search returns.
///
/// This is the half that says the state is read correctly rather than
/// merely kept: an entry written before the abort check, or written from
/// the partial result of an iteration that was cut off, disagrees with what
/// the caller is handed and nothing else in the suite would see it.
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

/// An iteration that is abandoned leaves nothing behind.
///
/// Under `infinite` the loop has no budget to break on, so the only way out
/// is the abort and the condition this gate wants holds by construction
/// rather than by a clock coming out right; the coverage assertion is that
/// an iteration completed at all before the flag went up.
#[test]
fn an_abandoned_iteration_leaves_no_entry() {
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut board = Board::from_fen(MIDDLEGAME).expect("the middlegame position");
    std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            stop.store(true, Ordering::Relaxed);
        });
        let mut s = Search::new(Limits::infinite(), &stop, &tt);
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

/// A search that completes no iteration keeps nothing, and still returns a
/// move.
///
/// The fallback move is the best root move fully searched, which is not an
/// iteration's result and must not read as one: a run of one over it would
/// tell a rule the root move had stood for an iteration when none finished.
#[test]
fn a_search_that_completes_no_iteration_keeps_nothing() {
    let stop = AtomicBool::new(true);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut board = Board::from_fen(MIDDLEGAME).expect("the middlegame position");
    let mut s = Search::new(Limits::infinite(), &stop, &tt);
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

/// The run counts the iterations that kept the move, and it is read on a
/// position where the move changes and one where it cannot.
///
/// **The changing half carries the coverage.** A reader returning the whole
/// length passes every assertion a stable position can make, so the gate
/// requires a position in the set whose root move moved and fails with the
/// entries printed when none did, rather than passing over a property it
/// never met.
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

/// One search's entries do not reach the next.
///
/// The killers, the history and the correction table are cleared at the
/// head of a search and this is kept beside them, so a run carried over
/// would tell the next move's rule the root had stood since a position it
/// never saw.
#[test]
fn the_entries_do_not_survive_into_the_next_search() {
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut first = Board::from_fen(MIDDLEGAME).expect("the middlegame position");
    let mut second = Board::from_fen(FORCED).expect("the forced position");
    let mut s = Search::new(Limits::depth(6), &stop, &tt);
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
