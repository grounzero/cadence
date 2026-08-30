// SPDX-License-Identifier: GPL-3.0-or-later

//! Reverse futility: near the horizon, a node whose static evaluation
//! stands a margin above beta is returned without being searched at all.
//!
//! **What these gates demonstrate is an early return decided by the
//! margin, and a refused condition not returning.** Both halves are
//! sharper than "the pruning happens" and the first is sharper than any
//! counter: the load-bearing gate below searches one position twice with
//! beta moved by a single centipawn across the threshold the margin sets,
//! and asserts that on one side the whole search is **one node** and on
//! the other it is not. A rule keyed on the depth, on the position being
//! quiet, or on anything but the margin gives the same answer to both and
//! fails. Nothing about the position, the depth or the move list differs
//! between the two runs.
//!
//! **And the refusal is gated through its own counter rather than through
//! an absence.** A full-window node clears the margin here and is searched
//! anyway; the gate asserts both that it was searched and that the window
//! condition is what decided, so a tree in which no full-window node ever
//! cleared the margin cannot pass it.
//!
//! **One gate exists for a constant that is not there.** The rule carries
//! no depth limit: what stops it acting deep is the margin outrunning the
//! evaluation's own spread, and `the_margin_is_the_depth_limit` pins that
//! as a property rather than leaving it as an omission a later session
//! reads as an oversight and repairs.
//!
//! The counters these gates read are written wherever the rule runs and
//! read on no decision path, so a depth-limited search here reads no clock
//! and the assertions are exact, not statistical.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::START_FEN;
use cadence_core::position::Board;
use cadence_engine::eval;
use cadence_engine::score::{Score, mate_in, mated_in};
use cadence_engine::search::{Limits, Search, reverse_futile, reverse_futility_margin};
use support::{PAWN_ENDGAMES, table};

/// The depth the set gate below searches to.
///
/// Six, which is where the null-move gates stand, and for a related
/// reason: the rule needs an interior node inside a null window whose
/// evaluation clears beta by a margin, and a null window below the root is
/// something the search only produces once a node has a move in hand.
/// Deeper than the first depth that works, on the standing ground that a
/// coverage assertion resting on the first depth that works is one the
/// next tree change empties in silence.
const GATE_DEPTH: u32 = 6;

/// A quiet middlegame, the same position the reduction and futility gates
/// use. Nothing is en prise, so the static evaluation is a reading the rule
/// can be asked about rather than a snapshot of a position mid-exchange.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// One node, searched at `depth` inside the window given, with a table of
/// its own. The score, the nodes the search took, and the two counters.
fn one_node(fen: &str, depth: u32, alpha: Score, beta: Score) -> (Score, u64, u64, u64) {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(fen);
    let mut s = Search::new(Limits::depth(depth), &stop, &tt);
    let score = s.node_window(&mut b, depth, 0, alpha, beta);
    (
        score,
        s.nodes(),
        s.reverse_futility_cutoffs(),
        s.reverse_futility_refused_by_window(),
    )
}

/// The margin is what returns the node, and one centipawn either side of
/// it is the whole difference between the two runs.
///
/// The node is searched at depth one, so the only node the rule can act at
/// is the one the gate hands it: every child is at depth zero, which is the
/// quiescence search, and the quiescence search has no margin rule. That
/// makes the counters below the root's own and not a subtree's, and it
/// makes the node count a statement about this node alone.
///
/// `beta` is placed exactly at `eval - reverse_futility_margin(1)`, where
/// the condition `eval - margin >= beta` first holds, and then one
/// centipawn higher, where it does not. Nothing else moves.
///
/// **The assertion that carries the gate is the node count**, because this
/// rule's whole claim is that the node is answered without being searched:
/// one node on the admitted side, against a tree on the refused side. A
/// counter can say the rule fired; only the node count says nothing was
/// searched.
#[test]
fn the_margin_is_what_returns_the_node() {
    let eval = eval::evaluate(&board(MIDDLEGAME));
    let beta = eval - reverse_futility_margin(1);

    let (score, nodes, cutoffs, _) = one_node(MIDDLEGAME, 1, beta - 1, beta);
    assert_eq!(cutoffs, 1, "the margin did not return the node at beta");
    assert_eq!(
        nodes, 1,
        "the node was returned on the margin and {nodes} nodes were searched"
    );
    assert_eq!(
        score,
        eval - reverse_futility_margin(1),
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

/// A full-window node that clears the margin is searched anyway, and the
/// window condition is what decided.
///
/// The rule returns a bound and nothing else: no move, no line. That is the
/// answer a null-window question wants and it is not the answer the
/// principal variation wants, which is the same refusal the null move takes
/// and for the same reason. Here it is asserted through the refusal counter
/// as well as through the node count, so a tree in which no full-window
/// node ever cleared the margin cannot pass by presenting no case.
///
/// The window is the same beta the gate above fires on, with alpha a
/// hundred centipawns below it rather than one, so the only thing that
/// differs between the two gates is the width of the window.
#[test]
fn a_full_window_node_that_clears_the_margin_is_searched() {
    let eval = eval::evaluate(&board(MIDDLEGAME));
    let beta = eval - reverse_futility_margin(1);
    let (_, nodes, cutoffs, refused) = one_node(MIDDLEGAME, 1, beta - 100, beta);
    assert_eq!(cutoffs, 0, "a full-window node was returned on the margin");
    assert_eq!(
        refused, 1,
        "the window refusal decided nothing here, so the zero above covers nothing"
    );
    assert!(nodes > 1, "the full-window node was not searched");
}

/// A node in check has no static evaluation, so the rule cannot claim
/// anything about it, and the exemption is that absence rather than a
/// condition anybody has to remember.
#[test]
fn a_node_in_check_is_never_returned() {
    for depth in 0..8 {
        for beta in [-30_000, -100, 0, 100, 30_000] {
            assert!(
                reverse_futile(None, depth, beta).is_none(),
                "depth {depth} beta {beta}"
            );
        }
    }
}

/// A beta on the mate scale refuses the rule, at every depth and in both
/// directions. A mate score is not a quantity a centipawn margin is
/// commensurable with, and a claim resting on no search has proved nothing
/// about a forced mate.
#[test]
fn a_mate_beta_refuses_the_margin() {
    for depth in 1..8 {
        for ply in 0..8 {
            assert!(
                reverse_futile(Some(30_000), depth, mate_in(ply)).is_none(),
                "depth {depth}: mate in {ply}"
            );
            assert!(
                reverse_futile(Some(30_000), depth, mated_in(ply)).is_none(),
                "depth {depth}: mated in {ply}"
            );
        }
    }
}

/// The margin is a pawn and a half per ply of remaining depth, pinned
/// against the values the constant's own comment names, and it grows: a
/// node with more search under it has to clear beta by more before its
/// whole subtree is given up.
#[test]
fn the_margin_is_the_documented_margin() {
    assert_eq!(reverse_futility_margin(1), 150);
    assert_eq!(reverse_futility_margin(2), 300);
    assert_eq!(reverse_futility_margin(3), 450);
    assert_eq!(reverse_futility_margin(6), 900);
    for depth in 1..16 {
        assert!(
            reverse_futility_margin(depth + 1) > reverse_futility_margin(depth),
            "depth {depth}: the margin did not grow"
        );
    }
}

/// What comes back is the quantity the condition established, and it is at
/// or above beta whenever anything comes back at all.
///
/// Two properties in one sweep, and they fail differently. Returning the
/// evaluation instead of the evaluation less the margin passes the second
/// and fails the first, which is the mistake worth gating: it would claim
/// back the whole margin the rule discounted in order to fire.
#[test]
fn the_bound_is_the_quantity_the_condition_established() {
    let mut fired = 0;
    for depth in 1..8 {
        for eval in [-2_000, -150, 0, 150, 450, 1_000, 5_000] {
            for beta in [-1_000, -150, 0, 150, 900] {
                let Some(bound) = reverse_futile(Some(eval), depth, beta) else {
                    continue;
                };
                assert_eq!(
                    bound,
                    eval - reverse_futility_margin(depth),
                    "depth {depth}, eval {eval}, beta {beta}"
                );
                assert!(
                    bound >= beta,
                    "depth {depth}, eval {eval}: {bound} came back below beta {beta}"
                );
                fired += 1;
            }
        }
    }
    assert!(
        fired > 20,
        "the sweep fired {fired} times and covers little"
    );
}

/// The margin is the depth limit, and there is no other one.
///
/// This gate exists for a constant that is deliberately absent. The rule
/// carries no depth test: what stops it acting at a deep node is that the
/// margin it must clear grows with the depth while the evidence available
/// does not. Pinned as the property rather than left as an omission,
/// because an omission reads as an oversight and the repair a later session
/// would reach for is exactly the constant this declines.
///
/// Two halves. A fixed gap admits a band of shallow depths and nothing
/// past it, which a rule with no bound at all would fail. And the band
/// **moves with the evidence**, which is the half a depth constant cannot
/// do: ten times the gap buys ten times the band, where a limit would cut
/// both at the same ply.
#[test]
fn the_margin_is_the_depth_limit() {
    // 600 centipawns above beta: four plies of margin exactly, and the
    // fifth is one the evidence does not cover.
    for depth in 1..=4 {
        assert!(
            reverse_futile(Some(600), depth, 0).is_some(),
            "depth {depth}: 600 centipawns did not cover {} of margin",
            reverse_futility_margin(depth)
        );
    }
    for depth in 5..64 {
        assert!(
            reverse_futile(Some(600), depth, 0).is_none(),
            "depth {depth}: the margin did not outrun a gap of 600"
        );
    }
    // The band is a function of the evidence and not of a constant.
    assert!(reverse_futile(Some(150), 1, 0).is_some());
    assert!(reverse_futile(Some(150), 2, 0).is_none());
    assert!(reverse_futile(Some(6_000), 40, 0).is_some());
    assert!(reverse_futile(Some(6_000), 41, 0).is_none());
}

/// The pruning happens in a real search: a middlegame and the start
/// position both return nodes on the margin.
///
/// Coverage first: each search completed the depth it was asked for, so the
/// counters were read off finished trees. Then the property. A rule wired
/// in but never admitted fails it, which is the whole of what this gate is
/// for; the sharper claims are the two gates at the head of this file.
#[test]
fn a_middlegame_search_returns_nodes_on_the_margin() {
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
            s.reverse_futility_cutoffs() > 0,
            "{fen}: depth {GATE_DEPTH} searched {} nodes and the margin returned none",
            s.nodes()
        );
    }
}

/// A pawn endgame takes the margin where the null move refuses it, and the
/// difference between the two guards is deliberate.
///
/// The null move refuses a side with nothing but pawns beside its king,
/// because its mechanism is passing and passing is exactly what a side in
/// zugzwang wants and cannot have. This rule has no such guard and declines
/// one: it does not pass, it compares a reading against a bound, and its
/// exposure to a position the evaluation misreads is the one every member
/// of the margin family has rather than the one the null move's mechanism
/// creates.
///
/// That difference is asserted rather than described. On the same positions
/// at the same depth, `tests/pruning.rs` asserts the null move is never
/// tried; here the margin returns nodes. A later session that gives this
/// rule the material guard for symmetry breaks this gate, which is what it
/// is for.
#[test]
fn a_pawn_endgame_takes_the_margin_where_the_null_move_refuses_it() {
    for fen in PAWN_ENDGAMES {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = board(fen);
        let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
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
