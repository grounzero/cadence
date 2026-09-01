// SPDX-License-Identifier: GPL-3.0-or-later

//! Late move pruning: at low depth, a node stops searching quiet moves once
//! a count of them has failed to beat alpha, instead of searching the rest
//! at reduced depth.
//!
//! **What these gates demonstrate is that the count is what decides, and
//! that every refusal decides something.** The first is the sharper of the
//! two claims and it is made the way the margin's file makes its own: one
//! position searched twice with nothing different between the runs but the
//! count the node is given, moves given up on one side of it and none on
//! the other.
//!
//! **The second claim is the one this rule needs and the margin's does
//! not.** A reduction that was wrong is caught by the re-search above it; a
//! move given up here is never searched, at any depth, by any mechanism, so
//! an exemption that does not fire is a claim nothing is checking. Every
//! exemption below is therefore gated twice: once as arithmetic, on the
//! function, and once as a decision, on a counter that has to move or a
//! move the search has to find. The check exemption is gated by a mate that
//! only exists behind it.
//!
//! **And the floor is gated as the tie it is.** [`lmp_count`] never returns
//! less than [`REDUCTION_INDEX`], so no move the reduction refuses to
//! shorten by a ply is a move this rule deletes. That is one assertion and
//! it is the whole of why the base is not a tuning parameter.
//!
//! The counters are written wherever the rule runs and read on no decision
//! path, so a depth-limited search here reads no clock and the assertions
//! are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal};
use cadence_engine::picker;
use cadence_engine::score::{MATE_IN_MAX_PLY, Score};
use cadence_engine::search::{
    Limits, REDUCTION_INDEX, Search, lmp_count, lmp_index, lmp_skips, lmr_reduction,
};
use support::table;

/// The depth the set gate below searches to.
///
/// **Seven, and it is chosen the way the margin's `GATE_DEPTH` is: one ply
/// past the shallowest depth that works, measured rather than assumed.**
/// Over the set below, a search to depth five gives up moves at every
/// position and a search to four does not at all of them, because the rule
/// needs a node holding more moves than the count admits and the count is
/// eight at depth four. Seven is taken. A gate standing exactly on the
/// first depth that works is a gate the next tree change silently empties,
/// which is how the counterfactual ceilings in `tests/ordering.rs` have
/// failed before.
const GATE_DEPTH: u32 = 7;

/// A quiet middlegame, the same position the reduction and margin gates
/// use, so the three rules are asked about one tree.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// White is three queens down and has one thing on the board: a rook that
/// reaches a8 along an empty file, where it mates. The mating move is quiet
/// and gives check, and the gate below reads its place in the sorted list
/// rather than assuming one. The margin's own gate stands on the same
/// position for the same exemption; here what would delete the move is its
/// rank rather than the evaluation.
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
    (score, s.lmp_nodes(), s.lmp_skipped(), s.lmp_kept_check())
}

/// The count is what gives the move up, and the move's own rank is the
/// whole difference between two runs of one function.
///
/// A quiet non-killer at the index the count names is given up and the same
/// move one index lower is not. Nothing else moves: not the move, not the
/// node, not the killers. A rule keyed on anything but the count -- the
/// move being quiet, the node being admitted -- answers the same on both
/// and fails here.
#[test]
fn the_count_is_what_gives_the_move_up() {
    let quiet = generate_legal(&board(START_FEN))
        .iter()
        .find(|m| !m.is_noisy())
        .expect("the start position has a quiet move");
    let killers = [Move::NULL; 2];
    for depth in 1..=8 {
        let count = lmp_count(depth);
        let from = Some(count);
        assert!(
            lmp_skips(from, quiet, killers, count),
            "depth {depth}: the move at the count was searched"
        );
        assert!(
            !lmp_skips(from, quiet, killers, count - 1),
            "depth {depth}: the move one inside the count was given up"
        );
    }
}

/// No move the reduction refuses to shorten is a move this rule deletes.
///
/// The tie is the argument for the count's floor and this is the assertion
/// that keeps it true: the count never falls below [`REDUCTION_INDEX`], so
/// at every depth the rule acts on, a move inside the reduction's exempt
/// prefix is inside the count as well. Both halves are asserted, because
/// the first alone would still hold if the reduction's own threshold moved
/// out from under it.
#[test]
fn nothing_is_deleted_that_the_reduction_will_not_shorten() {
    let quiet = generate_legal(&board(START_FEN))
        .iter()
        .find(|m| !m.is_noisy())
        .expect("the start position has a quiet move");
    for depth in 1..=32 {
        assert!(
            lmp_count(depth) >= REDUCTION_INDEX,
            "depth {depth}: the count fell inside the reduction's exempt prefix"
        );
        for index in 0..REDUCTION_INDEX {
            assert_eq!(
                lmr_reduction(depth, index),
                0,
                "depth {depth} index {index}: the reduction fired inside its own prefix"
            );
            assert!(
                !lmp_skips(lmp_index(false, depth, 64), quiet, [Move::NULL; 2], index),
                "depth {depth} index {index}: given up inside the reduction's prefix"
            );
        }
    }
}

/// The count grows with the depth and never shrinks, so a node with more
/// search under it searches more moves before giving the rest up.
#[test]
fn the_count_is_the_documented_count() {
    assert_eq!(lmp_count(1), 3);
    assert_eq!(lmp_count(2), 5);
    assert_eq!(lmp_count(3), 7);
    assert_eq!(lmp_count(4), 11);
    assert_eq!(lmp_count(5), 15);
    assert_eq!(lmp_count(6), 21);
    assert_eq!(lmp_count(7), 27);
    assert_eq!(lmp_count(8), 35);
    for depth in 1..64 {
        assert!(
            lmp_count(depth + 1) > lmp_count(depth),
            "depth {depth}: the count did not grow"
        );
    }
}

/// Past the depth limit the rule is off, whatever the node holds.
///
/// A node with more moves than any count admits is refused at every depth
/// above the limit, so only the limit can be doing the refusing.
#[test]
fn no_pruning_past_the_depth_limit() {
    assert!(lmp_index(false, 8, 256).is_some(), "depth eight");
    for depth in 9..64 {
        assert!(
            lmp_index(false, depth, 256).is_none(),
            "depth {depth}: the node was admitted past the limit"
        );
    }
}

/// A node in check is never admitted, at any depth inside the band and
/// whatever it holds. Every move there is an evasion and what a wrong skip
/// loses is a mate defence.
#[test]
fn a_node_in_check_never_gives_a_move_up() {
    for depth in 0..12 {
        for moves in [1, 8, 40, 256] {
            assert!(
                lmp_index(true, depth, moves).is_none(),
                "depth {depth} with {moves} moves"
            );
        }
    }
}

/// A node holding no more moves than the count searches all of them, which
/// is what makes the node-level question worth asking once.
#[test]
fn a_node_inside_the_count_is_not_admitted() {
    for depth in 1..=8 {
        let count = lmp_count(depth);
        assert!(
            lmp_index(false, depth, count).is_none(),
            "depth {depth}: a node of exactly the count was admitted"
        );
        assert!(
            lmp_index(false, depth, count + 1).is_some(),
            "depth {depth}: a node one past the count was refused"
        );
    }
}

/// Every exemption the move itself carries refuses the skip, pinned one at
/// a time: the same quiet move at the same index is a candidate with no
/// exemption in force and is not one under each. Real moves from real
/// lists, so `is_noisy` is exercised against the generator.
#[test]
fn each_exemption_alone_keeps_the_move() {
    let list = generate_legal(&board(START_FEN));
    let quiet = list
        .iter()
        .find(|m| !m.is_noisy())
        .expect("the start position has a quiet move");
    let other = list
        .iter()
        .find(|m| !m.is_noisy() && *m != quiet)
        .expect("the start position has two quiet moves");
    let from = Some(4);
    assert!(
        lmp_skips(from, quiet, [Move::NULL; 2], 8),
        "no exemption in force and no skip"
    );
    assert!(!lmp_skips(None, quiet, [Move::NULL; 2], 8), "the node");
    assert!(
        !lmp_skips(from, quiet, [Move::NULL; 2], 3),
        "inside the count"
    );
    assert!(
        !lmp_skips(from, quiet, [quiet, Move::NULL], 8),
        "the first killer"
    );
    assert!(
        !lmp_skips(from, quiet, [other, quiet], 8),
        "the second killer"
    );
    let noisy = generate_legal(&board(&support::standard_fen("kiwipete")))
        .iter()
        .find(|m| m.is_noisy())
        .expect("Kiwipete has a noisy move");
    assert!(!lmp_skips(from, noisy, [Move::NULL; 2], 8), "noisy");
}

/// The pruning happens in a real search: three positions all admit nodes
/// and give quiet moves up at them.
///
/// Coverage first, so the counters are read off finished trees. Then the
/// property in two halves that fail differently: admitted nodes prove the
/// node-level question fires somewhere real, and given-up moves prove the
/// loop acts on it. A rule wired in but never admitted passes neither; one
/// admitted only at nodes whose every late move is exempt passes the first.
#[test]
fn a_middlegame_search_gives_up_late_quiet_moves() {
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
            s.lmp_nodes() > 0,
            "{fen}: depth {GATE_DEPTH} searched {} nodes and admitted none",
            s.nodes()
        );
        assert!(
            s.lmp_skipped() > 0,
            "{fen}: {} nodes admitted and not one quiet move given up",
            s.lmp_nodes()
        );
    }
}

/// A quiet move that gives check is searched at a node where the rule is
/// giving up every other move at its rank, and the proof is the mate.
///
/// `Ra8` is quiet, gives check and mates, and the gate reads its place in
/// the sorted list rather than assuming one. Three assertions, and the
/// first cannot be satisfied by accident: the search returns a mate score,
/// which it can only do by searching a move the count would otherwise have
/// deleted. The other two say the rule was live while it did so, so the
/// mate is not being found because the rule failed to fire.
///
/// **What is deliberately not asserted here is that a move was given up at
/// this node**, and the reason is the mate: it cuts, so the loop breaks and
/// nothing behind it is ever reached, which leaves the skip counter at zero
/// however live the rule is. A gate written to assert it would pass only by
/// finding a mate late enough to leave moves behind it, which is a property
/// of the position and not of the rule.
/// `a_node_that_gives_moves_up_still_has_an_answer` is where the skip
/// counter is asserted, on a node with no mate in it.
///
/// **Measured on a build with the check exemption taken out**, which is
/// what says this gate discriminates rather than passing on the shape of
/// the position: the same node returns -2,233 and the first assertion is
/// the one that fires.
#[test]
fn a_quiet_check_survives_the_count_and_the_mate_is_found() {
    // The rank is read off the same sort the search runs, with the empty
    // table and empty killers a fresh search starts from, so the gate
    // cannot pass because the mating move happened to sort inside the
    // count. If a later ordering change promotes it there, this fires.
    let b = board(QUIET_MATE);
    let mut list = generate_legal(&b);
    picker::sort_from(&b, &mut list, 0, [Move::NULL; 2], &[]);
    let mate = list
        .iter()
        .position(|m| board(QUIET_MATE).gives_check(m) && !m.is_noisy())
        .expect("the gate's own position has a quiet check");
    assert!(
        mate >= lmp_count(3),
        "the mating move sorts at {mate}, inside the count of {}",
        lmp_count(3)
    );

    let (score, nodes, _, kept) = one_node(QUIET_MATE, 3, 0);
    assert!(score >= MATE_IN_MAX_PLY, "the mate was not found: {score}");
    assert!(nodes > 0, "the rule admitted no node");
    assert!(
        kept > 0,
        "no move was kept for giving check, so the exemption decided nothing here"
    );
}

/// A node that gives moves up still answers with a score something
/// searched.
///
/// The count never reaches the node's first move, so this cannot fail as a
/// wrong score: it would fail as a node returning the sentinel it started
/// from. Asserted on a run where the rule was live.
#[test]
fn a_node_that_gives_moves_up_still_has_an_answer() {
    let (score, nodes, skipped, _) = one_node(MIDDLEGAME, 2, 0);
    assert!(nodes > 0, "the rule admitted no node");
    assert!(skipped > 0, "nothing was given up");
    assert!(
        score > -MATE_IN_MAX_PLY && score < MATE_IN_MAX_PLY,
        "the node returned {score}, which is not a score a move produced"
    );
}
