// SPDX-License-Identifier: GPL-3.0-or-later

//! A cutoff raises a score and the sort follows, asserted as one chain, since either alone passes
//! on a mechanism that does nothing. The malus has its own gate, because a table that only credits
//! would pass every gate that does not look for a negative score.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Colour, Move, MoveList, START_FEN, generate_legal};
use cadence_engine::history::{self, HISTORY_MAX, History, SHIFT_MAX, SPAN};
use cadence_engine::picker::sort_from;
use cadence_engine::search::{Limits, history_reduction, lmr_reduction};
use support::table;

/// Deep enough that the table is well written before the last iteration sorts a list.
const ORDER_DEPTH: u32 = 6;

/// The reduction reads the tails of the score distribution, which need a well-written table, and
/// late move pruning gives up most of the refuted moves the malus marks. No reduction is lengthened
/// at depth eight on this position and some are at nine; ten is one past.
const MODULATION_DEPTH: u32 = 10;

/// The same quiet moves recur from node to node, so the score separates them; Kiwipete serves the
/// opposite purpose, a noisy prefix long enough to show the other bands undisturbed.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The search writes, the write discriminates between quiet moves, and the sort follows with the
/// noisy prefix unchanged; the first two links stop the third passing for the wrong reason.
#[test]
fn a_cutoff_raises_a_score_and_the_order_follows() {
    let fen = support::standard_fen("kiwipete");
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(&fen);
    let mut s = support::search(Limits::depth(ORDER_DEPTH), &stop, &tt);
    let best = s.run(&mut b, &mut Vec::new());
    assert!(!best.is_null(), "{fen}: no move");
    assert_eq!(
        s.completed_depth(),
        ORDER_DEPTH,
        "the search did not complete depth {ORDER_DEPTH}"
    );

    let us = b.side_to_move();
    let hist = s.history();
    let quiets: Vec<Move> = generate_legal(&b)
        .iter()
        .filter(|m| !m.is_noisy())
        .collect();
    assert!(quiets.len() > 1, "the position needs quiet moves to rank");

    let scores: Vec<i32> = quiets.iter().map(|&m| hist.get(us, m)).collect();
    let high = *scores.iter().max().expect("a quiet move");
    let low = *scores.iter().min().expect("a quiet move");
    assert!(
        high > 0,
        "depth {ORDER_DEPTH} on Kiwipete and no quiet move of the root's own list was ever credited"
    );
    assert!(
        high > low,
        "every quiet move of the root's list scores {high}: the table does not separate them"
    );

    let mut flat = generate_legal(&b);
    let mut ranked = flat.clone();
    sort_from(&b, &mut flat, 0, [Move::NULL; 2], &[]);
    sort_from(&b, &mut ranked, 0, [Move::NULL; 2], hist.side(us));
    let first_quiet = |l: &MoveList| {
        l.iter()
            .position(|m| !m.is_noisy())
            .expect("the list has a quiet move")
    };
    let cut = first_quiet(&flat);
    assert_eq!(
        cut,
        first_quiet(&ranked),
        "the history score moved a move out of the quiet band"
    );
    assert_eq!(
        flat.as_slice()[..cut],
        ranked.as_slice()[..cut],
        "the history score reordered the noisy moves"
    );
    assert_ne!(
        flat.as_slice()[cut],
        ranked.as_slice()[cut],
        "the table separates the quiet moves and the sort put the same one first anyway"
    );
    assert_eq!(
        hist.get(us, ranked.as_slice()[cut]),
        high,
        "the sort did not put the best-scoring quiet move first"
    );
}

/// The lengthening counter can only move if a reduction site holds a negative score, which only the
/// debit writes. Two counters, not a sum, so a credit-only table fails.
#[test]
fn the_malus_reaches_a_reduction_and_so_does_the_bonus() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(MIDDLEGAME);
    let mut s = support::search(Limits::depth(MODULATION_DEPTH), &stop, &tt);
    let best = s.run(&mut b, &mut Vec::new());
    assert!(!best.is_null(), "no move");
    assert_eq!(
        s.completed_depth(),
        MODULATION_DEPTH,
        "the search did not complete depth {MODULATION_DEPTH}"
    );
    assert!(
        s.lmr_reductions() > 0,
        "nothing was reduced, so nothing could be modulated"
    );
    assert!(
        s.history_reduced_less() > 0,
        "no history score ever shortened a reduction"
    );
    assert!(
        s.history_reduced_more() > 0,
        "no history score ever lengthened a reduction"
    );
}

/// Measured over a real search, since `picker`'s band width and the shift's divisor are sized
/// against this bound.
#[test]
fn a_real_search_leaves_every_entry_inside_the_bound() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(&support::standard_fen("kiwipete"));
    let mut s = support::search(Limits::depth(ORDER_DEPTH), &stop, &tt);
    let _ = s.run(&mut b, &mut Vec::new());
    for side in Colour::ALL {
        let row = s.history().side(side);
        assert_eq!(row.len(), SPAN, "{side:?}: the row is not the whole span");
        for (i, &v) in row.iter().enumerate() {
            assert!(
                v.abs() <= HISTORY_MAX,
                "{side:?} index {i}: {v} is outside the bound"
            );
        }
    }
}

/// With the transposition table cleared and the killers cleared by `run`, history is the only state
/// that could carry, and identical runs show it did not. `bench` builds a fresh `Search` per
/// position, so only a gate through one `Search` can tell the lifetime.
#[test]
fn the_table_does_not_survive_a_second_search() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(&support::standard_fen("kiwipete"));
    let mut s = support::search(Limits::depth(ORDER_DEPTH), &stop, &tt);

    let _ = s.run(&mut b, &mut Vec::new());
    let nodes = s.nodes();
    let first: Vec<i32> = Colour::ALL
        .iter()
        .flat_map(|&c| s.history().side(c).to_vec())
        .collect();
    assert!(
        first.iter().any(|&v| v != 0),
        "the first run wrote nothing to compare against"
    );

    tt.clear();
    let _ = s.run(&mut b, &mut Vec::new());
    assert_eq!(
        s.nodes(),
        nodes,
        "the second run of the same search searched a different tree"
    );
    let second: Vec<i32> = Colour::ALL
        .iter()
        .flat_map(|&c| s.history().side(c).to_vec())
        .collect();
    assert_eq!(
        first, second,
        "the second run started from the first's table"
    );
}

/// The scale is pinned too: at a scale of one the ageing term never engages and the cap is
/// decoration.
#[test]
fn the_bonus_is_the_scaled_square_until_the_cap() {
    assert_eq!(history::bonus(0), 0);
    assert_eq!(history::bonus(1), 16, "the scale moved");
    for depth in 0..=32u32 {
        assert_eq!(
            history::bonus(depth),
            i32::try_from(16 * depth * depth).expect("inside the cap"),
            "depth {depth}"
        );
    }
    assert_eq!(history::bonus(32), HISTORY_MAX, "the cap is not at 32");
    for depth in [33u32, 128, 1_000, 65_535, u32::MAX] {
        assert_eq!(history::bonus(depth), HISTORY_MAX, "depth {depth}");
    }
}

/// The diminishing return makes it ageing rather than accumulation, and is what leaving out a term
/// breaks while every other assertion still passes.
#[test]
fn the_update_is_bounded_and_its_return_diminishes() {
    let extremes = [
        i32::MIN,
        -HISTORY_MAX - 1,
        -HISTORY_MAX,
        -1,
        0,
        1,
        HISTORY_MAX,
        HISTORY_MAX + 1,
        i32::MAX,
    ];
    for e in extremes {
        for b in extremes {
            let v = history::apply(e, b);
            assert!(v.abs() <= HISTORY_MAX, "apply({e}, {b}) = {v}");
        }
    }
    for e in (-HISTORY_MAX..=HISTORY_MAX).step_by(257) {
        assert!(history::apply(e, 64) >= e, "a credit lowered {e}");
        assert!(history::apply(e, -64) <= e, "a debit raised {e}");
    }
    // The same bonus moves an empty entry further than a half-full one.
    let bonus = history::bonus(8);
    let from_empty = history::apply(0, bonus);
    let from_half = history::apply(HISTORY_MAX / 2, bonus) - HISTORY_MAX / 2;
    assert!(
        from_empty > from_half,
        "the credit did not shrink as the entry grew: {from_empty} against {from_half}"
    );
    assert_eq!(
        history::apply(HISTORY_MAX, HISTORY_MAX),
        HISTORY_MAX,
        "an entry at the cap left it"
    );
    assert_eq!(
        history::apply(-HISTORY_MAX, -HISTORY_MAX),
        -HISTORY_MAX,
        "an entry at the floor left it"
    );
    // Repeated credits converge on the cap and never pass it.
    let mut e = 0;
    for _ in 0..10_000 {
        e = history::apply(e, bonus);
    }
    assert!(e > HISTORY_MAX / 2, "credits did not accumulate: {e}");
    assert!(e <= HISTORY_MAX, "credits passed the cap: {e}");
}

/// Symmetric about zero, never more than `SHIFT_MAX` plies, never smaller for a larger score.
#[test]
fn the_shift_is_bounded_and_monotone() {
    assert_eq!(history::shift(0), 0, "a move with no score was moved");
    let mut previous = -SHIFT_MAX - 1;
    for h in (-HISTORY_MAX..=HISTORY_MAX).step_by(97) {
        let s = history::shift(h);
        assert!(s.abs() <= SHIFT_MAX, "history {h}: shift {s}");
        assert!(s >= previous, "history {h}: the shift fell to {s}");
        assert_eq!(history::shift(-h), -s, "history {h}: not symmetric");
        previous = s;
    }
    assert_eq!(history::shift(HISTORY_MAX), SHIFT_MAX, "the cap is the top");
    assert_eq!(
        history::shift(-HISTORY_MAX),
        -SHIFT_MAX,
        "the floor is the bottom"
    );
}

/// A base of zero comes back zero for every score, so every exemption `reduction` holds survives
/// the table.
#[test]
fn history_never_creates_a_reduction() {
    for h in (-HISTORY_MAX..=HISTORY_MAX).step_by(31) {
        assert_eq!(
            history_reduction(0, h),
            0,
            "history {h} created a reduction"
        );
    }
}

/// In both directions, and never below zero.
#[test]
fn history_moves_a_reduction_by_the_shift() {
    for depth in [3u32, 4, 8, 16, 31] {
        for index in [3usize, 4, 8, 16, 32, 64] {
            let base = lmr_reduction(depth, index);
            assert!(base > 0, "depth {depth} index {index}: no base to move");
            for h in (-HISTORY_MAX..=HISTORY_MAX).step_by(97) {
                let r = history_reduction(base, h);
                let want = base.saturating_add_signed(-history::shift(h));
                assert_eq!(r, want, "depth {depth} index {index} history {h}");
                assert!(
                    i64::from(r) <= i64::from(base) + i64::from(SHIFT_MAX),
                    "depth {depth} index {index} history {h}: {r} against {base}"
                );
            }
            assert!(
                history_reduction(base, HISTORY_MAX) <= base,
                "a credited move was reduced more"
            );
            assert!(
                history_reduction(base, -HISTORY_MAX) >= base,
                "a refuted move was reduced less"
            );
        }
    }
}

/// A promotion carries its piece above the twelve-bit index, so the check runs over real lists
/// rather than the mask.
#[test]
fn every_generated_move_indexes_inside_the_table() {
    let mut seen = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        for m in generate_legal(&b).iter() {
            assert!(m.from_to() < SPAN, "{fen}: {m:?} indexes past the table");
            seen += 1;
        }
    }
    assert!(seen > 0, "the corpus produced no moves");
}

/// Which makes the empty slice `picker` takes from a caller with none the same order as no table.
#[test]
fn a_fresh_table_reads_zero() {
    let h = History::new();
    let b = board(START_FEN);
    for side in Colour::ALL {
        assert!(h.side(side).iter().all(|&v| v == 0));
        for m in generate_legal(&b).iter() {
            assert_eq!(h.get(side, m), 0, "{m:?}");
        }
    }
}
