// SPDX-License-Identifier: GPL-3.0-or-later

//! The quiet, continuation and correction histories outlive a `go` within a game, and are emptied
//! wherever the table is, so a new game searches as a fresh engine does.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_engine::search::{Learned, Limits};
use support::table;

/// Deep enough that every table is written and the move order moves the node count.
const DEPTH: u32 = 6;

/// The UCI half's depth, which `tests/tt.rs` already runs through the binary.
const UCI_DEPTH: u32 = 7;

/// A fresh table each time, so only the histories can carry anything from one search to the next.
fn fresh_nodes(fen: &str) -> u64 {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut search = support::search(Limits::depth(DEPTH), &stop, &tt);
    search.run(&mut support::position(fen), &mut Vec::new());
    assert_eq!(search.completed_depth(), DEPTH, "{fen}: depth not reached");
    search.nodes()
}

/// As [`fresh_nodes`], with `learned` lent to the search and taken back after it.
fn nodes_with(fen: &str, learned: &mut Learned) -> u64 {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut search = support::search(Limits::depth(DEPTH), &stop, &tt);
    search.swap_learned(learned);
    search.run(&mut support::position(fen), &mut Vec::new());
    search.swap_learned(learned);
    assert_eq!(search.completed_depth(), DEPTH, "{fen}: depth not reached");
    search.nodes()
}

/// The search writes into the lent tables and hands them back, keeping the empty ones it had.
#[test]
fn a_search_returns_what_it_learned_to_the_caller() {
    let fen = support::standard_fen("kiwipete");
    let mut learned = Learned::new();
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut board = support::position(&fen);
    let us = board.side_to_move();
    let mut search = support::search(Limits::depth(DEPTH), &stop, &tt);
    search.swap_learned(&mut learned);
    search.run(&mut board, &mut Vec::new());
    assert!(
        search.history().side(us).iter().any(|&v| v != 0),
        "{fen}: the search wrote no history"
    );
    search.swap_learned(&mut learned);
    assert!(
        search.history().side(us).iter().all(|&v| v == 0),
        "{fen}: the written history stayed with the search"
    );
}

/// A run's `begin` leaves lent tables as they are, so the next search orders by them.
#[test]
fn a_search_given_learned_histories_starts_from_them() {
    let fen = support::standard_fen("kiwipete");
    let mut learned = Learned::new();
    let first = nodes_with(&fen, &mut learned);
    assert_eq!(first, fresh_nodes(&fen), "empty tables differ from none");
    let second = nodes_with(&fen, &mut learned);
    assert_ne!(
        second, first,
        "{fen}: the second search spent what an empty one does, so nothing carried over"
    );
}

/// What `ucinewgame` does to each set, after which a search spends what a fresh one does.
#[test]
fn cleared_histories_search_as_a_fresh_engine_does() {
    let fen = support::standard_fen("kiwipete");
    let mut learned = Learned::new();
    nodes_with(&fen, &mut learned);
    learned.clear();
    assert_eq!(nodes_with(&fen, &mut learned), fresh_nodes(&fen));
}

/// Three searches of the start position in one process, `between` sent before the third.
fn three_searches(between: &str) -> Vec<u64> {
    let go = format!("go depth {UCI_DEPTH}");
    let mut engine = support::Engine::spawn();
    engine.sync();
    let mut counts = Vec::new();
    for round in 0..3 {
        if round == 2 {
            engine.send(between);
        }
        engine.send("position startpos");
        engine.send(&go);
        let lines = engine.read_until("bestmove ");
        let info = lines
            .iter()
            .rfind(|l| l.starts_with(&format!("info depth {UCI_DEPTH} ")))
            .unwrap_or_else(|| panic!("no completed iteration in {lines:?}"));
        let toks: Vec<&str> = info.split_whitespace().collect();
        let at = toks.iter().position(|t| *t == "nodes").expect("nodes");
        counts.push(toks[at + 1].parse().expect("a node count"));
    }
    engine.quit();
    counts
}

/// The button is `ucinewgame`'s table half, and the histories go with the table.
#[test]
fn clear_hash_empties_the_histories() {
    let counts = three_searches("setoption name Clear Hash");
    assert_eq!(
        counts[2], counts[0],
        "the search after Clear Hash did not start as a fresh engine: {counts:?}"
    );
}
