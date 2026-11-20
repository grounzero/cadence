// SPDX-License-Identifier: GPL-3.0-or-later

//! A known mate bounds every window below it: a node cannot score better than mating at the next
//! ply, nor worse than being mated where it stands, so a search that has found a mate stays small.

mod support;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_engine::position::Position;
use cadence_engine::score::{Score, mate_in, mated_in};
use cadence_engine::search::Limits;
use cadence_nnue::Network;
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
    if let Some(net) = Network::shipped(EMBEDDED) {
        let net = Arc::new(net.expect("the shipped net loads"));
        out.push(("net", Position::with_net(board(), net)));
    }
    out
}

/// The depth completed, the score and the nodes spent under the node limit.
fn searched(board: &mut Position) -> (u32, Score, u64) {
    let stop = AtomicBool::new(false);
    let tt = support::table();
    let mut limits = Limits::depth(DEPTH);
    limits.nodes = Some(NODES);
    let mut search = support::search(limits, &stop, &tt);
    search.run(board, &mut std::io::sink());
    (search.completed_depth(), search.score(), search.nodes())
}

#[test]
fn a_forced_mate_finishes_every_depth_within_the_node_limit() {
    for (fen, mate) in [(FORCED, mated_in(2)), (MATING, mate_in(1))] {
        for (eval, mut board) in positions(fen) {
            let (depth, score, nodes) = searched(&mut board);
            assert_eq!(depth, DEPTH, "{eval}, {fen}: {nodes} nodes");
            assert_eq!(score, mate, "{eval}, {fen}");
        }
    }
}
