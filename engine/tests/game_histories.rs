// SPDX-License-Identifier: GPL-3.0-or-later

//! A search's move-ordering histories carry into the next `go` of the same game, so a search
//! after another spends a different node count from the same search on a fresh engine.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_engine::search::{Learned, Limits};

const DEPTH: u32 = 7;

/// Rich enough that every history table is written well before depth seven.
const FIRST: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

/// No queens, and white pawns on d2 and e2 and black pawns on b7, e7, g7 and h7, none of which
/// any position after `FIRST` can hold. A pawn never returns to its second rank, so the table
/// `FIRST` leaves is never probed here and only the histories can carry over.
const SECOND: &str = "r1b2rk1/1p2p1pp/p1n2n2/8/8/2N2N2/P2PP1PP/R1B2RK1 w - - 0 1";

/// As the session searches a game, a new `Search` per `go` with one set of histories lent to each,
/// but on a new table every time. Every way a session replaces its table also clears the
/// histories, so only here does the second search share nothing with the first but them.
fn nodes_in_process(fens: &[&str]) -> Vec<u64> {
    let stop = AtomicBool::new(false);
    let mut learned = Learned::new();
    let mut counts = Vec::new();
    for fen in fens {
        let tt = support::table();
        let mut search = support::search(Limits::depth(DEPTH), &stop, &tt);
        search.swap_learned(&mut learned);
        search.run(&mut support::position(fen), &mut Vec::new());
        search.swap_learned(&mut learned);
        assert_eq!(search.completed_depth(), DEPTH, "{fen}: depth not reached");
        counts.push(search.nodes());
    }
    counts
}

/// The node count of the last completed iteration at `DEPTH`, for each `fen` in turn in one
/// process, `between` sent before every search after the first.
fn nodes_in_one_game(fens: &[&str], between: Option<&str>) -> Vec<u64> {
    let go = format!("go depth {DEPTH}");
    let mut engine = support::Engine::spawn();
    engine.sync();
    let mut counts = Vec::new();
    for (i, fen) in fens.iter().enumerate() {
        if let Some(line) = between.filter(|_| i > 0) {
            engine.send(line);
        }
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

/// Equal counts mean the second search started from empty histories, as a fresh search's does.
#[test]
fn a_later_search_in_a_game_starts_from_what_earlier_ones_learned() {
    let after = nodes_in_process(&[FIRST, SECOND])[1];
    let fresh = nodes_in_process(&[SECOND])[0];
    assert_ne!(
        after, fresh,
        "the search after another spent what a fresh one does, so nothing carried over"
    );
}

/// The same through the binary, where the table is kept too and `SECOND` is what keeps it out.
#[test]
fn the_session_keeps_the_histories_from_one_go_to_the_next() {
    let after = nodes_in_one_game(&[FIRST, SECOND], None)[1];
    let fresh = nodes_in_one_game(&[SECOND], None)[0];
    assert_ne!(
        after, fresh,
        "the search after another spent what a fresh engine does, so nothing carried over"
    );
}

/// The pair the test above shows the histories alone move, so an equal count here shows
/// `ucinewgame` emptied them.
#[test]
fn ucinewgame_empties_the_histories_of_the_game_before() {
    let after = nodes_in_one_game(&[FIRST, SECOND], Some("ucinewgame"))[1];
    let fresh = nodes_in_one_game(&[SECOND], None)[0];
    assert_eq!(
        after, fresh,
        "the search after ucinewgame did not spend what a fresh engine does"
    );
}
