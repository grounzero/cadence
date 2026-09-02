// SPDX-License-Identifier: GPL-3.0-or-later

//! Main-search SEE pruning: at low depth, a node refuses a capture whose exchange loses more
//! material than a depth-scaled bound, instead of searching it in full.
//!
//! **What these gates demonstrate is that the exchange is what refuses the move, and that a
//! refusal is never something another rule was already taking.** The first is the claim the
//! rule is, and it is made the way the count's file makes its own: one node searched once,
//! with the moves the predicate names computed independently from the same public functions
//! and compared against what the search actually refused.
//!
//! **The second is this rule's own hazard and no other rule in the loop has it.** The margin
//! and the count both refuse every noisy move and this one takes nothing else, so the three
//! populations are disjoint and the order among them in the loop cannot move a single
//! count. That is asserted over real move lists rather than argued from the predicates,
//! because it is the property that lets the three counters be read side by side.
//!
//! **A refused capture is never searched, at any depth, by any mechanism**, so an exemption
//! that does not fire is a claim nothing is checking. The check exemption is therefore gated
//! on a counter that has to move rather than on the absence of a counterexample.
//!
//! The counters are written wherever the rule runs and read on no decision path, so a
//! depth-limited search here reads no clock and the assertions are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal};
use cadence_engine::picker;
use cadence_engine::search::{
    Limits, Search, futility_skips, lmp_index, lmp_skips, see_capture_bound, see_prune_node,
    see_skips,
};
use cadence_engine::see;
use support::table;

/// The depth the end-to-end gates search to.
///
/// **Seven, one ply past the shallowest depth that reaches the rule at every position in the
/// set.** A gate standing exactly on the first depth that works is a gate the next tree
/// change silently empties, which is how the counterfactual ceilings in `tests/ordering.rs`
/// have failed before.
const GATE_DEPTH: u32 = 7;

/// A quiet middlegame, the same position the margin, the count and the reduction gates use,
/// so the four rules are asked about one tree.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// White has one winning capture and one losing one. `Qxb4` takes an undefended knight and
/// sorts first; `Qxd5` takes a pawn the c6 pawn defends, so the exchange loses eight hundred
/// centipawns and the move sorts behind it.
///
/// **Neither capture gives check**, which the check exemption's own gate relies on: this
/// position isolates the exchange as the only thing that can refuse a move here.
const ONE_LOSING_CAPTURE: &str = "4k3/8/2p5/3p4/1n6/8/3Q4/4K3 w - - 0 1";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The node's list in the order the search will try it, with no table move, no killer and no
/// history, which is what a first visit to a node with an empty table holds.
fn ordered(b: &Board) -> Vec<Move> {
    let mut legal = generate_legal(b);
    picker::sort_from(b, &mut legal, 0, [Move::NULL; 2], &[]);
    legal.iter().collect()
}

/// The bound is the documented bound, and it is one number times the depth.
#[test]
fn the_bound_is_the_documented_bound() {
    assert_eq!(see_capture_bound(1), -100);
    assert_eq!(see_capture_bound(2), -200);
    assert_eq!(see_capture_bound(3), -300);
    assert_eq!(see_capture_bound(4), -400);
    assert_eq!(see_capture_bound(5), -500);
}

/// The bound loosens with depth and never tightens, so a node with more search under a
/// losing capture refuses fewer of them rather than more.
///
/// The direction is the whole of the rule's shape and it is the one thing a retune must not
/// invert: a bound that tightened with depth would delete moves hardest where the search is
/// most able to justify them.
#[test]
fn the_bound_loosens_with_depth_and_never_tightens() {
    for depth in 1..64 {
        assert!(
            see_capture_bound(depth + 1) < see_capture_bound(depth),
            "depth {depth}: the bound did not loosen"
        );
    }
    assert!(
        see_capture_bound(u32::MAX) < 0,
        "the bound saturated to a non-negative value"
    );
}

/// Past the depth limit the rule is off, whatever the node holds.
#[test]
fn no_refusal_past_the_depth_limit() {
    for depth in 0..=5 {
        assert!(see_prune_node(false, depth), "depth {depth} was refused");
    }
    for depth in 6..64 {
        assert!(
            !see_prune_node(false, depth),
            "depth {depth}: the node was admitted past the limit"
        );
    }
}

/// A node in check is never admitted, at any depth inside the band.
///
/// Every move there is an evasion and what a wrong refusal loses is a mate defence, which is
/// the same reason the count refuses a node in check.
#[test]
fn a_node_in_check_never_refuses_a_capture() {
    for depth in 0..12 {
        assert!(!see_prune_node(true, depth), "depth {depth} in check");
    }
}

/// Every exemption the move itself carries refuses the skip, pinned one at a time.
///
/// Real moves from real lists, so `is_capture` is exercised against the generator rather
/// than against a move this file built.
#[test]
fn each_exemption_alone_keeps_the_move() {
    let kiwipete = board(&support::standard_fen("kiwipete"));
    let list = generate_legal(&kiwipete);
    let capture = list
        .iter()
        .find(|m| m.is_capture())
        .expect("Kiwipete has a capture");
    let quiet = list
        .iter()
        .find(|m| !m.is_noisy())
        .expect("Kiwipete has a quiet move");
    assert!(
        see_skips(true, capture, 3),
        "no exemption in force and no skip"
    );
    assert!(!see_skips(false, capture, 3), "the node");
    assert!(!see_skips(true, capture, 0), "the node's first move");
    assert!(!see_skips(true, quiet, 3), "a quiet move");
}

/// A promotion that captures nothing is not a candidate, and one that captures is.
///
/// This is the one place the rule's population parts company with `picker`'s noisy band, and
/// it is gated on real promotions rather than on the predicate's wording.
#[test]
fn a_quiet_promotion_is_not_a_candidate_and_a_capturing_one_is() {
    let b = board("2r1k3/1P6/8/8/8/8/8/4K3 w - - 0 1");
    let list = generate_legal(&b);
    let quiet_promotion = list
        .iter()
        .find(|m| m.is_promotion() && !m.is_capture())
        .expect("b7b8 promotes and captures nothing");
    assert!(
        !see_skips(true, quiet_promotion, 3),
        "a promotion that captures nothing was a candidate"
    );
    let promotion_capture = list
        .iter()
        .find(|m| m.is_promotion() && m.is_capture())
        .expect("b7c8 promotes and captures");
    assert!(
        see_skips(true, promotion_capture, 3),
        "a promotion that captures was not a candidate"
    );
}

/// This rule's population is disjoint from the margin's and from the count's, so where it
/// sits among the three in the move loop cannot move a single counter.
///
/// **The margin and the count overlap each other by design and this rule overlaps neither**,
/// which is the asymmetry the counters' own note rests on: those two refuse every noisy move
/// and this one takes nothing but captures. Asserted over whole move lists rather than
/// argued from the predicates, and pairwise rather than over the three at once, because the
/// claim is about this rule and not about the other two.
#[test]
fn this_rule_is_disjoint_from_the_margin_and_from_the_count() {
    for fen in [
        START_FEN.to_string(),
        support::standard_fen("kiwipete"),
        MIDDLEGAME.to_string(),
        ONE_LOSING_CAPTURE.to_string(),
    ] {
        let b = board(&fen);
        let list = ordered(&b);
        let killers = [Move::NULL; 2];
        let give_up = lmp_index(false, 1, list.len());
        for (index, m) in list.iter().enumerate() {
            let mine = see_skips(true, *m, index);
            let margin = futility_skips(true, *m, index);
            let count = lmp_skips(give_up, *m, killers, index);
            assert!(
                !(mine && margin),
                "{fen}: move {index} is a candidate for this rule and the margin"
            );
            assert!(
                !(mine && count),
                "{fen}: move {index} is a candidate for this rule and the count"
            );
        }
    }
}

/// The exchange is what refuses the move, and the rule refuses exactly the moves the
/// predicate names.
///
/// One node at depth one, so every child is a quiescence node and the counters can only have
/// been written by this node. The expectation is computed from the same public functions the
/// search uses and not transcribed, so a change to the bound or to the order moves both
/// sides together and a change to the rule moves only one.
#[test]
fn the_rule_refuses_exactly_the_captures_the_bound_refuses() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(ONE_LOSING_CAPTURE);
    let expected: Vec<Move> = ordered(&b)
        .into_iter()
        .enumerate()
        .filter(|(index, m)| {
            see_skips(true, *m, *index)
                && !b.gives_check(*m)
                && see::see(&b, *m) < see_capture_bound(1)
        })
        .map(|(_, m)| m)
        .collect();
    assert!(
        !expected.is_empty(),
        "the position refuses nothing, so this gate proves nothing"
    );
    let mut s = Search::new(Limits::depth(1), &stop, &tt);
    let _ = s.node(&mut b, 1, 0);
    assert_eq!(s.see_nodes(), 1, "more than the one node was admitted");
    assert_eq!(
        s.see_skipped(),
        expected.len() as u64,
        "the search refused a different set from the one the predicate names"
    );
}

/// The pruning happens in a real search: three positions all admit nodes and refuse captures
/// at them.
///
/// Coverage first, so the counters are read off finished trees, then the property in two
/// halves that fail differently. Admitted nodes prove the node-level question fires
/// somewhere real and refused captures prove the loop acts on it, so a rule wired in but
/// never admitted passes neither and one admitted only where every capture is exempt passes
/// the first.
#[test]
fn a_middlegame_search_refuses_losing_captures() {
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
            s.see_nodes() > 0,
            "{fen}: depth {GATE_DEPTH} searched {} nodes and admitted none",
            s.nodes()
        );
        assert!(
            s.see_skipped() > 0,
            "{fen}: {} nodes admitted and not one capture refused",
            s.see_nodes()
        );
    }
}

/// The check exemption decides rather than merely failing to be contradicted.
///
/// A gate asserting that an exempt move was searched needs to see the exemption fire, and a
/// tree in which no losing capture ever gave check would pass a gate written the other way.
#[test]
fn the_check_exemption_decides() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(&support::standard_fen("kiwipete"));
    let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
    let _ = s.run(&mut b, &mut Vec::new());
    assert!(
        s.see_kept_check() > 0,
        "{} captures refused and the check exemption never fired",
        s.see_skipped()
    );
}
