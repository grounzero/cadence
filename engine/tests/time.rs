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
use cadence_engine::time::{
    Budget, MOVE_OVERHEAD_MS, PROGRESS_MAX, another_iteration_fits, budget, progress,
};
use cadence_engine::tt::Table;
use support::Engine;

fn limits(s: &str) -> Limits {
    Limits::parse(s.split_whitespace())
}

fn clock(time: u64, inc: u64, movestogo: Option<u32>) -> Budget {
    clock_at(time, inc, movestogo, 0)
}

/// The same, at a stated point in the game. `clock` above takes the opening's
/// full complement, which is where the hard budget is largest and so where
/// every property about not overspending is hardest to hold.
fn clock_at(time: u64, inc: u64, movestogo: Option<u32>, progress: u32) -> Budget {
    let mut l = limits(&format!("wtime {time} btime {time} winc {inc} binc {inc}"));
    l.movestogo = movestogo;
    budget(&l, Colour::White, progress).expect("a clock gives a budget")
}

const TIMES: [u64; 9] = [1, 20, 21, 50, 100, 500, 1000, 8000, 3_600_000];
const INCS: [u64; 5] = [0, 10, 80, 400, 1000];
const MTG: [Option<u32>; 5] = [None, Some(1), Some(5), Some(20), Some(40)];
const PROGRESSES: [u32; 5] = [0, 6, 12, 18, PROGRESS_MAX];

/// Nothing constrains the time, so the clock is never read. "Nothing" is
/// exactly "the `go` named no clock at all": see the test below for the case
/// that used to be filed here and is not the same thing.
#[test]
fn no_constraint_means_no_budget() {
    for s in ["", "depth 6", "nodes 1000", "infinite", "depth 3 nodes 5"] {
        assert_eq!(budget(&limits(s), Colour::White, 0), None, "{s:?}");
        assert_eq!(budget(&limits(s), Colour::Black, 0), None, "{s:?}");
    }
    // Fixed depth is what `bench` runs under, and it must never consult a
    // clock; this is the contract at the allocation.
    assert_eq!(budget(&Limits::depth(7), Colour::White, 0), None);
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
            budget(&limits(line), us, 0),
            Some(Budget { soft: 0, hard: 0 }),
            "go {line} for {us:?}"
        );
    }
    // `movetime` and `infinite` are not clocks and are unaffected: the first
    // governs, and the second is refused a budget by the search itself.
    assert_eq!(
        budget(&limits("movetime 300 wtime 1000"), Colour::Black, 0),
        budget(&limits("movetime 300"), Colour::Black, 0)
    );
}

#[test]
fn movetime_is_the_budget_less_the_overhead() {
    let b = budget(&limits("movetime 500"), Colour::White, 0).expect("budget");
    assert_eq!(b.soft, 500 - MOVE_OVERHEAD_MS);
    assert_eq!(b.hard, 500 - MOVE_OVERHEAD_MS);
    // Under the overhead: nothing to spend, but still a budget -- the search
    // returns its first iteration and no more.
    let b = budget(&limits("movetime 5"), Colour::Black, 0).expect("budget");
    assert_eq!(b, Budget { soft: 0, hard: 0 });
    // movetime is the same for either side.
    assert_eq!(
        budget(&limits("movetime 300"), Colour::White, 0),
        budget(&limits("movetime 300"), Colour::Black, 0)
    );
}

#[test]
fn a_clock_gives_a_budget_bounded_by_half_of_what_is_left() {
    for &t in &TIMES {
        for &inc in &INCS {
            for &mtg in &MTG {
                // Every point in the game, because the cap and the ordering
                // of the two budgets are properties of the arithmetic and
                // not of where the game has got to.
                for &p in &PROGRESSES {
                    let b = clock_at(t, inc, mtg, p);
                    let avail = t.saturating_sub(MOVE_OVERHEAD_MS);
                    assert!(b.soft <= b.hard, "{t}+{inc} mtg {mtg:?} at {p}: {b:?}");
                    assert!(
                        b.hard <= avail / 2,
                        "{t}+{inc} mtg {mtg:?} at {p}: {b:?} vs avail {avail}"
                    );
                    if t <= MOVE_OVERHEAD_MS {
                        assert_eq!(
                            b,
                            Budget { soft: 0, hard: 0 },
                            "{t}+{inc} mtg {mtg:?} at {p}"
                        );
                    }
                    if t >= 1000 {
                        assert!(
                            b.soft >= 10,
                            "{t}+{inc} mtg {mtg:?} at {p}: {b:?} is stingy"
                        );
                    }
                }
            }
        }
    }
    // And Black's clock is read for Black.
    let l = limits("wtime 100 btime 8000 winc 0 binc 80");
    let w = budget(&l, Colour::White, 0).expect("w");
    let b = budget(&l, Colour::Black, 0).expect("b");
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
// How far into the game the position is
// ---------------------------------------------------------------------------

/// The scale runs from the opening's full complement to a pawn ending, and
/// it is read off material rather than off the move number. A move number
/// counts shuffling and a book exit alike, which is the reason the input is
/// not one.
#[test]
fn the_opening_is_no_progress_and_a_pawn_ending_is_all_of_it() {
    let start = Board::from_fen(START_FEN).expect("start");
    assert_eq!(progress(&start), 0);

    let pawns = Board::from_fen("4k3/pppppppp/8/8/8/8/PPPPPPPP/4K3 w - - 0 1").expect("pawns");
    assert_eq!(progress(&pawns), PROGRESS_MAX);

    // A rook ending is late but not the end of the scale, and a queenless
    // middlegame sits between the two.
    let rooks = Board::from_fen("r3k3/pppppppp/8/8/8/8/PPPPPPPP/R3K3 w Qq - 0 1").expect("rooks");
    let mid = Board::from_fen("r1b1kbnr/pppppppp/2n5/8/8/2N5/PPPPPPPP/R1B1KBNR w KQkq - 0 1")
        .expect("mid");
    assert!(progress(&rooks) > progress(&mid), "{rooks:?}");
    assert!(progress(&mid) > 0);
    assert!(progress(&rooks) < PROGRESS_MAX);
}

/// The hard budget moves with how far into the game the position is, and the
/// soft budget does not. The soft budget is this move's share of the clock
/// and nothing here changes it; the hard budget is how far past that share an
/// iteration already started may run, and that is what varies.
#[test]
fn game_progress_moves_the_hard_budget_and_leaves_the_soft_one_alone() {
    let mut moved = 0;
    for &t in &[1000u64, 8000, 3_600_000] {
        for &inc in &INCS {
            let opening = clock_at(t, inc, None, 0);
            let ending = clock_at(t, inc, None, PROGRESS_MAX);
            let cap = t.saturating_sub(MOVE_OVERHEAD_MS) / 2;
            assert_eq!(
                opening.soft, ending.soft,
                "{t}+{inc}: the share moved, {opening:?} against {ending:?}"
            );
            if ending.hard < cap {
                assert!(
                    ending.hard < opening.hard,
                    "{t}+{inc}: the hard budget did not move, {opening:?}"
                );
                moved += 1;
            } else {
                // The half-of-remaining cap binds at both ends, so the two
                // are equal and are the cap. That is the low-clock and
                // large-increment corner, where the budget is the cap's to
                // set and this rule has no room in it at all.
                assert_eq!(opening.hard, cap, "{t}+{inc}: {opening:?} vs cap {cap}");
                assert_eq!(ending.hard, cap, "{t}+{inc}: {ending:?} vs cap {cap}");
            }
        }
    }
    assert!(moved >= 8, "only {moved} clocks left the rule any room");
}

/// It falls as the game goes on, and never rises. One more completed
/// iteration changes the move chosen 23.0% of the time at the opening's full
/// complement against 7.8% near a pawn ending, so what an overrun buys is
/// largest early; the direction is the whole of the rule and this is the gate
/// that holds it.
#[test]
fn the_hard_budget_falls_as_the_game_goes_on() {
    for &t in &[1000u64, 8000, 60_000, 3_600_000] {
        for &inc in &INCS {
            for &mtg in &MTG {
                let mut last = u64::MAX;
                for p in 0..=PROGRESS_MAX {
                    let b = clock_at(t, inc, mtg, p);
                    assert!(
                        b.hard <= last,
                        "{t}+{inc} mtg {mtg:?} at {p}: {} rose from {last}",
                        b.hard
                    );
                    last = b.hard;
                }
                // Non-increasing is not the property on its own: a constant
                // satisfies it. Where the cap is not already binding at the
                // ending, it must actually fall from one end to the other.
                let opening = clock_at(t, inc, mtg, 0);
                let ending = clock_at(t, inc, mtg, PROGRESS_MAX);
                let cap = t.saturating_sub(MOVE_OVERHEAD_MS) / 2;
                if ending.hard < cap {
                    assert!(
                        ending.hard < opening.hard,
                        "{t}+{inc} mtg {mtg:?}: flat at {opening:?}"
                    );
                }
            }
        }
    }
}

/// The hard budget never falls below the soft one, at any point in the game.
///
/// This is not a tidiness property. `another_iteration_fits` returns early
/// when the two are equal, so a hard budget scaled under its soft budget
/// would silently switch off the promoted rule that refuses an iteration it
/// predicts cannot finish, and nothing else in this file would notice.
#[test]
fn the_hard_budget_never_falls_under_the_soft_one() {
    for &t in &TIMES {
        for &inc in &INCS {
            for &mtg in &MTG {
                for p in 0..=PROGRESS_MAX {
                    let b = clock_at(t, inc, mtg, p);
                    assert!(b.hard >= b.soft, "{t}+{inc} mtg {mtg:?} at {p}: {b:?}");
                    // And where they differ the rule can still say no, which
                    // is the half a bare ordering assertion would not catch.
                    if b.hard > b.soft {
                        let runaway = [1, 1, b.hard * 4];
                        assert!(
                            !another_iteration_fits(&runaway, b),
                            "{t}+{inc} mtg {mtg:?} at {p}: the rule went inert at {b:?}"
                        );
                    }
                }
            }
        }
    }
}

/// A budget scaled past the end of the scale is the end of the scale. The
/// caller derives the input from material and cannot exceed it, so this is
/// the clamp holding rather than a case that arises.
#[test]
fn progress_past_the_end_of_the_scale_is_the_end_of_it() {
    for &t in &[1000u64, 8000] {
        let end = clock_at(t, 80, None, PROGRESS_MAX);
        for p in [PROGRESS_MAX + 1, PROGRESS_MAX * 4, u32::MAX] {
            assert_eq!(clock_at(t, 80, None, p), end, "at {p}");
        }
    }
}

/// `movetime` names the budget exactly, so nothing about the position moves
/// it. Under `movetime` the two budgets are equal and the rule that refuses
/// an iteration is inert by design, which is what would otherwise be at risk
/// here.
#[test]
fn movetime_does_not_read_how_far_into_the_game_the_position_is() {
    for p in [0, 6, 12, 18, PROGRESS_MAX] {
        assert_eq!(
            budget(&limits("movetime 300"), Colour::White, p),
            budget(&limits("movetime 300"), Colour::White, 0),
            "at {p}"
        );
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
        if cum >= MEASURABLE_MS && 3 * soft <= 1_500 && cum + next > 3 * soft {
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
                cum < MEASURABLE_MS || cum + next <= 3 * soft,
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
        3 * soft
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
    let movetime = budget(&limits("movetime 200"), Colour::White, 0).expect("budget");
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
