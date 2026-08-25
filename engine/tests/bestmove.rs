// SPDX-License-Identifier: GPL-3.0-or-later

//! The search as a function: whatever it returns is a legal move.
//!
//! This is the one property of the move chooser that every later search
//! must keep, and it is tested here independently of how the move is found.
//! The UCI plumbing around it -- `go`, `stop`, the `bestmove` line and its
//! spelling -- is tested against the binary in tests/uci.rs.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{Move, START_FEN, generate_legal};
use cadence_engine::search::{Limits, Search};
use support::{Rng, table};

/// Search `board` to the given limits with a fresh stop flag, discarding
/// `info` output.
fn best(board: &mut Board, limits: Limits) -> Move {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut sink = Vec::new();
    Search::new(limits, &stop, &tt).run(board, &mut sink)
}

fn assert_legal(fen: &str, m: Move) {
    let board = Board::from_fen(fen).expect("fen parses");
    let legal = generate_legal(&board);
    assert!(
        legal.contains(m),
        "{m:?} is not legal in {fen}; legal: {:?}",
        legal.as_slice()
    );
}

#[test]
fn the_search_returns_a_legal_move_from_every_corpus_position() {
    for fen in support::corpus_fens() {
        let mut board = Board::from_fen(&fen).expect("corpus fen parses");
        if generate_legal(&board).is_empty() {
            continue;
        }
        let m = best(&mut board, Limits::depth(1));
        assert_legal(&fen, m);
        // And the board comes back as it went in.
        assert_eq!(
            board.to_fen(cadence_core::FenStyle::Shredder),
            fen_normalised(&fen)
        );
        assert_eq!(board.ply(), 0);
    }
}

/// The corpus writes some FENs with four fields; the board emits six.
fn fen_normalised(fen: &str) -> String {
    Board::from_fen(fen)
        .expect("fen parses")
        .to_fen(cadence_core::FenStyle::Shredder)
}

#[test]
fn the_search_returns_a_legal_move_along_random_games() {
    let mut seeds: Vec<String> = vec![START_FEN.to_string()];
    seeds.extend(support::dfrc_arrays().into_iter().map(|(_, _, f)| f));
    let mut positions = 0;
    for (i, fen) in seeds.iter().enumerate() {
        let mut board = Board::from_fen(fen).expect("fen parses");
        let mut rng = Rng::new(0xBE57 + i as u64);
        for _ in 0..120 {
            let legal = generate_legal(&board);
            if legal.is_empty() {
                break;
            }
            let m = best(&mut board, Limits::depth(1));
            assert!(legal.contains(m), "{m:?} is not legal in {board:?}");
            positions += 1;
            // Walk on with a random move, not the chosen one, so the
            // positions reached do not depend on the search.
            board.play(legal.as_slice()[rng.below(legal.len())]);
        }
    }
    assert!(positions > 1000, "only {positions} positions searched");
}

#[test]
fn the_search_returns_null_when_there_is_no_legal_move() {
    for fen in [
        "7k/6Q1/6K1/8/8/8/8/8 b - - 0 1", // mated
        "7k/8/6Q1/8/8/8/8/7K b - - 0 1",  // stalemated
        "rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3", // fool's mate
    ] {
        let mut board = Board::from_fen(fen).expect("fen parses");
        assert!(generate_legal(&board).is_empty(), "{fen} has legal moves");
        assert_eq!(best(&mut board, Limits::depth(1)), Move::NULL, "{fen}");
    }
}

#[test]
fn the_search_is_a_function_of_the_position() {
    for fen in support::standard_fens() {
        let mut a = Board::from_fen(&fen).expect("fen parses");
        let mut b = Board::from_fen(&fen).expect("fen parses");
        let first = best(&mut a, Limits::depth(1));
        let second = best(&mut b, Limits::depth(1));
        assert_eq!(first, second, "{fen}");
        // And again on the same board.
        assert_eq!(best(&mut a, Limits::depth(1)), first, "{fen}");
    }
}

/// With the stop flag already raised, even `infinite` returns at once, and
/// with a legal move.
#[test]
fn a_raised_stop_flag_returns_a_legal_move_at_once() {
    let fen = support::standard_fen("kiwipete");
    let mut board = Board::from_fen(&fen).expect("kiwipete parses");
    let stop = AtomicBool::new(true);
    let mut sink = Vec::new();
    let start = Instant::now();
    let tt = table();
    let m = Search::new(Limits::infinite(), &stop, &tt).run(&mut board, &mut sink);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert_legal(&fen, m);
}

/// `infinite` does not return on its own; it returns when `stop` is raised,
/// and then with a legal move.
#[test]
fn infinite_waits_for_stop() {
    let fen = support::standard_fen("kiwipete");
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let worker = {
        let stop = stop.clone();
        let fen = fen.clone();
        std::thread::spawn(move || {
            let mut board = Board::from_fen(&fen).expect("kiwipete parses");
            let mut sink = Vec::new();
            let tt = table();
            Search::new(Limits::infinite(), &stop, &tt).run(&mut board, &mut sink)
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    assert!(!worker.is_finished(), "infinite returned without stop");
    stop.store(true, Ordering::Relaxed);
    let m = worker.join().expect("search thread");
    assert_legal(&fen, m);
}

#[test]
fn limits_parse_every_go_token() {
    let l = Limits::parse("wtime 1000 btime 2000 winc 10 binc 20 movestogo 30".split_whitespace());
    assert_eq!(l.time, [Some(1000), Some(2000)]);
    assert_eq!(l.inc, [Some(10), Some(20)]);
    assert_eq!(l.movestogo, Some(30));
    assert_eq!(l.depth, None);
    assert!(!l.infinite);
    assert_eq!(l.clock(cadence_core::Colour::White), Some((1000, 10)));
    assert_eq!(l.clock(cadence_core::Colour::Black), Some((2000, 20)));

    let l = Limits::parse("depth 7".split_whitespace());
    assert_eq!(l, Limits::depth(7));
    let l = Limits::parse("movetime 250".split_whitespace());
    assert_eq!(l.movetime, Some(250));
    let l = Limits::parse("nodes 12345".split_whitespace());
    assert_eq!(l.nodes, Some(12345));
    let l = Limits::parse("infinite".split_whitespace());
    assert_eq!(l, Limits::infinite());
    // Nothing at all: no limit applies.
    assert_eq!(Limits::parse("".split_whitespace()), Limits::default());
    // wtime without binc: the increment defaults to zero in `clock`.
    let l = Limits::parse("wtime 500".split_whitespace());
    assert_eq!(l.clock(cadence_core::Colour::White), Some((500, 0)));
    assert_eq!(l.clock(cadence_core::Colour::Black), None);
    // Unknown and ignored tokens do not disturb the rest.
    let l = Limits::parse("ponder searchmoves e2e4 d2d4 mate 3 depth 4 frob".split_whitespace());
    assert_eq!(l.depth, Some(4));
    // A token without its number, or with a non-number, is skipped.
    let l = Limits::parse("depth".split_whitespace());
    assert_eq!(l, Limits::default());
    let l = Limits::parse("depth x movetime 10".split_whitespace());
    assert_eq!(l.depth, None);
    assert_eq!(l.movetime, Some(10));
}
