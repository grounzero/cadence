// SPDX-License-Identifier: GPL-3.0-or-later

//! The board holds `MAX_PLY` plies of state, so a deeper perft is refused before it makes a move.
//! Mated roots end at once at any depth, so the refusal is all a test here can wait on.

use cadence_core::{Board, MAX_PLY, generate_legal, parse_uci, perft, perft_divide};

/// Black to move and mated.
const MATED: &str = "7k/6Q1/6K1/8/8/8/8/8 b - - 0 1";

fn mated() -> Board {
    Board::from_fen(MATED).expect("a legal FEN")
}

fn deepest() -> u32 {
    u32::try_from(MAX_PLY).expect("a small constant")
}

#[test]
fn a_depth_the_board_holds_is_counted() {
    assert_eq!(perft(&mut mated(), deepest()), 0);
    assert!(perft_divide(&mut mated(), deepest()).is_empty());
}

#[test]
#[should_panic(expected = "deeper than the board holds")]
fn perft_refuses_a_depth_past_max_ply() {
    perft(&mut mated(), deepest() + 1);
}

#[test]
#[should_panic(expected = "deeper than the board holds")]
fn perft_divide_refuses_a_depth_past_max_ply() {
    let _ = perft_divide(&mut mated(), deepest() + 1);
}

/// White mates in one, so after it the root is mated one ply in.
#[test]
#[should_panic(expected = "deeper than the board holds")]
fn the_limit_counts_the_plies_already_made() {
    let mut board = Board::from_fen("7k/Q7/6K1/8/8/8/8/8 w - - 0 1").expect("a legal FEN");
    let mate = parse_uci(&generate_legal(&board), "a7g7").expect("a legal move");
    board.make_move(mate);
    perft(&mut board, deepest());
}
