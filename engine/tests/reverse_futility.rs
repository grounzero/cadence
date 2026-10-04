// SPDX-License-Identifier: GPL-3.0-or-later

//! The margin decides: beta moved a centipawn across the threshold makes the search one node on one
//! side and a tree on the other. The full-window refusal is gated through its own counter, and
//! `the_margin_is_the_depth_limit` pins the absent depth limit as a property.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::START_FEN;
use cadence_core::position::Board;
use cadence_engine::eval;
use cadence_engine::score::{Score, mate_in, mated_in};
use cadence_engine::search::{Limits, reverse_futile, reverse_futility_margin};
use cadence_engine::tune::Tunables;
use support::{PAWN_ENDGAMES, table};

/// The compiled-in values every gate here pins.
const DEFAULT: &Tunables = &Tunables::DEFAULT;

/// The start position first fires at depth eight; nine is one past it, so a tree change cannot
/// empty the gate in silence.
const GATE_DEPTH: u32 = 9;

/// Nothing is en prise, so the static evaluation is a reading the rule can be asked about.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// Returns the score, the nodes taken and the two counters.
fn one_node(fen: &str, depth: u32, alpha: Score, beta: Score) -> (Score, u64, u64, u64) {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(fen);
    let mut s = support::search(Limits::depth(depth), &stop, &tt);
    let score = s.node_window(&mut b, depth, 0, alpha, beta);
    (
        score,
        s.nodes(),
        s.reverse_futility_cutoffs(),
        s.reverse_futility_refused_by_window(),
    )
}

/// At depth one every child is quiescence, so the node count speaks for this node alone, and it
/// carries the gate: one node on the admitted side. Beta sits at `eval -
/// reverse_futility_margin(DEFAULT, 1)` and then a centipawn higher.
#[test]
fn the_margin_is_what_returns_the_node() {
    let eval = eval::evaluate(&board(MIDDLEGAME));
    let beta = eval - reverse_futility_margin(DEFAULT, 1);

    let (score, nodes, cutoffs, _) = one_node(MIDDLEGAME, 1, beta - 1, beta);
    assert_eq!(cutoffs, 1, "the margin did not return the node at beta");
    assert_eq!(
        nodes, 1,
        "the node was returned on the margin and {nodes} nodes were searched"
    );
    assert_eq!(
        score,
        eval - reverse_futility_margin(DEFAULT, 1),
        "the node came back at something other than the bound the condition established"
    );

    let (_, nodes_above, cutoffs_above, _) = one_node(MIDDLEGAME, 1, beta, beta + 1);
    assert_eq!(
        cutoffs_above, 0,
        "one centipawn above the margin the node was still returned"
    );
    assert!(
        nodes_above > 1,
        "one centipawn above the margin the node was still not searched"
    );
}

/// The rule returns a bound, which the principal variation cannot use. Same beta as above with
/// alpha a hundred below, so only the window's width differs.
#[test]
fn a_full_window_node_that_clears_the_margin_is_searched() {
    let eval = eval::evaluate(&board(MIDDLEGAME));
    let beta = eval - reverse_futility_margin(DEFAULT, 1);
    let (_, nodes, cutoffs, refused) = one_node(MIDDLEGAME, 1, beta - 100, beta);
    assert_eq!(cutoffs, 0, "a full-window node was returned on the margin");
    assert_eq!(
        refused, 1,
        "the window refusal decided nothing here, so the zero above covers nothing"
    );
    assert!(nodes > 1, "the full-window node was not searched");
}

/// No static evaluation in check, so the exemption is that absence rather than a condition to
/// remember.
#[test]
fn a_node_in_check_is_never_returned() {
    for depth in 0..8 {
        for beta in [-30_000, -100, 0, 100, 30_000] {
            assert!(
                reverse_futile(DEFAULT, None, depth, beta).is_none(),
                "depth {depth} beta {beta}"
            );
        }
    }
}

/// A mate score is not commensurable with a centipawn margin, and a claim resting on no search
/// proves nothing about a forced mate.
#[test]
fn a_mate_beta_refuses_the_margin() {
    for depth in 1..8 {
        for ply in 0..8 {
            assert!(
                reverse_futile(DEFAULT, Some(30_000), depth, mate_in(ply)).is_none(),
                "depth {depth}: mate in {ply}"
            );
            assert!(
                reverse_futile(DEFAULT, Some(30_000), depth, mated_in(ply)).is_none(),
                "depth {depth}: mated in {ply}"
            );
        }
    }
}

/// A pawn and a half per ply, growing with the search under the node.
#[test]
fn the_margin_is_the_documented_margin() {
    assert_eq!(reverse_futility_margin(DEFAULT, 1), 150);
    assert_eq!(reverse_futility_margin(DEFAULT, 2), 300);
    assert_eq!(reverse_futility_margin(DEFAULT, 3), 450);
    assert_eq!(reverse_futility_margin(DEFAULT, 6), 900);
    for depth in 1..16 {
        assert!(
            reverse_futility_margin(DEFAULT, depth + 1) > reverse_futility_margin(DEFAULT, depth),
            "depth {depth}: the margin did not grow"
        );
    }
}

/// Returning either evaluation, or a strict comparison, fails the sweep, which lands on the
/// threshold exactly.
#[test]
fn a_cut_returns_beta_where_the_margin_clears_it() {
    let mut fired = 0;
    for depth in 1..8 {
        for eval in [-2_000, -150, 0, 150, 450, 1_000, 5_000] {
            for beta in [-1_000, -150, 0, 150, 900] {
                let clears = eval - reverse_futility_margin(DEFAULT, depth) >= beta;
                let cut = reverse_futile(DEFAULT, Some(eval), depth, beta);
                assert_eq!(
                    cut,
                    clears.then_some(beta),
                    "depth {depth}, eval {eval}, beta {beta}"
                );
                fired += usize::from(clears);
            }
        }
    }
    assert!(
        fired > 20,
        "the sweep fired {fired} times and covers little"
    );
}

/// The rule has no depth test: the margin grows with depth while the evidence does not. A fixed gap
/// admits a band of shallow depths, and ten times the gap buys ten times the band, which a depth
/// constant could not.
#[test]
fn the_margin_is_the_depth_limit() {
    // 600 centipawns above beta: four plies of margin exactly, and the
    // fifth is one the evidence does not cover.
    for depth in 1..=4 {
        assert!(
            reverse_futile(DEFAULT, Some(600), depth, 0).is_some(),
            "depth {depth}: 600 centipawns did not cover {} of margin",
            reverse_futility_margin(DEFAULT, depth)
        );
    }
    for depth in 5..64 {
        assert!(
            reverse_futile(DEFAULT, Some(600), depth, 0).is_none(),
            "depth {depth}: the margin did not outrun a gap of 600"
        );
    }
    // The band is a function of the evidence and not of a constant.
    assert!(reverse_futile(DEFAULT, Some(150), 1, 0).is_some());
    assert!(reverse_futile(DEFAULT, Some(150), 2, 0).is_none());
    assert!(reverse_futile(DEFAULT, Some(6_000), 40, 0).is_some());
    assert!(reverse_futile(DEFAULT, Some(6_000), 41, 0).is_none());
}

/// A rule wired in but never admitted fails it; the sharper claims are the gates at the head of
/// this file.
#[test]
fn a_middlegame_search_returns_nodes_on_the_margin() {
    for fen in [
        START_FEN.to_string(),
        support::standard_fen("kiwipete"),
        MIDDLEGAME.to_string(),
    ] {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = support::position(&fen);
        let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
        let best = s.run(&mut b, &mut Vec::new());
        assert!(!best.is_null(), "{fen}: no move");
        assert_eq!(
            s.completed_depth(),
            GATE_DEPTH,
            "{fen}: the search did not complete depth {GATE_DEPTH}"
        );
        assert!(
            s.reverse_futility_cutoffs() > 0,
            "{fen}: depth {GATE_DEPTH} searched {} nodes and the margin returned none",
            s.nodes()
        );
    }
}

/// This rule compares a reading against a bound and does not pass, so it takes no material guard.
/// `tests/pruning.rs` asserts the null move is never tried on these positions; here the margin
/// returns nodes, so a guard added for symmetry breaks this.
#[test]
fn a_pawn_endgame_takes_the_margin_where_the_null_move_refuses_it() {
    for fen in PAWN_ENDGAMES {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = support::position(fen);
        let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
        let _ = s.run(&mut b, &mut Vec::new());
        assert_eq!(
            s.completed_depth(),
            GATE_DEPTH,
            "{fen}: the search did not complete depth {GATE_DEPTH}"
        );
        assert_eq!(
            s.null_attempts(),
            0,
            "{fen}: the premise moved, and the null move now runs here"
        );
        assert!(
            s.reverse_futility_cutoffs() > 0,
            "{fen}: the margin returned no node in a tree the null move refuses entirely"
        );
    }
}
