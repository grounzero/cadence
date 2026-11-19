// SPDX-License-Identifier: GPL-3.0-or-later

//! The share of a completed iteration's root nodes that lay under its best move, which a clocked
//! search reads to stop early on a settled root or late on a contested one.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_engine::search::Limits;
use support::table;

/// The shares a fixed-depth search of `fen` recorded, one pair a completed iteration: of that
/// iteration's root nodes, and of the whole search's.
fn shares(fen: &str, depth: u32) -> Vec<(u32, u32)> {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut search = support::search(Limits::depth(depth), &stop, &tt);
    search.run(&mut support::position(fen), &mut std::io::sink());
    assert_eq!(
        search.iteration_shares().len(),
        search.iteration_roots().len(),
        "{fen}: a share for each completed iteration"
    );
    search.iteration_shares().to_vec()
}

/// A share is a part of the whole at every iteration of every position, and the first iteration is
/// the whole search so far.
#[test]
fn a_share_is_at_most_the_whole_root() {
    for fen in [
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "4k3/8/8/8/3q4/1N3N2/PPP3PP/4K3 w - - 0 1",
        "8/8/8/4k3/8/8/4P3/4K3 w - - 0 1",
    ] {
        let read = shares(fen, 7);
        assert_eq!(read.len(), 7, "{fen}");
        assert!(
            read.iter().all(|&(it, all)| it <= 1000 && all <= 1000),
            "{fen}: {read:?}"
        );
        assert_eq!(read[0].0, read[0].1, "{fen}: {read:?}");
    }
}

/// With one legal move every node of the root lies under it. The queen's check from d2 is held by
/// the pawn, so the king's step to f1 is all there is.
#[test]
fn a_root_with_one_legal_move_gives_it_the_whole_share() {
    let read = shares("4k3/8/8/8/8/4p3/3q4/4K2R w K - 0 1", 6);
    assert_ne!(read.len(), 0, "a completed iteration");
    assert!(read.iter().all(|&s| s == (1000, 1000)), "{read:?}");
}
