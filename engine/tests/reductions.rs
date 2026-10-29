// SPDX-License-Identifier: GPL-3.0-or-later

//! The reductions happen and the re-search fires, so a reduced fail-high is verified rather than
//! trusted. The formula's size is pinned directly, and the counters are on no decision path, so the
//! assertions are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal, parse_uci};
use cadence_engine::history::HISTORY_MAX;
use cadence_engine::search::{
    Limits, REDUCTION_INDEX, cut_node_reduction, history_reduction, lmr_reduction, losing_capture,
    move_history, pv_reduction, reduction,
};
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
        reduction(false, false, quiet, false, none, 8, 8) > 0,
        "no exemption in force and no reduction"
    );
    assert_eq!(
        reduction(true, false, quiet, false, none, 8, 8),
        0,
        "in check"
    );
    assert_eq!(
        reduction(false, true, quiet, false, none, 8, 8),
        0,
        "gives check"
    );
    assert_eq!(
        reduction(false, false, quiet, false, [quiet, Move::NULL], 8, 8),
        0,
        "killer, first slot"
    );
    assert_eq!(
        reduction(false, false, quiet, false, [Move::NULL, quiet], 8, 8),
        0,
        "killer, second slot"
    );
    let noisy = generate_legal(&board(&support::standard_fen("kiwipete")))
        .iter()
        .find(|m| m.is_noisy())
        .expect("Kiwipete has a noisy move");
    assert_eq!(
        reduction(false, false, noisy, false, none, 8, 8),
        0,
        "noisy"
    );
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

/// A null-window node's moves keep the reduction the history term gave them.
#[test]
fn a_null_window_reduction_is_unchanged() {
    for r in 0..64 {
        assert_eq!(pv_reduction(r, false), r, "reduction {r}");
    }
}

/// One ply off at a PV node, so a move whose reduction would be one is searched unreduced.
#[test]
fn a_pv_move_takes_one_ply_less() {
    assert_eq!(pv_reduction(1, true), 0, "a reduction of one at a PV node");
    for r in 1..64 {
        assert_eq!(pv_reduction(r, true), r - 1, "reduction {r}");
    }
}

/// Zero stays zero for every base, history score and window, so the exemptions in `reduction` hold.
#[test]
fn the_pv_term_never_creates_a_reduction() {
    let scores = [
        -HISTORY_MAX,
        -HISTORY_MAX / 4,
        -HISTORY_MAX / 16,
        -1,
        0,
        1,
        HISTORY_MAX / 16,
        HISTORY_MAX / 4,
        HISTORY_MAX,
    ];
    for base in 0..16 {
        for history in scores {
            let shifted = history_reduction(base, history);
            for pv in [false, true] {
                let r = pv_reduction(shifted, pv);
                assert!(
                    r <= shifted,
                    "base {base} history {history} pv {pv}: raised"
                );
                if shifted == 0 {
                    assert_eq!(r, 0, "base {base} history {history} pv {pv}: created");
                }
            }
        }
    }
}

/// The term reaches the search: a depth-eight search of a middlegame lowers some PV reductions.
#[test]
fn a_search_lessens_some_pv_reductions() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(MIDDLEGAME);
    let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
    assert!(!s.run(&mut b, &mut Vec::new()).is_null(), "no move");
    assert!(
        s.lmr_pv_lessened() > 0,
        "depth {GATE_DEPTH} reduced {} late moves and lowered none at a PV node",
        s.lmr_reductions()
    );
}

/// A queen taking a pawn a pawn defends, a pawn taking a queen, and a knight taking a knight a pawn
/// defends: one exchange below zero, one above, one level.
const LOSING: (&str, &str) = ("4k3/8/2p5/3p4/8/8/3Q4/4K3 w - - 0 1", "d2d5");
const WINNING: (&str, &str) = ("4k3/8/8/3q4/4P3/8/8/4K3 w - - 0 1", "e4d5");
const LEVEL: (&str, &str) = ("4k3/8/2p5/3n4/8/4N3/8/4K3 w - - 0 1", "e3d5");

fn capture(case: (&str, &str)) -> (Board, Move) {
    let b = board(case.0);
    let m = parse_uci(&generate_legal(&b), case.1).expect("a legal capture");
    assert!(m.is_capture(), "{} is a capture", case.1);
    (b, m)
}

/// Only an exchange below zero is losing, and only at an index the reduction reaches.
#[test]
fn a_capture_is_losing_by_its_exchange_and_its_index() {
    for (case, losing) in [(LOSING, true), (WINNING, false), (LEVEL, false)] {
        let (b, m) = capture(case);
        assert_eq!(losing_capture(&b, m, REDUCTION_INDEX), losing, "{}", case.1);
        assert!(
            !losing_capture(&b, m, REDUCTION_INDEX - 1),
            "{} early",
            case.1
        );
    }
    let b = board(START_FEN);
    let quiet = generate_legal(&b)
        .iter()
        .find(|m| !m.is_noisy())
        .expect("a quiet move");
    assert!(!losing_capture(&b, quiet, REDUCTION_INDEX), "a quiet move");
}

/// One ply wherever a quiet move at the same depth and index would be reduced at all.
#[test]
fn a_losing_capture_is_reduced_one_ply_where_a_quiet_move_would_be() {
    let (_, m) = capture(LOSING);
    let none = [Move::NULL; 2];
    for depth in 0..24 {
        for index in 0..64 {
            let quiet = u32::from(lmr_reduction(depth, index) > 0);
            assert_eq!(
                reduction(false, false, m, true, none, depth, index),
                quiet,
                "depth {depth} index {index}"
            );
        }
    }
}

/// A capture that is not losing is never reduced, and a losing one keeps the check exemptions.
#[test]
fn a_capture_keeps_its_exemptions_unless_it_loses() {
    let none = [Move::NULL; 2];
    for case in [LOSING, WINNING, LEVEL] {
        let (_, m) = capture(case);
        for (depth, index) in [(3, 3), (8, 8), (20, 40)] {
            assert_eq!(
                reduction(false, false, m, false, none, depth, index),
                0,
                "{}",
                case.1
            );
            assert_eq!(
                reduction(true, false, m, true, none, depth, index),
                0,
                "in check"
            );
            assert_eq!(
                reduction(false, true, m, true, none, depth, index),
                0,
                "gives check"
            );
        }
    }
}

/// The quiet history does not score a capture, so a capture's reduction takes no shift from it.
#[test]
fn a_capture_takes_no_history_shift() {
    let (_, capture) = capture(LOSING);
    let quiet = generate_legal(&board(START_FEN))
        .iter()
        .find(|m| !m.is_noisy())
        .expect("a quiet move");
    for history in [-HISTORY_MAX, -1, 0, 1, HISTORY_MAX] {
        assert_eq!(move_history(capture, history), 0, "a capture at {history}");
        assert_eq!(
            move_history(quiet, history),
            history,
            "a quiet move at {history}"
        );
    }
}

/// The term reaches the search: a depth-eight search of a middlegame reduces some losing captures.
#[test]
fn a_search_reduces_some_losing_captures() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(MIDDLEGAME);
    let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
    assert!(!s.run(&mut b, &mut Vec::new()).is_null(), "no move");
    assert!(
        s.lmr_losing_captures() > 0,
        "depth {GATE_DEPTH} reduced {} late moves and no losing capture",
        s.lmr_reductions()
    );
}

/// An expected cut node takes one ply more on whatever reduction the other terms left.
#[test]
fn a_late_move_at_an_expected_cut_node_takes_one_ply_more() {
    for r in 1..16 {
        assert_eq!(cut_node_reduction(r, true), r + 1, "r {r}");
    }
}

/// Every other node keeps the reduction, and zero stays zero at a cut node too.
#[test]
fn the_cut_node_term_leaves_other_nodes_and_never_creates_a_reduction() {
    for r in 0..16 {
        assert_eq!(cut_node_reduction(r, false), r, "r {r}");
    }
    assert_eq!(cut_node_reduction(0, true), 0);
}

/// The term reaches the search: a depth-eight search of a middlegame raises some reductions at
/// expected cut nodes.
#[test]
fn a_search_reduces_more_at_some_expected_cut_nodes() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(MIDDLEGAME);
    let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
    assert!(!s.run(&mut b, &mut Vec::new()).is_null(), "no move");
    assert!(
        s.lmr_cut_reduced_more() > 0,
        "depth {GATE_DEPTH} reduced {} late moves and raised none at an expected cut node",
        s.lmr_reductions()
    );
}
