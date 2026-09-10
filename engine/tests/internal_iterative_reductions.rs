// SPDX-License-Identifier: GPL-3.0-or-later

//! A node whose table probe named no move is searched a ply shallower than
//! it was asked for.
//!
//! **What these gates demonstrate is that the absent table move is what
//! decides, and that the rule is not inert.** The first is made the way the
//! count's file makes its own: one position searched twice with nothing
//! different between the runs but whether the table names a move at the
//! node. Seeding the table is what makes that a difference of one thing,
//! since every other route to a table move -- searching the position first,
//! deepening into it -- also changes the tree.
//!
//! **The second claim is the one this rule needs and a reduction does
//! not.** A late move's reduction is undone by the re-search above it when
//! it was wrong; this rule shortens the node itself, stores the result at
//! the shortened depth, and nothing re-searches it. So a rule that had
//! quietly stopped firing would look exactly like a rule that was working,
//! and the counter is what separates them.
//!
//! **The exact gates stand on a position with no checking move, and that is
//! a condition rather than a convenience.** At the threshold depth a child
//! is searched a ply below the rule and cannot reach it -- unless the move
//! gave check, where the extension hands the ply straight back and the child
//! fires at the depth its parent did. On a quiet middlegame the exempted run
//! therefore still reads one, from a checking child rather than from the
//! root, and a gate written there would assert the opposite of what it says.
//! The last gate but one is that case, kept so the condition is visible.
//!
//! The counter is written where the rule runs and read on no decision path,
//! so a depth-limited search here reads no clock and the assertions are
//! exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::{Move, generate_legal};
use cadence_engine::score::{INFINITE, Score};
use cadence_engine::search::{Limits, iir_reduction};
use cadence_engine::tt::Bound;
use support::{PAWN_ENDGAMES, table};

/// The shallowest depth the rule fires at, restated here rather than
/// exported. A gate that reads the constant it is checking cannot see the
/// constant move, which is the whole of what this file is for.
const THRESHOLD: u32 = 4;

/// How many plies the node loses, restated for [`THRESHOLD`]'s reason.
const AMOUNT: u32 = 1;

/// Kings behind their own pawns: no legal move gives check at either of the
/// first two plies, which is what the exact gates below need. It is
/// `PAWN_ENDGAMES[0]`, already read by the null move's gates, so two rules
/// are asked about one tree.
const NO_CHECKS: &str = PAWN_ENDGAMES[0];

/// A quiet middlegame, the same position the count and margin gates use.
/// **It has one checking move**, so it is read here only where the
/// assertion is a floor or where the checking child is the subject.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// One node searched at `depth` with the full window and a table of its
/// own, seeded with `seed` at the root key when one is given. The count of
/// nodes the rule fired at comes back with the score.
fn one_node(fen: &str, depth: u32, seed: Option<Move>) -> (Score, u64) {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(fen);
    if let Some(mv) = seed {
        // Deep enough that no probe refuses it for depth, and an upper bound
        // so that it names a move without also granting a cutoff.
        tt.store(b.key(), mv, 0, u8::MAX, Bound::Upper);
    }
    let mut s = support::search(Limits::depth(depth), &stop, &tt);
    let score = s.node_window(&mut b, depth, 0, -INFINITE, INFINITE);
    (score, s.iir_nodes())
}

/// The first legal move of `fen`, which is a move the table can name
/// without any claim being made that it is a good one.
fn first_move(fen: &str) -> Move {
    generate_legal(&support::position(fen))
        .iter()
        .next()
        .expect("the position has a legal move")
}

/// The arithmetic: a table move exempts the node at every depth, the
/// threshold is where the rule starts, and the amount is one ply.
///
/// Read against restated constants, so a change to either constant fails
/// here and is declared rather than absorbed.
#[test]
fn the_arithmetic_is_the_exemption_the_threshold_and_the_amount() {
    for depth in 0..32 {
        assert_eq!(
            iir_reduction(depth, true),
            0,
            "depth {depth}: a node the table named a move for lost a ply"
        );
        let want = if depth < THRESHOLD { 0 } else { AMOUNT };
        assert_eq!(
            iir_reduction(depth, false),
            want,
            "depth {depth}: wrong reduction with no table move"
        );
    }
}

/// The absent table move is what decides, and it is the only thing that
/// differs between the two runs.
///
/// At the threshold depth on a position with no checking move the root is
/// the only node that can reach the rule, so the counter is one when the
/// probe named nothing and zero when it named a move. A rule keyed on
/// anything else -- the depth alone, the node being interior -- answers the
/// same on both and fails here.
#[test]
fn the_absent_table_move_is_what_decides() {
    let (_, fired) = one_node(NO_CHECKS, THRESHOLD, None);
    let (_, exempt) = one_node(NO_CHECKS, THRESHOLD, Some(first_move(NO_CHECKS)));
    assert_eq!(fired, 1, "the root did not lose a ply with an empty table");
    assert_eq!(exempt, 0, "the root lost a ply with a table move in hand");
}

/// The threshold decides, read at the two depths either side of it.
///
/// One ply below it no node in this tree can fire, so the counter is exactly
/// zero rather than merely small; at it the root fires and nothing else can.
/// A threshold that drifted down would move the first assertion and one that
/// drifted up would move the second.
#[test]
fn the_threshold_is_where_the_rule_starts() {
    let (_, below) = one_node(NO_CHECKS, THRESHOLD - 1, None);
    let (_, at) = one_node(NO_CHECKS, THRESHOLD, None);
    assert_eq!(below, 0, "a node below the threshold lost a ply");
    assert_eq!(at, 1, "the node at the threshold kept its ply");
}

/// The extension hands the ply back, which is the one way a child reaches
/// the rule at the depth its parent had.
///
/// This is the condition the two gates above are stated on rather than a
/// property worth having: on the middlegame, whose single checking move is
/// the whole difference from the position they read, the exempted run still
/// reads one. A reader who took those gates for a claim about every position
/// is what this exists to stop.
#[test]
fn a_checking_child_reaches_the_rule_at_its_parents_depth() {
    let (_, exempt) = one_node(MIDDLEGAME, THRESHOLD, Some(first_move(MIDDLEGAME)));
    assert_eq!(exempt, 1, "no child fired behind an exempted root");
}

/// The rule reaches a real tree rather than only its own gates.
///
/// A search deep enough to hold interior nodes the table has never seen
/// fires at many of them, and the assertion is a floor well under the
/// measured count rather than the count itself. What it guards against is a
/// rule that still compiles, still passes the gates above, and has stopped
/// meeting a node that qualifies.
#[test]
fn the_rule_is_not_inert_at_a_real_depth() {
    let (_, fired) = one_node(MIDDLEGAME, 9, None);
    assert!(
        fired > 100,
        "the rule fired at {fired} nodes of a depth-nine search"
    );
}
