// SPDX-License-Identifier: GPL-3.0-or-later

//! Futility pruning: near the horizon, a quiet move is skipped without
//! being searched where the static evaluation plus a margin still does not
//! reach alpha.
//!
//! **What these gates demonstrate is that the margin is what decides.**
//! That is a sharper claim than "the pruning happens", and it is the one
//! worth making, because a rule that skipped quiet moves on any pretext
//! would satisfy a counter and fail this file: the first gate below runs
//! one position twice at one depth with alpha moved by a single centipawn
//! across the threshold the margin sets, and asserts that moves are skipped
//! on one side of it and none on the other. Nothing about the position, the
//! depth or the move list differs between the two runs.
//!
//! **And that an exempt move is searched, demonstrated by the answer and
//! not only by a counter.** A quiet move that gives check is the one
//! exemption that costs something to compute, so it is the one a later
//! session would be tempted to drop. The mate gate below stands in a
//! position hopeless enough that every quiet move is futile, where the only
//! thing that saves the side to move is a quiet check that mates: with the
//! exemption the search returns the mate, and without it the move is
//! skipped and the mate is not there to find.
//!
//! The counters these gates read are written wherever the rule runs and
//! read on no decision path, so a depth-limited search here reads no clock
//! and the assertions are exact, not statistical.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal};
use cadence_engine::eval;
use cadence_engine::score::{MATE_IN_MAX_PLY, Score, mate_in, mated_in};
use cadence_engine::search::{Limits, Search, futile_node, futility_margin, futility_skips};
use support::table;

/// The depth the set gate below searches to.
///
/// **Eight, and the reason is the mechanism rather than the arithmetic.**
/// The rule needs a node whose evaluation sits a margin below alpha, and a
/// balanced position does not produce one until the search is deep enough
/// for a line to have gone wrong somewhere. Measured on the tree this
/// landed on, over the set below: at depths four and five the start
/// position admits no node at all, at six it admits one and skips nothing
/// at it, and seven is the first depth every position in the set both
/// admits and skips. Eight is taken, one ply past that, because a gate
/// standing exactly on the first depth that works is a gate the next tree
/// change silently empties. That is how the counterfactual ceilings in
/// `tests/ordering.rs` have failed before, arriving on a coverage
/// assertion instead of on a ceiling, and it fails the same way: quietly,
/// and in the direction that looks like everything is fine.
const GATE_DEPTH: u32 = 8;

/// A quiet middlegame, the same position the reduction gates use. Nothing
/// is en prise, so the static evaluation is a reading the rule can be asked
/// about rather than a snapshot of a position mid-exchange.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// White is three queens down and has one thing on the board: a rook that
/// reaches a8 along an empty file, where it mates. The mating move is quiet
/// and it gives check, which are the two properties the check exemption
/// exists for. Every other quiet move at this node is futile by any margin,
/// and this one is futile by the arithmetic too.
const QUIET_MATE: &str = "6k1/5ppp/8/8/8/8/1qqq4/R5K1 w - - 0 1";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// One node, searched at `depth` inside the null window `(alpha, alpha+1)`,
/// with a table of its own. The counters come back with the score.
fn one_node(fen: &str, depth: u32, alpha: Score) -> (Score, u64, u64, u64) {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(fen);
    let mut s = Search::new(Limits::depth(depth), &stop, &tt);
    let score = s.node_window(&mut b, depth, 0, alpha, alpha + 1);
    (
        score,
        s.futility_nodes(),
        s.futility_skipped(),
        s.futility_kept_check(),
    )
}

/// The margin is what skips the move, and one centipawn either side of it
/// is the whole difference between the two runs.
///
/// The node is searched at depth one, so the only node the rule can act at
/// is the one the gate hands it: every child is at depth zero, which is the
/// quiescence search, and the quiescence search has no margin rule. That
/// makes the counters below the root's own and not a subtree's.
///
/// `alpha` is placed exactly at `eval + futility_margin(1)`, where the
/// condition `eval + margin <= alpha` first holds, and then one centipawn
/// lower, where it does not. Nothing else moves. A rule keyed on anything
/// but the margin -- the depth, the move's index, the position being quiet
/// -- gives the same answer on both runs and fails here.
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

/// A node that skips moves still answers with a score something searched.
///
/// The first move of a node is exempt for exactly this reason, and it is
/// the exemption whose absence would not show up as a wrong score but as a
/// node returning the sentinel it started from. Asserted on the run above's
/// admitted side, where every quiet move behind the first was skipped.
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

/// The pruning happens in a real search: a middlegame and the start
/// position both admit nodes and skip quiet moves at them.
///
/// Coverage first: each search completed the depth it was asked for, so
/// the counters were read off finished trees. Then the property in two
/// halves that fail differently: admitted nodes prove the margin's own test
/// fires somewhere real, and skipped moves prove the loop acts on it. A
/// rule wired in but never admitted passes neither; one admitted at nodes
/// whose every move is exempt passes only the first.
#[test]
fn a_middlegame_search_skips_quiet_moves_near_the_horizon() {
    for fen in [
        START_FEN.to_string(),
        support::standard_fen("kiwipete"),
        MIDDLEGAME.to_string(),
    ] {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = board(&fen);
        let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
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

/// A quiet move that gives check is searched at a node where every other
/// quiet move is skipped, and the proof is the mate.
///
/// White is three queens down, so the evaluation sits thousands of
/// centipawns below any alpha this gate can pass, and every quiet move at
/// the root is futile by the arithmetic. `Ra8` is quiet, gives check, and
/// mates. Three assertions, and the first is the one that cannot be
/// satisfied by accident: the search returns a mate score, which it can only
/// do by searching a move the margin would otherwise have skipped. The
/// other two say the rule was live while it did so -- moves were skipped at
/// this node, and the check exemption is what kept one -- so the mate is
/// not being found because the rule failed to fire.
///
/// **Measured on a build with the check exemption taken out**, which is
/// what says this gate discriminates rather than passing on the shape of
/// the position: the same node returns -2,533 and skips 13 moves, and the
/// first assertion is the one that fires.
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

/// A node in check has no static evaluation, so the rule cannot claim
/// anything about it, and the exemption is that absence rather than a
/// condition anybody has to remember.
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

/// Past the depth limit the rule is off, whatever the gap: a node four
/// plies from the horizon searches every move it generates.
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

/// An alpha on the mate scale refuses the rule, at every depth inside the
/// band. A mate score is not a quantity a centipawn margin is
/// commensurable with, and a node whose alpha already names a mate may hold
/// a shorter one.
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

/// The margin is a pawn and a half per ply of remaining depth, pinned
/// against the values the constant's own comment names, and it grows: a
/// node with more search under it demands more before its quiet moves are
/// given up.
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

/// Every exemption the move itself carries refuses the skip, pinned
/// directly: the same quiet move at the same index is a candidate with no
/// exemption in force and is not one under each. Real moves from real
/// lists, so `is_noisy` is exercised against the generator and not a
/// hand-built encoding.
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
