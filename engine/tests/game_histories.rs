// SPDX-License-Identifier: GPL-3.0-or-later

//! A search's move-ordering histories carry into the next `go` of the same game, so a search
//! after another spends a different node count from the same search on a fresh engine.

mod support;

const DEPTH: u32 = 7;

/// Rich enough that every history table is written well before depth seven.
const FIRST: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

/// No queens, and white pawns on d2 and e2 and black pawns on b7, e7, g7 and h7, none of which
/// any position after `FIRST` can hold. A pawn never returns to its second rank, so the table
/// `FIRST` leaves is never probed here and only the histories can carry over.
const SECOND: &str = "r1b2rk1/1p2p1pp/p1n2n2/8/8/2N2N2/P2PP1PP/R1B2RK1 w - - 0 1";

/// The node count of the last completed iteration at `DEPTH`, for each `fen` in turn in one
/// process.
fn nodes_in_one_game(fens: &[&str]) -> Vec<u64> {
    let go = format!("go depth {DEPTH}");
    let mut engine = support::Engine::spawn();
    engine.sync();
    let mut counts = Vec::new();
    for fen in fens {
        engine.send(&format!("position fen {fen}"));
        engine.send(&go);
        let lines = engine.read_until("bestmove ");
        let info = lines
            .iter()
            .rfind(|l| l.starts_with(&format!("info depth {DEPTH} ")))
            .unwrap_or_else(|| panic!("{fen}: no completed iteration in {lines:?}"));
        let toks: Vec<&str> = info.split_whitespace().collect();
        let at = toks.iter().position(|t| *t == "nodes").expect("nodes");
        counts.push(toks[at + 1].parse().expect("a node count"));
    }
    engine.quit();
    counts
}

/// Equal counts mean the second search started from empty histories, as a fresh engine's does.
#[test]
fn a_later_search_in_a_game_starts_from_what_earlier_ones_learned() {
    let after = nodes_in_one_game(&[FIRST, SECOND])[1];
    let fresh = nodes_in_one_game(&[SECOND])[0];
    assert_ne!(
        after, fresh,
        "the search after another spent what a fresh engine does, so nothing carried over"
    );
}
