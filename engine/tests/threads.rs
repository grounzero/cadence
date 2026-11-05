// SPDX-License-Identifier: GPL-3.0-or-later

//! The search with helper threads and with pondering, driven through the binary: one answer and a
//! legal one, node counts that only rise, a node limit the group keeps, and a prompt answer on
//! `ponderhit` and `stop`.

mod support;

use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{START_FEN, generate_legal, parse_uci};
use support::Engine;

const POSITIONS: [&str; 4] = [
    START_FEN,
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
];

/// The `nodes` of every `info` line, in order.
fn node_counts(lines: &[String]) -> Vec<u64> {
    lines
        .iter()
        .filter(|l| l.starts_with("info "))
        .filter_map(|l| {
            let mut words = l.split_whitespace();
            words.find(|&w| w == "nodes")?;
            words.next()?.parse().ok()
        })
        .collect()
}

fn assert_one_legal_answer(fen: &str, lines: &[String], label: &str) {
    let answers: Vec<&String> = lines.iter().filter(|l| l.starts_with("bestmove ")).collect();
    assert_eq!(answers.len(), 1, "{label}: {lines:?}");
    let mv = answers[0].split_whitespace().nth(1).expect("a move");
    let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
    assert!(
        parse_uci(&generate_legal(&board), mv).is_some(),
        "{label}: {mv} is not legal in {fen}"
    );
}

#[test]
fn a_parallel_search_answers_once_with_a_legal_move_and_counts_upward() {
    for threads in [2, 4] {
        for fen in POSITIONS {
            let lines = Engine::within(Duration::from_secs(120), move || {
                let mut e = Engine::spawn();
                e.send(&format!("setoption name Threads value {threads}"));
                e.send(&format!("position fen {fen}"));
                e.sync();
                e.send("go depth 6");
                let mut lines = e.read_until("bestmove ");
                // A second answer would arrive before this one's `readyok`.
                lines.extend(e.sync());
                e.quit();
                lines
            });
            let label = format!("Threads {threads}, {fen}");
            assert_one_legal_answer(fen, &lines, &label);
            let nodes = node_counts(&lines);
            assert!(!nodes.is_empty(), "{label}: no info line");
            assert!(nodes.windows(2).all(|w| w[0] <= w[1]), "{label}: {nodes:?}");
        }
    }
}

/// Every worker reads the group's count, which a sibling publishes every 64 nodes, so the group
/// stops within a few slices of the limit.
#[test]
fn a_node_limit_holds_across_threads() {
    const LIMIT: u64 = 20_000;
    let fen = POSITIONS[1];
    let lines = Engine::within(Duration::from_secs(60), move || {
        let mut e = Engine::spawn();
        e.send("setoption name Threads value 3");
        e.send(&format!("position fen {fen}"));
        e.sync();
        e.send(&format!("go nodes {LIMIT}"));
        let lines = e.read_until("bestmove ");
        e.quit();
        lines
    });
    assert_one_legal_answer(fen, &lines, "go nodes");
    let nodes = node_counts(&lines);
    assert!(!nodes.is_empty(), "no info line: {lines:?}");
    for n in nodes {
        assert!(
            n <= LIMIT + 3 * 1024,
            "{n} nodes against {LIMIT}: {lines:?}"
        );
    }
}

/// After the hit the search spends the clock `go ponder` carried, at most half of what is left, so
/// the answer comes inside it; the margin is for a loaded runner.
#[test]
fn a_ponderhit_is_answered_inside_the_budget() {
    for threads in [1, 2, 4] {
        let elapsed = Engine::within(Duration::from_secs(30), move || {
            let mut e = Engine::spawn();
            e.send(&format!("setoption name Threads value {threads}"));
            e.send("position startpos");
            e.sync();
            e.send("go ponder wtime 2000 btime 2000");
            std::thread::sleep(Duration::from_millis(300));
            let start = Instant::now();
            e.send("ponderhit");
            e.read_until("bestmove ");
            let elapsed = start.elapsed();
            e.quit();
            elapsed
        });
        assert!(
            elapsed < Duration::from_millis(1500),
            "Threads {threads}: {elapsed:?} after the hit"
        );
    }
}

/// A search with no budget of its own answers `stop` at once, whatever the threads.
#[test]
fn an_open_ended_search_answers_stop_at_once() {
    for go in ["go ponder wtime 60000 btime 60000", "go infinite"] {
        for threads in [1, 3] {
            let (elapsed, lines) = Engine::within(Duration::from_secs(30), move || {
                let mut e = Engine::spawn();
                e.send(&format!("setoption name Threads value {threads}"));
                e.send("position startpos");
                e.sync();
                e.send(go);
                std::thread::sleep(Duration::from_millis(300));
                let start = Instant::now();
                e.send("stop");
                let lines = e.read_until("bestmove ");
                let elapsed = start.elapsed();
                e.quit();
                (elapsed, lines)
            });
            assert!(
                elapsed < Duration::from_millis(1000),
                "`{go}` at Threads {threads}: {elapsed:?}, {lines:?}"
            );
            assert_one_legal_answer(START_FEN, &lines, go);
        }
    }
}
