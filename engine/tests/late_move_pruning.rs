// SPDX-License-Identifier: GPL-3.0-or-later

//! The count decides, shown by one position searched twice with only the count changed. A given-up
//! move is never searched again, so every exemption is gated twice, as arithmetic and as a
//! decision; `lmp_count` never falls below `REDUCTION_INDEX`, so no move the reduction spares is
//! deleted.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal};
use cadence_engine::picker;
use cadence_engine::score::{MATE_IN_MAX_PLY, Score};
use cadence_engine::search::{
    Limits, REDUCTION_INDEX, lmp_count, lmp_index, lmp_skips, lmr_reduction,
};
use cadence_engine::tune::Tunables;
use support::table;

/// The compiled-in values every gate here pins.
const DEFAULT: &Tunables = &Tunables::DEFAULT;

/// Depth five is the shallowest at which every position gives moves up; seven leaves margin, so a
/// tree change cannot silently empty the gate.
const GATE_DEPTH: u32 = 7;

/// The same position the reduction and margin gates use, so the three rules are asked about one
/// tree.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// The gate reads the mating move's place in the sorted list rather than assuming one: here its
/// rank, not the evaluation, is what would delete it.
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
    (score, s.lmp_nodes(), s.lmp_skipped(), s.lmp_kept_check())
}

/// A quiet non-killer at the index the count names is given up, and the same move one index lower
/// is not. A rule keyed on anything but the count answers the same on both.
#[test]
fn the_count_is_what_gives_the_move_up() {
    let quiet = generate_legal(&board(START_FEN))
        .iter()
        .find(|m| !m.is_noisy())
        .expect("the start position has a quiet move");
    let killers = [Move::NULL; 2];
    for depth in 1..=8 {
        let count = lmp_count(DEFAULT, depth);
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

/// Both halves are asserted, since the first alone would hold if the reduction's threshold moved.
#[test]
fn nothing_is_deleted_that_the_reduction_will_not_shorten() {
    let quiet = generate_legal(&board(START_FEN))
        .iter()
        .find(|m| !m.is_noisy())
        .expect("the start position has a quiet move");
    for depth in 1..=32 {
        assert!(
            lmp_count(DEFAULT, depth) >= REDUCTION_INDEX,
            "depth {depth}: the count fell inside the reduction's exempt prefix"
        );
        for index in 0..REDUCTION_INDEX {
            assert_eq!(
                lmr_reduction(depth, index),
                0,
                "depth {depth} index {index}: the reduction fired inside its own prefix"
            );
            assert!(
                !lmp_skips(
                    lmp_index(DEFAULT, false, depth, 64),
                    quiet,
                    [Move::NULL; 2],
                    index
                ),
                "depth {depth} index {index}: given up inside the reduction's prefix"
            );
        }
    }
}

/// A node with more search under it searches more moves before giving the rest up.
#[test]
fn the_count_is_the_documented_count() {
    assert_eq!(lmp_count(DEFAULT, 1), 3);
    assert_eq!(lmp_count(DEFAULT, 2), 5);
    assert_eq!(lmp_count(DEFAULT, 3), 7);
    assert_eq!(lmp_count(DEFAULT, 4), 11);
    assert_eq!(lmp_count(DEFAULT, 5), 15);
    assert_eq!(lmp_count(DEFAULT, 6), 21);
    assert_eq!(lmp_count(DEFAULT, 7), 27);
    assert_eq!(lmp_count(DEFAULT, 8), 35);
    for depth in 1..64 {
        assert!(
            lmp_count(DEFAULT, depth + 1) > lmp_count(DEFAULT, depth),
            "depth {depth}: the count did not grow"
        );
    }
}

/// A node with more moves than any count admits is refused above the limit, so only the limit
/// refuses.
#[test]
fn no_pruning_past_the_depth_limit() {
    assert!(lmp_index(DEFAULT, false, 8, 256).is_some(), "depth eight");
    for depth in 9..64 {
        assert!(
            lmp_index(DEFAULT, false, depth, 256).is_none(),
            "depth {depth}: the node was admitted past the limit"
        );
    }
}

/// Every move in check is an evasion, and a wrong skip loses a mate defence.
#[test]
fn a_node_in_check_never_gives_a_move_up() {
    for depth in 0..12 {
        for moves in [1, 8, 40, 256] {
            assert!(
                lmp_index(DEFAULT, true, depth, moves).is_none(),
                "depth {depth} with {moves} moves"
            );
        }
    }
}

/// Which makes the node-level question worth asking once.
#[test]
fn a_node_inside_the_count_is_not_admitted() {
    for depth in 1..=8 {
        let count = lmp_count(DEFAULT, depth);
        assert!(
            lmp_index(DEFAULT, false, depth, count).is_none(),
            "depth {depth}: a node of exactly the count was admitted"
        );
        assert!(
            lmp_index(DEFAULT, false, depth, count + 1).is_some(),
            "depth {depth}: a node one past the count was refused"
        );
    }
}

/// The same quiet move at the same index is a candidate with no exemption in force and not one
/// under each. Real moves, so `is_noisy` is exercised against the generator.
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

/// Admitted nodes prove the node-level question fires; given-up moves prove the loop acts. Never
/// admitted passes neither; admitted only where every late move is exempt passes the first.
#[test]
fn a_middlegame_search_gives_up_late_quiet_moves() {
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

/// The mate score can only come from searching a move the count would delete. No skip is asserted
/// here, since the mate cuts before any move behind it; without the check exemption the node
/// returns -2,233.
#[test]
fn a_quiet_check_survives_the_count_and_the_mate_is_found() {
    // Read off the search's own sort from a fresh state, so the gate cannot pass because the mating
    // move sorted inside the count.
    let b = board(QUIET_MATE);
    let mut list = generate_legal(&b);
    picker::sort_from(&b, &mut list, 0, [Move::NULL; 2], &[]);
    let mate = list
        .iter()
        .position(|m| board(QUIET_MATE).gives_check(m) && !m.is_noisy())
        .expect("the gate's own position has a quiet check");
    assert!(
        mate >= lmp_count(DEFAULT, 3),
        "the mating move sorts at {mate}, inside the count of {}",
        lmp_count(DEFAULT, 3)
    );

    let (score, nodes, _, kept) = one_node(QUIET_MATE, 3, 0);
    assert!(score >= MATE_IN_MAX_PLY, "the mate was not found: {score}");
    assert!(nodes > 0, "the rule admitted no node");
    assert!(
        kept > 0,
        "no move was kept for giving check, so the exemption decided nothing here"
    );
}

/// The count never reaches the first move, so a failure here is the sentinel returned, not a wrong
/// score.
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
