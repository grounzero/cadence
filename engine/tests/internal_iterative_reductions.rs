// SPDX-License-Identifier: GPL-3.0-or-later

//! A non-PV node whose table probe named no move is searched shallower, and a PV node, a node in
//! check and a node with a table move are not. The counter is on no decision path, so the
//! assertions are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::{Move, generate_legal};
use cadence_engine::score::{INFINITE, Score};
use cadence_engine::search::{Limits, iir_reduction};
use cadence_engine::tt::Bound;
use support::{PAWN_ENDGAMES, table};

/// Restated rather than exported, so moving either constant fails here and is declared.
const THRESHOLD: u32 = 4;
const AMOUNT: u32 = 2;

/// No legal move gives check at either of the first two plies, so at the threshold only the root
/// can reach the rule.
const NO_CHECKS: &str = PAWN_ENDGAMES[0];

/// A quiet middlegame with one checking move, whose child is searched in check at the root's depth.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// Far above either position's value, so the null move, the margin and the capture probe all pass
/// the root by and the rule is what the root reads next.
const NULL_WINDOW: (Score, Score) = (2_000, 2_001);
const FULL_WINDOW: (Score, Score) = (-INFINITE, INFINITE);

/// The score, the nodes and the count of nodes the rule shortened, for one root searched alone. A
/// seed is stored at depth zero, so it names the move and answers nothing.
fn one_node(
    fen: &str,
    depth: u32,
    window: (Score, Score),
    seed: Option<Move>,
) -> (Score, u64, u64) {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(fen);
    if let Some(mv) = seed {
        tt.store(b.key(), mv, 0, 0, Bound::Upper);
    }
    let mut s = support::search(Limits::depth(depth), &stop, &tt);
    let score = s.node_window(&mut b, depth, 0, window.0, window.1);
    (score, s.nodes(), s.iir_nodes())
}

fn first_move(fen: &str) -> Move {
    generate_legal(&support::position(fen))
        .iter()
        .next()
        .expect("the position has a legal move")
}

/// The amount every search uses is read end to end below, against the shallower root.
#[test]
fn the_arithmetic_is_the_exemptions_and_the_threshold() {
    for depth in 0..32 {
        for pv in [false, true] {
            for in_check in [false, true] {
                for has_tt_move in [false, true] {
                    let exempt = pv || in_check || has_tt_move || depth < THRESHOLD;
                    assert_eq!(
                        iir_reduction(depth, pv, in_check, has_tt_move, AMOUNT),
                        if exempt { 0 } else { AMOUNT },
                        "depth {depth}, pv {pv}, in check {in_check}, table move {has_tt_move}"
                    );
                }
            }
        }
    }
}

/// The same score and the same nodes as the root asked for `AMOUNT` plies less, which is below the
/// threshold and so searched whole.
#[test]
fn a_non_pv_node_with_no_table_move_is_searched_as_a_shallower_node() {
    let (score, nodes, fired) = one_node(NO_CHECKS, THRESHOLD, NULL_WINDOW, None);
    let (shallow_score, shallow_nodes, shallow_fired) =
        one_node(NO_CHECKS, THRESHOLD - AMOUNT, NULL_WINDOW, None);
    assert_eq!(fired, 1, "the root kept its depth with no table move");
    assert_eq!(
        shallow_fired, 0,
        "a node below the threshold was searched shallower"
    );
    assert_eq!(
        (score, nodes),
        (shallow_score, shallow_nodes),
        "the shortened root did not search as the shallower one"
    );
}

#[test]
fn a_table_move_or_a_depth_below_the_threshold_keeps_the_depth() {
    let seed = Some(first_move(NO_CHECKS));
    let (_, _, with_move) = one_node(NO_CHECKS, THRESHOLD, NULL_WINDOW, seed);
    let (_, _, below) = one_node(NO_CHECKS, THRESHOLD - 1, NULL_WINDOW, None);
    assert_eq!(
        with_move, 0,
        "the root was searched shallower with a table move in hand"
    );
    assert_eq!(
        below, 0,
        "a node below the threshold was searched shallower"
    );
}

/// The root the null window shortens, asked with the full window.
#[test]
fn a_pv_node_with_no_table_move_keeps_its_depth() {
    let (_, _, fired) = one_node(NO_CHECKS, THRESHOLD, FULL_WINDOW, None);
    assert_eq!(fired, 0, "a PV node was searched shallower");
}

/// The extension hands the checking child its parent's depth, so only the in-check exemption keeps
/// it whole behind a root the table move exempts.
#[test]
fn a_checking_child_at_the_threshold_keeps_its_depth() {
    let seed = Some(first_move(MIDDLEGAME));
    let (_, _, fired) = one_node(MIDDLEGAME, THRESHOLD, NULL_WINDOW, seed);
    assert_eq!(fired, 0, "a node in check was searched shallower");
}

/// What the table gates set, so that their arm with no table is not a shallower search.
#[test]
fn a_reduction_set_to_zero_shortens_no_node() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(MIDDLEGAME);
    let mut s = support::search(Limits::depth(8), &stop, &tt);
    s.set_iir_reduction(0);
    let _ = s.node_window(&mut b, 8, 0, FULL_WINDOW.0, FULL_WINDOW.1);
    assert_eq!(
        s.iir_nodes(),
        0,
        "a node was shortened by a reduction of zero"
    );
}

/// A floor well under the measured count, against a rule that has stopped meeting a node that
/// qualifies.
#[test]
fn the_rule_is_not_inert_at_a_real_depth() {
    let (_, _, fired) = one_node(MIDDLEGAME, 8, FULL_WINDOW, None);
    assert!(
        fired > 100,
        "the rule fired at {fired} nodes of a depth-8 search"
    );
}
