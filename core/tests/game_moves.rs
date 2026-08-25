// SPDX-License-Identifier: GPL-3.0-or-later

//! Game moves, as opposed to search moves, and position duplication.
//!
//! `Board` keeps two key sequences: the search stack, indexed by ply and
//! bounded by `MAX_PLY`, and the game history, which grows only on real game
//! moves and is not bounded by anything. `make_move` pushes the
//! former. `play` is the operation that advances the latter: it is what the
//! UCI `position` handler calls for each move in a `moves` list, and it is the
//! only way a key ever reaches `game_history()`.
//!
//! `duplicate` is the named, documented copy of a position (the search
//! thread's own `Board`, made once per `go`), and it has to carry everything:
//! the placement, the state stack at its current ply, and the history.

mod support;

use cadence_core::fen::START_FEN;
use cadence_core::position::Board;
use cadence_core::{FenStyle, MAX_PLY, Move, generate_legal};
use support::generative::{Rng, fingerprint, legal};

/// A random legal move, or `None` at a terminal position.
fn random_move(board: &Board, rng: &mut Rng) -> Option<Move> {
    let moves = legal(board);
    if moves.is_empty() {
        None
    } else {
        Some(moves[rng.below(moves.len())])
    }
}

/// Play `plies` random game moves with `play`, and the same moves with
/// `make_move` on a second board that is never unmade. At every step the
/// two boards must agree on the whole position, the played board must sit
/// at ply zero, and its history must be exactly the keys of the positions
/// it has left behind, oldest first.
///
/// The `make_move` board is the search stack and stops at `MAX_PLY`; the
/// played board does not, and past that point it is checked against a key
/// recomputed from the placement instead.
fn walk_and_check(fen: &str, seed: u64, plies: usize) -> usize {
    let mut played = Board::from_fen(fen).expect("fen parses");
    let mut made = Board::from_fen(fen).expect("fen parses");
    let mut rng = Rng::new(seed);
    let mut keys = Vec::new();
    let mut n = 0;
    while n < plies {
        let Some(m) = random_move(&played, &mut rng) else {
            break;
        };
        keys.push(played.key());
        played.play(m);
        n += 1;

        assert_eq!(played.ply(), 0, "play must leave the root at ply zero");
        assert_eq!(played.game_history(), &keys[..]);
        assert_eq!(played.key(), played.recompute_key());
        assert_eq!(played.pawn_key(), played.recompute_pawn_key());

        if n <= MAX_PLY {
            made.make_move(m);
            assert_eq!(made.ply(), n, "make_move is the search stack");
            assert_eq!(
                played.to_fen(FenStyle::Shredder),
                made.to_fen(FenStyle::Shredder),
                "after {n} plies from {fen} (seed {seed})"
            );
            assert_eq!(played.key(), made.key());
            assert_eq!(played.checkers(), made.checkers());
            assert!(
                made.game_history().is_empty(),
                "make_move must never touch the game history"
            );
        }
    }
    n
}

#[test]
fn play_advances_the_game_history_and_keeps_the_root_at_ply_zero() {
    for (i, fen) in support::generative::walk_seeds().iter().enumerate() {
        walk_and_check(fen, 0xC0FFEE + i as u64, 60);
    }
}

/// The search stack is `MAX_PLY + 1` deep. A game is not. A walk of more
/// than `MAX_PLY` game moves must not run out of anything: a history bounded
/// by the stack is exactly the bug the two-sequence split exists to prevent.
#[test]
fn game_history_is_not_bounded_by_max_ply() {
    let target = MAX_PLY + 60;
    let mut longest = 0;
    for seed in 1..=8u64 {
        longest = longest.max(walk_and_check(START_FEN, seed, target));
        if longest >= target {
            break;
        }
    }
    assert!(
        longest >= target,
        "no seed produced a {target}-ply game (longest {longest}); pick another seed"
    );
}

#[test]
fn play_in_a_dfrc_game_records_castles_like_any_other_move() {
    // The corpus's arrays that can castle at their first move: the castle
    // is played as a game move, then the walk goes on from there.
    for case in support::immediate_castles() {
        let mut board = Board::from_fen(&case.fen).expect("dfrc fen parses");
        let castle = support::legal_move_named(&case.fen, &case.fen, &case.castling);
        assert!(castle.is_castle());
        let before = board.key();
        board.play(castle);
        assert_eq!(board.game_history(), &[before]);
        assert_eq!(board.ply(), 0);
        assert_eq!(board.key(), board.recompute_key());

        let mut rng = Rng::new(u64::from(case.wid) * 1000 + u64::from(case.bid));
        for i in 1..=40 {
            let Some(m) = random_move(&board, &mut rng) else {
                break;
            };
            board.play(m);
            assert_eq!(board.game_history().len(), i + 1);
            assert_eq!(board.key(), board.recompute_key());
        }
    }
}

#[test]
fn duplicate_is_equal_and_independent() {
    let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    let mut original = Board::from_fen(fen).expect("kiwipete parses");
    let mut rng = Rng::new(7);
    for _ in 0..10 {
        let m = random_move(&original, &mut rng).expect("kiwipete does not end in 10");
        original.play(m);
    }

    let mut copy = original.duplicate();
    assert_eq!(fingerprint(&copy), fingerprint(&original));
    assert_eq!(copy.game_history(), original.game_history());
    assert_eq!(copy.ply(), original.ply());
    assert_eq!(copy.halfmove_clock(), original.halfmove_clock());
    assert_eq!(copy.fullmove_number(), original.fullmove_number());

    // Mutating the copy leaves the original alone, in both directions.
    let before = fingerprint(&original);
    let m = random_move(&copy, &mut rng).expect("not terminal");
    copy.make_move(m);
    assert_eq!(fingerprint(&original), before);
    assert_eq!(original.ply(), 0);
    copy.unmake_move(m);
    assert_eq!(fingerprint(&copy), before);

    let history_len = copy.game_history().len();
    let m = random_move(&original, &mut rng).expect("not terminal");
    original.play(m);
    assert_eq!(copy.game_history().len(), history_len);
    assert_eq!(fingerprint(&copy), before);
}

/// The copy carries the whole state stack, not just the current slot: a
/// duplicate taken mid-line can unmake its way back to the root.
#[test]
fn duplicate_carries_the_search_stack() {
    let mut board = Board::from_fen(START_FEN).expect("startpos parses");
    let root = fingerprint(&board);
    let mut rng = Rng::new(11);
    let mut line = Vec::new();
    for _ in 0..6 {
        let m = random_move(&board, &mut rng).expect("not terminal");
        board.make_move(m);
        line.push(m);
    }
    let mut copy = board.duplicate();
    assert_eq!(copy.ply(), 6);
    assert_eq!(fingerprint(&copy), fingerprint(&board));
    for m in line.iter().rev() {
        copy.unmake_move(*m);
    }
    assert_eq!(copy.ply(), 0);
    assert_eq!(fingerprint(&copy), root);
    // And the original is still at the end of the line.
    assert_eq!(board.ply(), 6);
}

#[test]
fn start_fen_is_the_corpus_start_position() {
    let corpus = support::standard_positions()
        .into_iter()
        .find(|p| p.name == "startpos")
        .expect("the corpus names the start position");
    assert_eq!(START_FEN, corpus.fen);
    let board = Board::from_fen(START_FEN).expect("START_FEN parses");
    assert_eq!(board.to_fen(FenStyle::XFen), START_FEN);
    assert_eq!(generate_legal(&board).len(), 20);
}
