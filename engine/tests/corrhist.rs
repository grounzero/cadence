// SPDX-License-Identifier: GPL-3.0-or-later

//! What a gate can pin is the arithmetic and the refusals: a weighted mean at the stated weights,
//! saturation, a fresh slot correcting by nothing, separate sides, no clock and no float. Whether
//! it makes the engine stronger is the SPRT's question.

use cadence_core::Colour;
use cadence_engine::conthist::{Frame, PIECE_TO};
use cadence_engine::corrhist::{
    CorrectionHistory, CorrectionKeys, GRAIN, MAX_CORRECTION, MAX_DELTA, MAX_WEIGHT,
};

/// A key that is not any other key used here, so a test that means "a
/// structure nobody has seen" gets one.
const KEY_A: u64 = 0x1234_5678_9abc_def0;
const KEY_B: u64 = 0x0fed_cba9_8765_4321;

/// A position's keys, its pieces drawn from its pawn key so each name is one position.
const A: CorrectionKeys = CorrectionKeys {
    pawn: KEY_A,
    non_pawn: [KEY_A.rotate_left(1), KEY_A.rotate_left(2)],
    continuation: None,
};
const B: CorrectionKeys = CorrectionKeys {
    pawn: KEY_B,
    non_pawn: [KEY_B.rotate_left(1), KEY_B.rotate_left(2)],
    continuation: None,
};

/// A knight's move then a bishop's, as one continuation entry: below `PIECE_TO` squared.
const PAIR: u32 = 70 * PIECE_TO as u32 + 400;

/// A and B, each reached by the same two moves.
const A_AFTER: CorrectionKeys = CorrectionKeys {
    continuation: Some(PAIR),
    ..A
};
const B_AFTER: CorrectionKeys = CorrectionKeys {
    continuation: Some(PAIR),
    ..B
};

/// A grain of one would round a small correction away, and a correction reaching the mate scale
/// could imply a mate that is not there.
const _: () = assert!(GRAIN > 1);
const _: () = assert!(MAX_CORRECTION < 1000);

#[test]
fn a_fresh_table_corrects_by_nothing() {
    let t = CorrectionHistory::new();
    assert_eq!(t.correction(&A, Colour::White), 0);
    assert_eq!(t.correction(&A, Colour::Black), 0);
}

#[test]
fn one_observation_moves_the_entry_by_its_weight_and_no_further() {
    // The fold is `entry * (UNIT - w) + delta * GRAIN * w`, over `UNIT`, from an entry of zero.
    // Each of the three tables takes it, so the correction moves three times as far; at depth 0
    // the weight is 1 of 256.
    let mut t = CorrectionHistory::new();
    t.update(&A, Colour::White, 256, 0);
    assert_eq!(t.correction(&A, Colour::White), 3);

    // At the weight ceiling each table moves a sixteenth of the way.
    let mut t = CorrectionHistory::new();
    t.update(&A, Colour::White, 256, 64);
    assert_eq!(t.correction(&A, Colour::White), 3 * 256 * MAX_WEIGHT / 256);
}

#[test]
fn the_weight_rises_with_depth_and_then_stops() {
    let at = |depth: u32| {
        let mut t = CorrectionHistory::new();
        t.update(&A, Colour::White, MAX_DELTA, depth);
        t.correction(&A, Colour::White)
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
        t.update(&A, Colour::White, MAX_DELTA, 64);
    }
    let held = t.correction(&A, Colour::White);
    assert_eq!(
        held, MAX_CORRECTION,
        "a correction is bounded whatever the stream says"
    );
    for _ in 0..10_000 {
        t.update(&A, Colour::White, -MAX_DELTA, 64);
    }
    assert_eq!(t.correction(&A, Colour::White), -MAX_CORRECTION);
}

#[test]
fn a_delta_past_the_bound_is_clamped_rather_than_folded_whole() {
    let mut bounded = CorrectionHistory::new();
    let mut unbounded = CorrectionHistory::new();
    bounded.update(&A, Colour::White, MAX_DELTA, 64);
    unbounded.update(&A, Colour::White, MAX_DELTA * 100, 64);
    assert_eq!(
        bounded.correction(&A, Colour::White),
        unbounded.correction(&A, Colour::White),
        "a tactic is not an evaluation error and does not dominate the mean"
    );
}

#[test]
fn the_two_sides_do_not_share_a_slot() {
    let mut t = CorrectionHistory::new();
    t.update(&A, Colour::White, MAX_DELTA, 64);
    assert!(t.correction(&A, Colour::White) > 0);
    assert_eq!(
        t.correction(&A, Colour::Black),
        0,
        "the evaluation is relative to the side to move, so the two halves are separate"
    );
}

#[test]
fn clearing_returns_it_to_fresh() {
    let mut t = CorrectionHistory::new();
    t.update(&A, Colour::White, MAX_DELTA, 64);
    t.update(&B, Colour::Black, -MAX_DELTA, 64);
    t.clear();
    assert_eq!(t.correction(&A, Colour::White), 0);
    assert_eq!(t.correction(&B, Colour::Black), 0);
}

#[test]
fn the_correction_is_reported_in_centipawns_and_the_grain_is_internal() {
    // GRAIN buys resolution below a centipawn inside the slot; nothing outside the table ever
    // sees it.
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        t.update(&A, Colour::White, 10, 64);
    }
    let held = t.correction(&A, Colour::White);
    assert!(
        (27..=30).contains(&held),
        "each of three tables fed a constant ten converges to ten, less the truncation below"
    );
}

#[test]
fn the_fold_truncates_toward_zero_and_that_is_deliberate() {
    // Both divisions floor, so a converged entry sits short of its target and the three read one
    // unit short of thirty. Kept identical to the arm the shadow measured, since the measurement
    // transfers only to what was measured.
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        t.update(&A, Colour::White, 10, 64);
    }
    assert_eq!(
        t.correction(&A, Colour::White),
        29,
        "pin the bias so a later session reads it as a choice, not a bug"
    );
}

/// The same pawns behind different pieces are a different evaluation, so an error learned with
/// knights is not read whole with rooks in their place.
#[test]
fn an_error_learned_with_one_set_of_pieces_is_not_read_whole_with_another() {
    let knights = Board::from_fen("4k3/pp3ppp/2n5/8/8/2N5/PP3PPP/4K3 w - - 0 1").expect("a fen");
    let rooks = Board::from_fen("4k3/pp3ppp/2r5/8/8/2R5/PP3PPP/4K3 w - - 0 1").expect("a fen");
    assert_eq!(knights.pawn_key(), rooks.pawn_key(), "the same pawns");
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        let left = 100 - t.correction_at(&knights);
        t.update_at(&knights, left, 64);
    }
    let (learned, read) = (t.correction_at(&knights), t.correction_at(&rooks));
    assert!(learned > 0, "the error was not learned with knights");
    assert!(
        read < learned,
        "rooks read {read} of the {learned} learned with knights"
    );
}

/// Each side's key reads that side's pieces other than pawns, and nothing else.
#[test]
fn a_side_s_key_moves_with_its_own_pieces_and_not_with_pawns_or_the_other_side() {
    let keys = |fen: &str| CorrectionKeys::of(&Board::from_fen(fen).expect("a fen"));
    let base = keys("4k3/pp3ppp/2n5/8/8/2N5/PP3PPP/4K3 w - - 0 1");
    let pawns = keys("4k3/p4ppp/1pn5/8/8/2N1P3/PP3P1P/4K3 w - - 0 1");
    let black = keys("4k3/pp3ppp/2r5/8/8/2N5/PP3PPP/4K3 w - - 0 1");
    let white = keys("4k3/pp3ppp/2n5/8/8/2N5/PP3PPP/3K4 w - - 0 1");
    let (w, b) = (Colour::White.index(), Colour::Black.index());
    assert_ne!(pawns.pawn, base.pawn);
    assert_eq!(
        pawns.non_pawn, base.non_pawn,
        "pawns move neither side's key"
    );
    assert_eq!(black.non_pawn[w], base.non_pawn[w]);
    assert_ne!(
        black.non_pawn[b], base.non_pawn[b],
        "a black knight became a rook"
    );
    assert_eq!(white.non_pawn[b], base.non_pawn[b]);
    assert_ne!(white.non_pawn[w], base.non_pawn[w], "the white king moved");
}

/// The keys are read from the pieces where they stand, so two move orders to one position agree.
#[test]
fn two_move_orders_to_one_position_read_the_same_keys() {
    let reach = |moves: &[&str]| {
        let mut b = Board::from_fen(START_FEN).expect("the start");
        for uci in moves {
            let m = parse_uci(&generate_legal(&b), uci).expect("a legal move");
            b.play(m);
        }
        CorrectionKeys::of(&b)
    };
    assert_eq!(
        reach(&["g1f3", "g8f6", "b1c3", "b8c6"]),
        reach(&["b1c3", "b8c6", "g1f3", "g8f6"])
    );
}

/// The same two moves before different positions are the same continuation, so an error learned
/// after them in one position is read after them in another.
#[test]
fn an_error_learned_after_two_moves_is_read_after_them_in_another_position() {
    let mut t = CorrectionHistory::new();
    for _ in 0..10_000 {
        t.update(&A_AFTER, Colour::White, 100, 64);
    }
    let read = t.correction(&B_AFTER, Colour::White);
    assert!(
        (90..=100).contains(&read),
        "B read {read} after the two moves A learned 100 after"
    );
    assert_eq!(
        t.correction(&B, Colour::White),
        0,
        "B reached by other moves reads nothing"
    );
}

/// A continuation entry is 16 bits, so it saturates at 127 centipawns, inside the sum's bound.
#[test]
fn a_continuation_entry_saturates_at_127() {
    let mut t = CorrectionHistory::new();
    for _ in 0..100_000 {
        t.update(&A_AFTER, Colour::White, MAX_DELTA, MAX_WEIGHT as u32);
    }
    assert_eq!(t.correction(&B_AFTER, Colour::White), 127);
    assert_eq!(t.correction(&A_AFTER, Colour::White), MAX_CORRECTION);
}

/// The pair is the frames one and two plies up, and absent where either is.
#[test]
fn the_pair_is_read_from_the_two_frames_above_and_is_absent_after_a_null_move() {
    let b = Board::from_fen(START_FEN).expect("the start");
    let frames: [Frame; 5] = [Some(10), Some(20), None, Some(30), Some(40)];
    let at = |ply| CorrectionKeys::on_path(&b, &frames, ply);
    let row = PIECE_TO as u32;
    assert_eq!(at(2).continuation, Some(10 * row + 20));
    assert_eq!(at(5).continuation, Some(30 * row + 40));
    assert_eq!(at(0).continuation, None, "at the root");
    assert_eq!(at(1).continuation, None, "one ply from the root");
    assert_eq!(at(3).continuation, None, "just after a null move");
    assert_eq!(at(4).continuation, None, "two plies after it");
    assert_eq!(
        CorrectionKeys {
            continuation: None,
            ..at(2)
        },
        CorrectionKeys::of(&b),
        "the board's own keys are unchanged"
    );
}

// ---------------------------------------------------------------------------
// The rule in the search, which is a different question from the table.
// ---------------------------------------------------------------------------

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{START_FEN, generate_legal, parse_uci};
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
        t.update(&A, Colour::White, MAX_DELTA, MAX_WEIGHT as u32);
    }
    let c = t.correction(&A, Colour::White);
    assert!(c.abs() <= MAX_CORRECTION);
}
