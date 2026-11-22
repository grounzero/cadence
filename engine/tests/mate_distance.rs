// SPDX-License-Identifier: GPL-3.0-or-later

//! A known mate bounds every window below it: a node cannot score better than mating at the next
//! ply, nor worse than being mated where it stands, so a search that has found a mate stays small.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::Move;
use cadence_core::position::Board;
use cadence_engine::position::Position;
use cadence_engine::score::{Score, mate_in, mated_in};
use cadence_engine::search::Limits;
use cadence_engine::tt::Table;
use cadence_nnue::AnyNetwork;
use cadence_nnue::net::EMBEDDED;

/// The king's step to f1 is forced, and the queen then mates on f2 under the pawn's guard.
const FORCED: &str = "4k3/8/8/8/8/4p3/3q4/4K2R w K - 0 1";
/// The same mate, one ply on, with the mating side to move.
const MATING: &str = "4k3/8/8/8/8/4p3/3q4/5K1R b - - 1 1";

/// Far past the mate, and far under what a search proving it again at every depth would spend.
const DEPTH: u32 = 40;
const NODES: u64 = 100_000;

/// Both evaluations, since a mate's window does not depend on how the rest is scored.
fn positions(fen: &str) -> Vec<(&'static str, Position)> {
    let board = || Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
    let mut out = vec![("handcrafted", Position::new(board()))];
    if let Some(net) = AnyNetwork::shipped(EMBEDDED) {
        let net = net.expect("the shipped net loads");
        out.push(("net", Position::with_net(board(), net)));
    }
    out
}

/// What one search under the node limit reports.
struct Searched {
    best: Move,
    depth: u32,
    score: Score,
    nodes: u64,
    /// Each completed depth's move and score, shallowest first.
    roots: Vec<(Move, Score)>,
}

/// With `tt` as it stands, so a caller can keep one table across searches.
fn searched_with(board: &mut Position, tt: &Table) -> Searched {
    let stop = AtomicBool::new(false);
    let mut limits = Limits::depth(DEPTH);
    limits.nodes = Some(NODES);
    let mut search = support::search(limits, &stop, tt);
    let best = search.run(board, &mut std::io::sink());
    Searched {
        best,
        depth: search.completed_depth(),
        score: search.score(),
        nodes: search.nodes(),
        roots: search.iteration_roots().to_vec(),
    }
}

/// On a fresh table.
fn searched(board: &mut Position) -> Searched {
    searched_with(board, &support::table())
}

/// Every depth completes under the node limit, and from the first depth that proves the mate every
/// later one reports it.
#[test]
fn a_forced_mate_finishes_every_depth_within_the_node_limit() {
    for (fen, mate) in [(FORCED, mated_in(2)), (MATING, mate_in(1))] {
        for (eval, mut board) in positions(fen) {
            let s = searched(&mut board);
            assert_eq!(s.depth, DEPTH, "{eval}, {fen}: {} nodes", s.nodes);
            assert_eq!(s.score, mate, "{eval}, {fen}");
            assert_eq!(
                u32::try_from(s.roots.len()),
                Ok(DEPTH),
                "{eval}, {fen}: one root per completed depth"
            );
            let proved = s
                .roots
                .iter()
                .position(|&(_, score)| score == mate)
                .unwrap_or_else(|| panic!("{eval}, {fen}: no depth proves it: {:?}", s.roots));
            for (depth, &(_, score)) in (1_u32..).zip(&s.roots).skip(proved) {
                assert_eq!(
                    score,
                    mate,
                    "{eval}, {fen}: proved at depth {}, lost at depth {depth}",
                    proved + 1
                );
            }
        }
    }
}

/// A table holding the mate, found from this position or from the one before it, changes nothing a
/// search reports: the move, the score and the depth reached match a fresh table's.
#[test]
fn a_table_warmed_by_the_mate_reports_what_a_fresh_one_does() {
    let reported = |s: &Searched| (s.best, s.score, s.depth);
    for i in 0..positions(MATING).len() {
        let fresh = |fen: &str| {
            let (_, mut board) = positions(fen).swap_remove(i);
            reported(&searched(&mut board))
        };
        let (fresh_mating, fresh_forced) = (fresh(MATING), fresh(FORCED));
        let tt = support::table();
        for (fen, expected) in [
            (MATING, fresh_mating),
            (FORCED, fresh_forced),
            (MATING, fresh_mating),
        ] {
            let (eval, mut board) = positions(fen).swap_remove(i);
            let warm = searched_with(&mut board, &tt);
            assert_eq!(
                reported(&warm),
                expected,
                "{eval}, {fen}: {} nodes",
                warm.nodes
            );
        }
    }
}
