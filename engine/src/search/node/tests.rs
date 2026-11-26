// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{MAX_PLY, Move, generate_legal, parse_uci};

use super::lookup::mate_window;
use crate::conthist;
use crate::eval;
use crate::position::Position;
use crate::score::{INFINITE, MATE_IN_MAX_PLY, Score, mate_in, mated_in};
use crate::search::Search;
// The call record exists in a debug build alone, so the tests that read it are built there too.
#[cfg(debug_assertions)]
use crate::search::{Call, Expected, Via};
use crate::tt::{Bound, Table};

const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// Far above either position's value, so no preamble rule cuts the node and every child is
/// searched as the parent asked.
#[cfg(debug_assertions)]
const HIGH: Score = 2_000;

#[cfg(debug_assertions)]
fn searcher<'a>(stop: &'a AtomicBool, tt: &'a Table) -> Search<'a> {
    let mut search = Search::new(stop, tt);
    search.trace_calls();
    search
}

/// Every call recorded at one ply, in order: at the ply below a call site, its children alone.
#[cfg(debug_assertions)]
fn at(search: &Search<'_>, ply: usize) -> Vec<Call> {
    search
        .calls()
        .iter()
        .filter(|c| c.ply == ply)
        .copied()
        .collect()
}

/// Below the reduction's depth, so every child is a full-depth null window and takes the
/// opposite of a cut parent, and a cut under an all parent.
#[cfg(debug_assertions)]
#[test]
fn a_null_window_node_hands_its_children_the_other_kind() {
    let stop = AtomicBool::new(false);
    for (parent, child) in [
        (Expected::Cut, Expected::All),
        (Expected::All, Expected::Cut),
    ] {
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let mut board = Position::new(Board::from_fen(MIDDLEGAME).expect("the middlegame"));
        let _ = search.node_expected(&mut board, 2, 0, HIGH, HIGH + 1, parent);
        assert_eq!(
            at(&search, 0).iter().map(|c| c.kind).collect::<Vec<_>>(),
            [parent]
        );
        let children = at(&search, 1);
        assert!(children.len() > 1, "under {parent:?}: too few children");
        for (i, c) in children.iter().enumerate() {
            let via = if i == 0 { Via::First } else { Via::Null };
            assert_eq!(c.asked, Some((parent, via)), "under {parent:?}, child {i}");
            assert_eq!(c.kind, child, "under {parent:?}, child {i}");
        }
    }
}

/// One late move searched alone from the middlegame at depth four, and the calls one ply below.
#[cfg(debug_assertions)]
fn late_move_alone(base: u32, alpha: Score, parent: Expected) -> Vec<Call> {
    let stop = AtomicBool::new(false);
    let root = Board::from_fen(MIDDLEGAME).expect("the middlegame");
    let m = parse_uci(&generate_legal(&root), "h2h3").expect("a legal move");
    let tt = Table::new(1).expect("a one mebibyte table");
    let mut search = searcher(&stop, &tt);
    let mut board = Position::new(root);
    search.root_depth = 4;
    search.make(&mut board, 0, m);
    let pv = parent == Expected::Pv;
    let _ = search.late_move(&mut board, 4, base, 0, 0, alpha, pv, parent);
    at(&search, 1)
}

/// The reduced search's child is expected to cut whatever the parent, and the re-search a
/// reduced move earns runs at full depth and flips the parent. A high alpha is never beaten, so
/// nothing is searched again; a low one always is.
#[cfg(debug_assertions)]
#[test]
fn a_reduced_late_moves_child_is_a_cut_node_and_its_re_search_flips_the_parent() {
    use Expected::{All, Cut, Pv};
    // Two plies off four, one more at a cut parent and one taken back at a PV parent.
    let cases = [
        (HIGH, Cut, vec![(Via::Reduced, Cut, 1)]),
        (HIGH, All, vec![(Via::Reduced, Cut, 2)]),
        (HIGH, Pv, vec![(Via::Reduced, Cut, 3)]),
        (
            -HIGH,
            Cut,
            vec![(Via::Reduced, Cut, 1), (Via::Null, All, 4)],
        ),
        (
            -HIGH,
            All,
            vec![(Via::Reduced, Cut, 2), (Via::Null, Cut, 4)],
        ),
    ];
    for (alpha, parent, expected) in cases {
        let read: Vec<(Via, Expected, u32)> = late_move_alone(2, alpha, parent)
            .iter()
            .map(|c| {
                let (from, via) = c.asked.expect("a derived kind");
                assert_eq!(from, parent, "alpha {alpha}, {parent:?}");
                (via, c.kind, c.depth)
            })
            .collect();
        assert_eq!(read, expected, "alpha {alpha}, {parent:?}");
    }
}

/// With no reduction the move is searched once, at full depth, flipping the parent.
#[cfg(debug_assertions)]
#[test]
fn an_unreduced_late_move_is_searched_once_at_full_depth() {
    use Expected::{All, Cut, Pv};
    for (alpha, parent, child) in [
        (HIGH, Cut, All),
        (HIGH, All, Cut),
        (HIGH, Pv, Cut),
        (-HIGH, Cut, All),
        (-HIGH, All, Cut),
    ] {
        let read: Vec<_> = late_move_alone(0, alpha, parent)
            .iter()
            .map(|c| (c.asked, c.kind, c.depth))
            .collect();
        assert_eq!(
            read,
            [(Some((parent, Via::Null)), child, 4)],
            "alpha {alpha}, {parent:?}"
        );
    }
}

#[cfg(debug_assertions)]
#[test]
fn the_null_moves_child_is_an_all_node() {
    let stop = AtomicBool::new(false);
    let tt = Table::new(1).expect("a one mebibyte table");
    let mut search = searcher(&stop, &tt);
    let root = Board::from_fen(MIDDLEGAME).expect("the middlegame");
    let m = parse_uci(&generate_legal(&root), "h2h3").expect("a legal move");
    let mut board = Position::new(root);
    search.root_depth = DEPTH;
    search.make(&mut board, 0, m);
    search.heuristics.evals[1] = Some(BETA + 200);
    let _ = search.null_move(&mut board, DEPTH, 1, BETA - 1, BETA);
    assert!(
        search.stats.null_attempts.get() > 0,
        "the null move was refused"
    );
    let read: Vec<_> = at(&search, 2).iter().map(|c| (c.asked, c.kind)).collect();
    assert_eq!(read, [(None, Expected::All)]);
}

/// A queen left to a pawn: the capture clears any raised beta, so the reduced search runs.
#[cfg(debug_assertions)]
#[test]
fn probcuts_child_flips_the_parent() {
    const HANGING: &str = "4k3/8/8/3q4/4P3/8/8/4K3 w - - 0 1";
    let stop = AtomicBool::new(false);
    for (parent, child) in [
        (Expected::Cut, Expected::All),
        (Expected::All, Expected::Cut),
    ] {
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let root = Board::from_fen(HANGING).expect("the hanging queen");
        let static_eval = eval::evaluate(&root);
        let mut board = Position::new(root);
        search.root_depth = 6;
        search.heuristics.evals[0] = Some(static_eval);
        let beta = static_eval;
        let cut = search.probcut(&mut board, 6, 0, beta - 1, beta, parent);
        assert!(cut.is_some(), "the capture did not cut");
        assert_eq!(search.stats.probcut_searches.get(), 1, "under {parent:?}");
        let read: Vec<_> = at(&search, 1).iter().map(|c| (c.asked, c.kind)).collect();
        assert_eq!(
            read,
            [(Some((parent, Via::Null)), child)],
            "under {parent:?}"
        );
    }
}

/// Only a fail low under a flagged parent adds the flag; a node's own flag is stored whatever
/// its bound, and a node with no parent reads none.
#[test]
fn a_node_failing_low_takes_its_parents_flag() {
    let stop = AtomicBool::new(false);
    let tt = Table::new(1).expect("a one mebibyte table");
    let mut search = Search::new(&stop, &tt);
    let cases = [
        (2, true, false, Bound::Upper, true),
        (2, true, false, Bound::Lower, false),
        (2, true, false, Bound::Exact, false),
        (2, false, false, Bound::Upper, false),
        (2, false, true, Bound::Lower, true),
        (0, true, false, Bound::Upper, false),
    ];
    for (i, &(ply, parent, own, bound, stored)) in cases.iter().enumerate() {
        let key = 0x9E37_79B9_7F4A_7C15_u64.wrapping_mul(i as u64 + 1);
        search.tt_pv = [parent; MAX_PLY];
        search.tt_pv[ply] = own;
        search.store(key, ply, 5, 0, Move::NULL, bound, None);
        let hit = tt.probe(key).expect("the entry was stored");
        assert_eq!(
            hit.pv, stored,
            "ply {ply}, parent {parent}, own {own}, {bound:?}"
        );
    }
}

/// Deep enough, and beta far enough above the child's evaluation, that the nodes below the null
/// move search quiet moves to a cutoff and so write the rows they read.
const DEPTH: u32 = 9;
const BETA: Score = 300;

/// A null move at ply 1 over the frame an earlier node left there, its static evaluation planted
/// above beta: the child reads no move one ply up and the root's move two plies up.
#[test]
fn a_null_move_hides_the_frame_below_it_and_not_the_one_above() {
    let stop = AtomicBool::new(false);
    let tt = Table::new(1).expect("a one mebibyte table");
    let mut search = Search::new(&stop, &tt);
    let root = Board::from_fen(MIDDLEGAME).expect("the middlegame");
    let m = parse_uci(&generate_legal(&root), "h2h3").expect("a legal move");
    let mut board = Position::new(root);
    search.root_depth = DEPTH;
    search.make(&mut board, 0, m);
    let root_frame = search.heuristics.frames[0].expect("the root move's frame");
    // Black's king onto a1, which no move below the null move can be.
    let stale = 11 * 64;
    search.heuristics.frames[1] = Some(stale);
    search.heuristics.evals[1] = Some(BETA + 200);
    let _ = search.null_move(&mut board, DEPTH, 1, BETA - 1, BETA);

    // Nodes below it try their own, but only after this one was admitted.
    assert!(
        search.stats.null_attempts.get() > 0,
        "the null move was refused"
    );
    assert_eq!(
        conthist::context(&search.heuristics.frames, 2, 1),
        None,
        "one ply up"
    );
    assert_eq!(
        conthist::context(&search.heuristics.frames, 2, 2),
        Some(root_frame),
        "two plies up"
    );
    let written = |context| {
        search
            .heuristics
            .conthist
            .row(context)
            .iter()
            .any(|&v| v != 0)
    };
    assert!(
        !written(stale),
        "a node below the null move read its ply's old frame"
    );
    assert!(
        written(root_frame),
        "no node below the null move read the root's move"
    );
}

/// A ply clear of both ends, so the bounds below are `mated_in(3)` = -31,997 and `mate_in(4)` =
/// 31,996.
const PLY: usize = 3;

#[test]
fn a_full_window_is_narrowed_to_being_mated_here_and_mating_next() {
    assert_eq!(
        mate_window(1, -INFINITE, INFINITE),
        Ok((mated_in(1), mate_in(2)))
    );
    assert_eq!(mate_window(1, -INFINITE, INFINITE), Ok((-31_999, 31_998)));
    // Inside both bounds nothing moves.
    assert_eq!(mate_window(PLY, -100, 100), Ok((-100, 100)));
}

#[test]
fn a_beta_at_or_below_being_mated_here_is_answered_by_that_bound() {
    let worst = mated_in(PLY);
    assert_eq!(mate_window(PLY, -INFINITE, worst), Err(worst));
    assert_eq!(mate_window(PLY, -INFINITE, worst), Err(-31_997));
    assert_eq!(mate_window(PLY, -INFINITE, worst - 1), Err(worst));
}

#[test]
fn an_alpha_at_or_above_mating_next_is_answered_by_that_bound() {
    let best = mate_in(PLY + 1);
    assert_eq!(mate_window(PLY, best, INFINITE), Err(best));
    assert_eq!(mate_window(PLY, best, INFINITE), Err(31_996));
    assert_eq!(mate_window(PLY, best + 1, INFINITE), Err(best));
}

/// An open window the clamp would leave one point wide stays as it was, at either end; one it
/// would leave two points wide is narrowed.
#[test]
fn an_open_window_is_never_narrowed_to_one_point() {
    let (worst, best) = (mated_in(PLY), mate_in(PLY + 1));
    assert_eq!(
        mate_window(PLY, best - 1, INFINITE),
        Ok((best - 1, INFINITE))
    );
    assert_eq!(
        mate_window(PLY, -INFINITE, worst + 1),
        Ok((-INFINITE, worst + 1))
    );
    assert_eq!(mate_window(PLY, best - 2, INFINITE), Ok((best - 2, best)));
    assert_eq!(mate_window(PLY, best - 2, INFINITE), Ok((31_994, 31_996)));
    assert_eq!(
        mate_window(PLY, -INFINITE, worst + 2),
        Ok((worst, worst + 2))
    );
    assert_eq!(
        mate_window(PLY, -INFINITE, worst + 2),
        Ok((-31_997, -31_995))
    );
}

/// The deepest ply a node reaches the table at, since a node at `MAX_PLY` returns before it.
/// Mating at the next ply there is `mate_in(MAX_PLY)`, the edge of the mate range.
#[test]
fn the_deepest_ply_bounds_the_window_at_the_edge_of_the_mate_range() {
    let ply = MAX_PLY - 1;
    let (worst, best) = (-MATE_IN_MAX_PLY - 1, MATE_IN_MAX_PLY);
    assert_eq!((worst, best), (-31_745, 31_744));
    assert_eq!((mated_in(ply), mate_in(ply + 1)), (worst, best));
    assert_eq!(mate_window(ply, -INFINITE, INFINITE), Ok((worst, best)));
    assert_eq!(mate_window(ply, -INFINITE, worst), Err(worst));
    assert_eq!(mate_window(ply, best, INFINITE), Err(best));
    assert_eq!(mate_window(ply, worst, worst + 1), Ok((worst, worst + 1)));
    assert_eq!(mate_window(ply, best - 1, best), Ok((best - 1, best)));
}

/// A null window touching a bound from outside is answered by it, and one just inside passes
/// through unchanged.
#[test]
fn a_null_window_is_answered_at_a_bound_and_unchanged_just_inside_it() {
    let (worst, best) = (mated_in(PLY), mate_in(PLY + 1));
    assert_eq!(mate_window(PLY, worst - 1, worst), Err(worst));
    assert_eq!(mate_window(PLY, best, best + 1), Err(best));
    assert_eq!(mate_window(PLY, worst, worst + 1), Ok((worst, worst + 1)));
    assert_eq!(mate_window(PLY, best - 1, best), Ok((best - 1, best)));
}
