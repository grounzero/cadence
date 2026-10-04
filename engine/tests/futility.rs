// SPDX-License-Identifier: GPL-3.0-or-later

//! The margin is what decides: one position twice at one depth, alpha moved a centipawn across the
//! threshold, skips on one side and none on the other. An exempt quiet check is proved by the mate
//! it finds, and the counters are on no decision path, so the assertions are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal};
use cadence_engine::eval;
use cadence_engine::score::{MATE_IN_MAX_PLY, Score, mate_in, mated_in};
use cadence_engine::search::{Limits, futile_node, futility_margin, futility_skips};
use support::table;

/// Seven is the first depth every position in the set admits and skips; eight is one past it, so
/// the next tree change cannot silently empty the gate.
const GATE_DEPTH: u32 = 8;

/// Nothing is en prise, so the static evaluation is a reading the rule can be asked about.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// White is three queens down; Ra8 mates along an empty file, quiet and giving check, and every
/// other quiet move is futile.
const QUIET_MATE: &str = "6k1/5ppp/8/8/8/8/1qqq4/R5K1 w - - 0 1";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// Inside the null window `(alpha, alpha+1)`, with a table of its own.
fn one_node(fen: &str, depth: u32, alpha: Score) -> (Score, u64, u64, u64) {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(fen);
    let mut s = support::search(Limits::depth(depth), &stop, &tt);
    let score = s.node_window(&mut b, depth, 0, alpha, alpha + 1);
    (
        score,
        s.futility_nodes(),
        s.futility_skipped(),
        s.futility_kept_check(),
    )
}

/// At depth one every child is quiescence, which has no margin rule, so the counters are the root's
/// own. Alpha sits exactly at `eval + futility_margin(1)` and then a centipawn lower; a rule keyed
/// on anything but the margin answers the same on both.
#[test]
fn the_margin_is_what_skips_the_move() {
    let eval = eval::evaluate(&board(MIDDLEGAME));
    let threshold = eval + futility_margin(1);

    let (_, nodes_at, skipped_at, _) = one_node(MIDDLEGAME, 1, threshold);
    assert_eq!(nodes_at, 1, "the margin did not admit the node at alpha");
    assert!(
        skipped_at > 0,
        "the node was admitted and not one quiet move was skipped"
    );

    let (_, nodes_below, skipped_below, _) = one_node(MIDDLEGAME, 1, threshold - 1);
    assert_eq!(
        nodes_below, 0,
        "one centipawn below the margin the node was still admitted"
    );
    assert_eq!(
        skipped_below, 0,
        "one centipawn below the margin {skipped_below} moves were skipped"
    );
}

/// The first move is exempt for this reason: without it a node returns the sentinel it started
/// from, not a wrong score.
#[test]
fn a_node_that_skips_everything_still_has_an_answer() {
    let eval = eval::evaluate(&board(MIDDLEGAME));
    let (score, nodes, skipped, _) = one_node(MIDDLEGAME, 1, eval + futility_margin(1));
    assert_eq!(nodes, 1, "the node was not admitted");
    assert!(skipped > 0, "nothing was skipped");
    assert!(
        score > mated_in(0) && score < mate_in(0),
        "the node returned {score}, which is not a score a move produced"
    );
}

/// Admitted nodes prove the margin's test fires somewhere real; skipped moves prove the loop acts
/// on it. Never admitted passes neither; admitted only where every move is exempt passes the first.
#[test]
fn a_middlegame_search_skips_quiet_moves_near_the_horizon() {
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
            s.futility_nodes() > 0,
            "{fen}: depth {GATE_DEPTH} searched {} nodes and the margin admitted none",
            s.nodes()
        );
        assert!(
            s.futility_skipped() > 0,
            "{fen}: {} nodes admitted and not one quiet move skipped",
            s.futility_nodes()
        );
    }
}

/// The mate score can only come from searching a move the margin would skip; the other assertions
/// show the rule was live. Without the check exemption the same node returns -2,533 and skips 13
/// moves.
#[test]
fn a_quiet_check_survives_the_margin_and_the_mate_is_found() {
    let b = board(QUIET_MATE);
    let eval = eval::evaluate(&b);
    let alpha = 0;
    assert!(
        futile_node(Some(eval), 3, alpha),
        "the gate's own node is not futile: eval {eval} against alpha {alpha}"
    );

    let (score, nodes, skipped, kept) = one_node(QUIET_MATE, 3, alpha);
    assert!(
        score >= MATE_IN_MAX_PLY,
        "the mate was not found: {score}, {skipped} moves skipped"
    );
    assert!(nodes > 0, "the margin admitted no node");
    assert!(skipped > 0, "the rule was not live: nothing was skipped");
    assert!(
        kept > 0,
        "no move was kept for giving check, so the exemption decided nothing here"
    );
}

/// No static evaluation in check, so the exemption is that absence rather than a condition to
/// remember.
#[test]
fn a_node_in_check_is_never_futile() {
    for depth in 0..8 {
        for alpha in [-30_000, -100, 0, 100, 30_000] {
            assert!(
                !futile_node(None, depth, alpha),
                "depth {depth} alpha {alpha}"
            );
        }
    }
}

/// A node four plies from the horizon searches every move it generates.
#[test]
fn no_pruning_past_the_depth_limit() {
    // Two below the evaluation's own bound, so the gap is as wide as the
    // scale allows and only the depth can be refusing.
    let eval = -29_000;
    let alpha = 29_000;
    assert!(futile_node(Some(eval), 3, alpha), "depth three");
    for depth in 4..64 {
        assert!(
            !futile_node(Some(eval), depth, alpha),
            "depth {depth}: the gap decided where the limit should have"
        );
    }
}

/// A mate score is not commensurable with a centipawn margin, and an alpha naming a mate may hold a
/// shorter one.
#[test]
fn a_mate_alpha_refuses_the_margin() {
    for depth in 1..4 {
        for ply in 0..8 {
            assert!(
                !futile_node(Some(0), depth, mate_in(ply)),
                "depth {depth}: mate in {ply}"
            );
            assert!(
                !futile_node(Some(0), depth, mated_in(ply)),
                "depth {depth}: mated in {ply}"
            );
        }
    }
}

/// A pawn and a half per ply, growing with the search under the node.
#[test]
fn the_margin_is_the_documented_margin() {
    assert_eq!(futility_margin(1), 150);
    assert_eq!(futility_margin(2), 300);
    assert_eq!(futility_margin(3), 450);
    for depth in 1..8 {
        assert!(
            futility_margin(depth + 1) > futility_margin(depth),
            "depth {depth}: the margin did not grow"
        );
    }
}

/// The same quiet move at the same index is a candidate with no exemption in force and not one
/// under each. Real moves, so `is_noisy` is exercised against the generator.
#[test]
fn each_exemption_alone_keeps_the_move() {
    let quiet = generate_legal(&board(START_FEN))
        .iter()
        .find(|m| !m.is_noisy())
        .expect("the start position has a quiet move");
    assert!(
        futility_skips(true, quiet, 5),
        "no exemption in force and no skip"
    );
    assert!(!futility_skips(false, quiet, 5), "the node is not futile");
    assert!(!futility_skips(true, quiet, 0), "the node's first move");
    let noisy = generate_legal(&board(&support::standard_fen("kiwipete")))
        .iter()
        .find(|m| m.is_noisy())
        .expect("Kiwipete has a noisy move");
    assert!(!futility_skips(true, noisy, 5), "noisy");
    assert!(!futility_skips(true, Move::NULL, 0), "the null move");
}
