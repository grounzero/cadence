// SPDX-License-Identifier: GPL-3.0-or-later

//! A position one reversible move from a draw by repetition is found before the move is made, and
//! agrees with `is_repetition` asked after it.

use cadence_core::position::Board;
use cadence_core::{Move, PieceType, START_FEN, cycles, generate_legal};

fn mv(board: &Board, uci: &str) -> Move {
    let legal = generate_legal(board);
    cadence_core::parse_uci(&legal, uci)
        .unwrap_or_else(|| panic!("{uci} is not legal in {board:?}"))
}

fn played(fen: &str, moves: &str) -> Board {
    let mut board = Board::from_fen(fen).expect("the position parses");
    for m in moves.split_whitespace() {
        let m = mv(&board, m);
        board.play(m);
    }
    board
}

fn made(board: &mut Board, moves: &str) {
    for m in moves.split_whitespace() {
        let m = mv(board, m);
        board.make_move(m);
    }
}

/// Whether making `uci` from here reaches a position the rule calls a draw.
fn repeats_after(board: &mut Board, uci: &str) -> bool {
    let m = mv(board, uci);
    board.make_move(m);
    let repeated = board.is_repetition();
    board.unmake_move(m);
    repeated
}

#[test]
fn the_table_finds_each_quiet_move_of_a_piece_and_no_other_move() {
    for fen in [
        "r2q1rk1/pp2bppp/2n1bn2/3p4/3P4/2NBBN2/PP3PPP/R2Q1RK1 w - - 0 1",
        "r2q1rk1/pp2bppp/2n1bn2/3p4/3P4/2NBBN2/PP3PPP/R2Q1RK1 b - - 0 1",
        "8/2k5/3r4/8/1n6/4Q3/5K2/6B1 w - - 0 1",
    ] {
        let mut board = Board::from_fen(fen).expect("the position parses");
        for &m in generate_legal(&board).as_slice() {
            let piece = board.piece_at(m.from_sq()).expect("a piece moves");
            let quiet = board.piece_at(m.to_sq()).is_none()
                && piece.piece_type() != PieceType::Pawn
                && !m.is_castle();
            let before = board.key();
            board.make_move(m);
            let change = before ^ board.key();
            board.unmake_move(m);
            let (low, high) = if m.from_sq().index() < m.to_sq().index() {
                (m.from_sq(), m.to_sq())
            } else {
                (m.to_sq(), m.from_sq())
            };
            let expected = quiet.then_some((piece, low, high));
            assert_eq!(cycles::find(change), expected, "{fen}: {m:?}");
        }
    }
}

#[test]
fn a_move_back_to_a_position_inside_the_search_is_found_before_it_is_made() {
    let mut board = Board::from_fen(START_FEN).expect("the position parses");
    made(&mut board, "g1f3 g8f6");
    assert!(
        !board.has_upcoming_repetition(),
        "no move returns to a position yet"
    );
    made(&mut board, "f3g1");
    assert!(board.has_upcoming_repetition(), "g8 returns to the root");
    assert!(repeats_after(&mut board, "f6g8"));
}

#[test]
fn a_position_the_game_reached_once_is_not_a_draw_to_return_to_and_one_it_reached_twice_is() {
    let mut once = played(START_FEN, "g1f3 g8f6 f3g1");
    assert!(
        !once.has_upcoming_repetition(),
        "the start would occur twice"
    );
    assert!(!repeats_after(&mut once, "f6g8"));

    let mut twice = played(START_FEN, "g1f3 g8f6 f3g1 f6g8 g1f3 g8f6 f3g1");
    assert!(
        twice.has_upcoming_repetition(),
        "the start would occur three times"
    );
    assert!(repeats_after(&mut twice, "f6g8"));
}

#[test]
fn a_move_whose_path_is_blocked_is_not_a_way_back() {
    // The rook goes round by b1 and b8 while the king walks round a square, so only a8 to a1
    // returns to the root, through the bishop where there is one.
    let line = "h6h7 a1b1 h7g7 b1b8 g7g6 b8a8 g6h6";
    let mut open = Board::from_fen("8/8/7k/8/8/8/8/R2BK3 b - - 0 1").expect("the position parses");
    made(&mut open, line);
    assert!(open.has_upcoming_repetition(), "a8 to a1 is open");
    assert!(repeats_after(&mut open, "a8a1"));

    let mut shut = Board::from_fen("8/8/7k/8/B7/8/8/R3K3 b - - 0 1").expect("the position parses");
    made(&mut shut, line);
    assert!(
        !shut.has_upcoming_repetition(),
        "the bishop on a4 is in the way"
    );
}
