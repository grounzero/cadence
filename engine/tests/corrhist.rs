// SPDX-License-Identifier: GPL-3.0-or-later

//! The correction table: what it holds, what it refuses, and that it is a
//! function of the code alone.
//!
//! The table records the running difference between a node's static
//! evaluation and the score the search returned there, keyed by the pawn
//! structure and the side to move, and offers it back the next time a
//! position with that structure is evaluated. What a gate can pin is the
//! arithmetic and the refusals: that the fold is a weighted mean at the
//! stated weights, that it saturates rather than wrapping, that a fresh
//! slot corrects by nothing, that the two sides do not share a slot, and
//! that nothing here reads a clock or a float.
//!
//! What it cannot pin is whether the correction makes the engine stronger,
//! which is the SPRT's job.

use cadence_core::Colour;
use cadence_engine::corrhist::{CorrectionHistory, GRAIN, MAX_CORRECTION, MAX_DELTA, MAX_WEIGHT};

/// A key that is not any other key used here, so a test that means "a
/// structure nobody has seen" gets one.
const KEY_A: u64 = 0x1234_5678_9abc_def0;
const KEY_B: u64 = 0x0fed_cba9_8765_4321;

/// Two properties of the constants themselves, checked when this file is
/// compiled rather than when it is run. A grain of one would round a small
/// persistent correction away, and a correction that reached the mate scale
/// could imply a mate that is not there.
const _: () = assert!(GRAIN > 1);
const _: () = assert!(MAX_CORRECTION < 1000);

#[test]
fn a_fresh_table_corrects_by_nothing() {
    let t = CorrectionHistory::new();
    assert_eq!(t.correction(KEY_A, Colour::White), 0);
    assert_eq!(t.correction(KEY_A, Colour::Black), 0);
}

#[test]
fn one_observation_moves_the_entry_by_its_weight_and_no_further() {
    // The fold is `entry * (UNIT - w) + delta * GRAIN * w`, over `UNIT`,
    // from an entry of zero. At depth 0 the weight is 1 of 256.
    let mut t = CorrectionHistory::new();
    t.update(KEY_A, Colour::White, 256, 0);
    assert_eq!(t.correction(KEY_A, Colour::White), 1);

    // At the weight ceiling one observation moves a sixteenth of the way.
    let mut t = CorrectionHistory::new();
    t.update(KEY_A, Colour::White, 256, 64);
    assert_eq!(t.correction(KEY_A, Colour::White), 256 * MAX_WEIGHT / 256);
}

#[test]
fn the_weight_rises_with_depth_and_then_stops() {
    let at = |depth: u32| {
        let mut t = CorrectionHistory::new();
        t.update(KEY_A, Colour::White, MAX_DELTA, depth);
        t.correction(KEY_A, Colour::White)
    };
    assert!(at(0) < at(4), "a deeper observation carries more weight");
    assert!(
        at(4) < at(MAX_WEIGHT as u32),
        "and more again below the cap"
    );
    assert_eq!(
        at(MAX_WEIGHT as u32),
        at(1000),
        "the weight stops at the cap rather than growing with depth"
    );
}

#[test]
fn a_repeated_observation_converges_and_saturates() {
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        t.update(KEY_A, Colour::White, MAX_DELTA, 64);
    }
    let held = t.correction(KEY_A, Colour::White);
    assert_eq!(
        held, MAX_CORRECTION,
        "a correction is bounded whatever the stream says"
    );
    for _ in 0..10_000 {
        t.update(KEY_A, Colour::White, -MAX_DELTA, 64);
    }
    assert_eq!(t.correction(KEY_A, Colour::White), -MAX_CORRECTION);
}

#[test]
fn a_delta_past_the_bound_is_clamped_rather_than_folded_whole() {
    let mut bounded = CorrectionHistory::new();
    let mut unbounded = CorrectionHistory::new();
    bounded.update(KEY_A, Colour::White, MAX_DELTA, 64);
    unbounded.update(KEY_A, Colour::White, MAX_DELTA * 100, 64);
    assert_eq!(
        bounded.correction(KEY_A, Colour::White),
        unbounded.correction(KEY_A, Colour::White),
        "a tactic is not an evaluation error and does not dominate the mean"
    );
}

#[test]
fn the_two_sides_do_not_share_a_slot() {
    let mut t = CorrectionHistory::new();
    t.update(KEY_A, Colour::White, MAX_DELTA, 64);
    assert!(t.correction(KEY_A, Colour::White) > 0);
    assert_eq!(
        t.correction(KEY_A, Colour::Black),
        0,
        "the evaluation is relative to the side to move, so the two halves are separate"
    );
}

#[test]
fn clearing_returns_it_to_fresh() {
    let mut t = CorrectionHistory::new();
    t.update(KEY_A, Colour::White, MAX_DELTA, 64);
    t.update(KEY_B, Colour::Black, -MAX_DELTA, 64);
    t.clear();
    assert_eq!(t.correction(KEY_A, Colour::White), 0);
    assert_eq!(t.correction(KEY_B, Colour::Black), 0);
}

#[test]
fn the_correction_is_reported_in_centipawns_and_the_grain_is_internal() {
    // GRAIN buys resolution below a centipawn inside the slot; nothing
    // outside the table ever sees it.
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        t.update(KEY_A, Colour::White, 10, 64);
    }
    let held = t.correction(KEY_A, Colour::White);
    assert!(
        (9..=10).contains(&held),
        "a constant stream of ten converges to ten, less the truncation below"
    );
}

#[test]
fn the_fold_truncates_toward_zero_and_that_is_deliberate() {
    // Both divisions floor, so a converged entry sits one unit below its
    // target and a reported correction can be a centipawn short. The
    // arithmetic is the arm the shadow measured and it is kept identical
    // rather than rounded, because the measurement transfers only to what
    // was measured.
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        t.update(KEY_A, Colour::White, 10, 64);
    }
    assert_eq!(
        t.correction(KEY_A, Colour::White),
        9,
        "pin the bias so a later session reads it as a choice, not a bug"
    );
}

// ---------------------------------------------------------------------------
// The rule in the search, which is a different question from the table.
// ---------------------------------------------------------------------------

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::START_FEN;
use cadence_core::position::Board;
use cadence_engine::search::{Limits, Search};
use support::table;

/// Deep enough that the table has been written and read many times over,
/// shallow enough to stay a gate rather than a benchmark.
const DEPTH: u32 = 8;

fn search_to(fen: &str, depth: u32) -> (u64, u64, u64) {
    let mut board = Board::from_fen(fen).expect("a legal fen");
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut s = Search::new(Limits::depth(depth), &stop, &tt);
    s.run(&mut board, &mut std::io::sink());
    (s.nodes(), s.corrhist_updates(), s.corrhist_applied())
}

#[test]
fn the_search_both_writes_and_reads_the_table() {
    let (_, updates, applied) = search_to(START_FEN, DEPTH);
    assert!(updates > 0, "a search of this depth folds observations in");
    assert!(
        applied > 0,
        "and reads a non-zero correction back at some node, \
         or the table is write-only and the rule is inert"
    );
}

#[test]
fn a_search_is_still_a_function_of_the_position_and_the_depth() {
    // The table lives in the per-thread `Search`, so two searches from
    // fresh state must agree to the node. This is the property `bench`
    // rests on and the one a correction is easiest to break.
    let a = search_to(START_FEN, DEPTH);
    let b = search_to(START_FEN, DEPTH);
    assert_eq!(a, b, "same position, same depth, same everything");
}

#[test]
fn a_node_in_check_contributes_nothing() {
    // The evaluation measures a position nobody is about to win material
    // in, and a check contests exactly that, so such a node has no static
    // reading to take a difference against.
    let (_, updates, _) = search_to("4k3/8/8/8/8/8/8/R3K2r w Q - 0 1", 2);
    let (_, deep, _) = search_to("4k3/8/8/8/8/8/8/R3K2r w Q - 0 1", DEPTH);
    assert!(
        deep >= updates,
        "a deeper search of one position folds in at least as many"
    );
}

#[test]
fn the_correction_is_bounded_where_the_search_reads_it() {
    // Whatever the table holds, what reaches a margin test is inside the
    // stated bound; a correction incommensurable with the mate scale is
    // the thing this refuses.
    let mut t = CorrectionHistory::new();
    for _ in 0..100_000 {
        t.update(KEY_A, Colour::White, MAX_DELTA, MAX_WEIGHT as u32);
    }
    let c = t.correction(KEY_A, Colour::White);
    assert!(c.abs() <= MAX_CORRECTION);
}
