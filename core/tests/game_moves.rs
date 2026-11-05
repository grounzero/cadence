// SPDX-License-Identifier: GPL-3.0-or-later

//! `play` is the only way a key reaches `game_history()`, which no `MAX_PLY` bounds. `duplicate`
//! must carry the placement, the state stack at its ply, and the history.

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

/// The played board must stay at ply zero with a history of exactly the keys it left, oldest first.
/// Past `MAX_PLY` it is checked against a key recomputed from the placement instead.
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

/// A history bounded by the stack is the bug the two-sequence split exists to prevent.
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

/// So a duplicate taken mid-line can unmake back to the root.
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

fn by_uci(board: &Board, uci: &str) -> Move {
    generate_legal(board)
        .iter()
        .find(|m| m.to_uci_chess960() == uci)
        .unwrap_or_else(|| panic!("{uci} is not legal in {}", board.to_fen(FenStyle::XFen)))
}

/// The move number stops at the most it holds, and every move unmade from there restores the number
/// it was made at, null moves and game moves included.
#[test]
fn the_move_number_saturates_and_unmakes_exactly() {
    let mut board = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 65534").expect("fen parses");
    let mut made = Vec::new();
    for (uci, after) in [
        ("e1d1", 65534),
        ("e8d8", 65535),
        ("d1e1", 65535),
        ("d8e8", 65535),
    ] {
        let m = by_uci(&board, uci);
        made.push((m, board.fullmove_number()));
        board.make_move(m);
        assert_eq!(board.fullmove_number(), after, "after {uci}");
    }
    while let Some((m, before)) = made.pop() {
        board.unmake_move(m);
        assert_eq!(board.fullmove_number(), before);
    }

    let mut board = Board::from_fen("4k3/8/8/8/8/8/8/4K3 b - - 0 65535").expect("fen parses");
    board.make_null_move();
    assert_eq!(board.fullmove_number(), 65535);
    board.unmake_null_move();
    assert_eq!(board.fullmove_number(), 65535);
    board.play(by_uci(&board, "e8d8"));
    assert_eq!(
        board.to_fen(FenStyle::XFen),
        "3k4/8/8/8/8/8/8/4K3 w - - 1 65535"
    );
}
