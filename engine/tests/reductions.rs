// SPDX-License-Identifier: GPL-3.0-or-later

//! Late move reductions: a move far down a sorted list is first searched
//! shallower than its siblings, and a reduced search that beats alpha is
//! re-run at full depth before its answer is believed.
//!
//! What these gates demonstrate is that the reductions **happen** and that
//! the **re-search fires**: the first counter proves later moves were
//! searched at reduced depth somewhere real, and the second proves that a
//! reduced fail-high was verified at full depth rather than trusted, which
//! is the half of the mechanism that keeps a shallow misjudgement out of
//! the score. Neither is "the code runs". The formula's own gates below
//! pin the size of the reduction directly, without a search.
//!
//! The counters these gates read are written wherever the rule runs and
//! read on no decision path, so a depth-limited search here reads no clock
//! and the assertions are exact, not statistical.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal};
use cadence_engine::search::{Limits, Search, lmr_reduction, reduction};
use support::table;

/// The depth the gates search to. Six, like the null-move gates: deep
/// enough that nodes with long sorted lists are plentiful, so both the
/// reduction and the re-search have somewhere real to fire.
const GATE_DEPTH: u32 = 6;

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The reductions happen, and the fail-highs are verified: a middlegame
/// search reduces late moves, and somewhere in the set a reduced search
/// beats alpha and is re-run at full depth.
///
/// Coverage first: each search completed the depth it was asked for, so
/// the counters were read off finished trees. Then the property, in two
/// halves that fail differently: reductions prove the conditions admit a
/// shallower first search somewhere real, per position, and re-searches
/// prove that a reduced search beating alpha is re-run at full depth,
/// which is the mechanism's safety half. A rule wired in but never
/// admitted passes neither; one that reduces but trusts the reduced
/// answer passes only the first. The re-search is asserted over the set
/// rather than per position, because it is the rare path by construction:
/// the ordering exists so that a late quiet move almost never beats
/// alpha, and Kiwipete at this depth reduces thousands of moves without
/// one doing so, while the start position re-searches a handful.
#[test]
fn a_middlegame_search_reduces_late_moves_and_verifies_fail_highs() {
    let mut researches = 0;
    for fen in [START_FEN.to_string(), support::standard_fen("kiwipete")] {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = board(&fen);
        assert!(!generate_legal(&b).is_empty(), "{fen}: no legal moves");
        let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
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

/// The formula refuses to reduce where there is nothing to reduce: the
/// first three moves of a node, and any node below depth three, whose
/// child search is at most one ply from the quiescence search already.
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

/// Past the thresholds every reduction is at least one ply, and the size
/// never falls as the depth or the index grows: a move further down the
/// list, or a node with more tree below it, is never reduced less.
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

/// Every exemption refuses the reduction, pinned directly: the same move
/// at the same depth and index reduces with no exemption in force and
/// does not reduce under each one alone. Real moves from real lists, so
/// `is_noisy` is exercised against the generator and not a hand-built
/// encoding.
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

/// The table in the formula's own comment, pinned cell by cell so the
/// comment and the code cannot drift: one band-representative probe per
/// cell, plus each band's edges on the diagonal.
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
