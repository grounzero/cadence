// SPDX-License-Identifier: GPL-3.0-or-later

//! The reductions happen and the re-search fires, so a reduced fail-high is verified rather than
//! trusted. The formula's size is pinned directly, and the counters are on no decision path, so the
//! assertions are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal};
use cadence_engine::search::{Limits, lmr_reduction, reduction};
use support::table;

/// Deep enough that the history-ordered sort still misjudges some late quiet moves: at eight the
/// set re-searches 10, 1 and 45 times.
const GATE_DEPTH: u32 = 8;

/// Where a late quiet move beating alpha is ordinary, unlike the start position and Kiwipete.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// Reductions are asserted per position, re-searches over the set: the ordering exists so a late
/// quiet move almost never beats alpha.
#[test]
fn a_middlegame_search_reduces_late_moves_and_verifies_fail_highs() {
    let mut researches = 0;
    for fen in [
        START_FEN.to_string(),
        support::standard_fen("kiwipete"),
        MIDDLEGAME.to_string(),
    ] {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = support::position(&fen);
        assert!(!generate_legal(&b).is_empty(), "{fen}: no legal moves");
        let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
        let best = s.run(&mut b, &mut Vec::new());
        assert!(!best.is_null(), "{fen}: no move");
        assert_eq!(
            s.completed_depth(),
            GATE_DEPTH,
            "{fen}: the search did not complete depth {GATE_DEPTH}"
        );
        assert!(
            s.lmr_reductions() > 0,
            "{fen}: depth {GATE_DEPTH} searched {} nodes and never reduced a late move",
            s.nodes()
        );
        researches += s.lmr_researches();
    }
    assert!(
        researches > 0,
        "no reduced search in the set beat alpha and was re-searched at full depth"
    );
}

/// The first three moves, and any node below depth three, already at most a ply from quiescence.
#[test]
fn no_reduction_below_the_thresholds() {
    for depth in 0..3 {
        for index in 0..64 {
            assert_eq!(
                lmr_reduction(depth, index),
                0,
                "depth {depth} index {index}"
            );
        }
    }
    for depth in 0..64 {
        for index in 0..3 {
            assert_eq!(
                lmr_reduction(depth, index),
                0,
                "depth {depth} index {index}"
            );
        }
    }
}

/// Further down the list or deeper in the tree is never reduced less.
#[test]
fn the_reduction_is_monotone_past_the_thresholds() {
    for depth in 3..64 {
        for index in 3..128 {
            let r = lmr_reduction(depth, index);
            assert!(r >= 1, "depth {depth} index {index}: no reduction");
            assert!(
                lmr_reduction(depth + 1, index) >= r,
                "depth {depth} index {index}: shrank with depth"
            );
            assert!(
                lmr_reduction(depth, index + 1) >= r,
                "depth {depth} index {index}: shrank with index"
            );
        }
    }
}

/// The same move at the same depth and index reduces with no exemption and not under each. Real
/// moves, so `is_noisy` is exercised against the generator.
#[test]
fn each_exemption_alone_refuses_the_reduction() {
    let none = [Move::NULL; 2];
    let quiet = generate_legal(&board(START_FEN))
        .iter()
        .find(|m| !m.is_noisy())
        .expect("the start position has a quiet move");
    assert!(
        reduction(false, false, quiet, none, 8, 8) > 0,
        "no exemption in force and no reduction"
    );
    assert_eq!(reduction(true, false, quiet, none, 8, 8), 0, "in check");
    assert_eq!(reduction(false, true, quiet, none, 8, 8), 0, "gives check");
    assert_eq!(
        reduction(false, false, quiet, [quiet, Move::NULL], 8, 8),
        0,
        "killer, first slot"
    );
    assert_eq!(
        reduction(false, false, quiet, [Move::NULL, quiet], 8, 8),
        0,
        "killer, second slot"
    );
    let noisy = generate_legal(&board(&support::standard_fen("kiwipete")))
        .iter()
        .find(|m| m.is_noisy())
        .expect("Kiwipete has a noisy move");
    assert_eq!(reduction(false, false, noisy, none, 8, 8), 0, "noisy");
}

/// Pinned cell by cell, one probe per band plus each band's edges on the diagonal.
#[test]
fn the_documented_table_is_the_table() {
    let table: [(u32, &[(usize, u32)]); 4] = [
        (3, &[(3, 1), (4, 1), (8, 1), (16, 2), (32, 2)]),
        (4, &[(3, 1), (4, 2), (8, 2), (16, 3), (32, 3)]),
        (8, &[(3, 1), (4, 2), (8, 3), (16, 4), (32, 4)]),
        (16, &[(3, 2), (4, 3), (8, 4), (16, 5), (32, 6)]),
    ];
    for (depth, cells) in table {
        for &(index, expected) in cells {
            assert_eq!(
                lmr_reduction(depth, index),
                expected,
                "depth {depth} index {index}"
            );
        }
    }
    // Band edges: the value is a function of the logarithm's band, so the
    // top of one band agrees with its bottom.
    assert_eq!(lmr_reduction(7, 7), lmr_reduction(4, 4));
    assert_eq!(lmr_reduction(15, 15), lmr_reduction(8, 8));
}
