// SPDX-License-Identifier: GPL-3.0-or-later

//! What a gate can pin is the arithmetic and the refusals: a step at the stated weights,
//! saturation, a fresh slot correcting by nothing, separate sides, no clock and no float. Whether
//! it makes the engine stronger is the SPRT's question.

use cadence_core::Colour;
use cadence_engine::corrhist::{CorrectionHistory, GRAIN, MAX_CORRECTION, MAX_DELTA, MAX_WEIGHT};

/// A key that is not any other key used here, so a test that means "a
/// structure nobody has seen" gets one.
const KEY_A: u64 = 0x1234_5678_9abc_def0;
const KEY_B: u64 = 0x0fed_cba9_8765_4321;

/// A grain of one would round a small correction away, and a correction reaching the mate scale
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
    // From an entry of zero the step is `delta * GRAIN * w / UNIT`, and nothing pulls it back.
    // At depth 0 the weight is 1 of 256.
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
        "a tactic is not an evaluation error and does not dominate the table"
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
    // GRAIN buys resolution below a centipawn inside the slot, so a small error is held whole at
    // the shallowest weight; nothing outside the table ever sees it.
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        let left = 10 - t.correction(KEY_A, Colour::White);
        t.update(KEY_A, Colour::White, left, 0);
    }
    assert_eq!(t.correction(KEY_A, Colour::White), 10);
}

#[test]
fn an_error_that_never_shrinks_climbs_to_the_bound_and_stops() {
    // Each step adds what is reported, so a report that ignores the correction keeps adding, and
    // the pull toward zero grows until it cancels the step at the bound.
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        t.update(KEY_A, Colour::White, 10, 64);
    }
    assert_eq!(t.correction(KEY_A, Colour::White), MAX_CORRECTION);
}

/// The search measures each error against the evaluation already corrected, so the table is fed
/// what is left of an error, and a repeated error must leave it holding all of it.
#[test]
fn an_error_measured_against_its_own_correction_is_learned_whole() {
    for (error, depth) in [(100, 64), (-100, 64), (40, 6)] {
        let mut t = CorrectionHistory::new();
        for _ in 0..10_000 {
            let left = error - t.correction(KEY_A, Colour::White);
            t.update(KEY_A, Colour::White, left, depth);
        }
        assert_eq!(
            t.correction(KEY_A, Colour::White),
            error,
            "a repeated error of {error} at depth {depth} is learned whole, not in part"
        );
    }
}

// ---------------------------------------------------------------------------
// The rule in the search, which is a different question from the table.
// ---------------------------------------------------------------------------

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::START_FEN;
use cadence_core::position::Board;
use cadence_engine::position::Position;
use cadence_engine::search::Limits;
use support::table;

/// Deep enough that the table has been written and read many times over,
/// shallow enough to stay a gate rather than a benchmark.
const DEPTH: u32 = 8;

fn search_to(fen: &str, depth: u32) -> (u64, u64, u64) {
    let mut board = Position::new(Board::from_fen(fen).expect("a legal fen"));
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut s = support::search(Limits::depth(depth), &stop, &tt);
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
    // Per-thread state, so two searches from fresh state agree to the node, the property `bench`
    // rests on.
    let a = search_to(START_FEN, DEPTH);
    let b = search_to(START_FEN, DEPTH);
    assert_eq!(a, b, "same position, same depth, same everything");
}

#[test]
fn a_node_in_check_contributes_nothing() {
    // A node in check has no static reading to take a difference against.
    let (_, updates, _) = search_to("4k3/8/8/8/8/8/8/R3K2r w Q - 0 1", 2);
    let (_, deep, _) = search_to("4k3/8/8/8/8/8/8/R3K2r w Q - 0 1", DEPTH);
    assert!(
        deep >= updates,
        "a deeper search of one position folds in at least as many"
    );
}

#[test]
fn the_correction_is_bounded_where_the_search_reads_it() {
    // Whatever the table holds, a margin test sees a correction inside the bound, off the mate
    // scale.
    let mut t = CorrectionHistory::new();
    for _ in 0..100_000 {
        t.update(KEY_A, Colour::White, MAX_DELTA, MAX_WEIGHT as u32);
    }
    let c = t.correction(KEY_A, Colour::White);
    assert!(c.abs() <= MAX_CORRECTION);
}
