// SPDX-License-Identifier: GPL-3.0-or-later

//! An en passant capture removes a pawn from a square the move never touches, so a check it
//! discovers through that square is the one `gives_check` must compute without making the move.

use cadence_core::position::Board;
use cadence_core::{FenStyle, Move, generate_legal};

/// Each side's capture opens its rook's rank to the enemy king, one for each colour's square.
const DISCOVERIES: [(&str, &str); 2] = [
    ("8/8/8/k2pP2R/8/8/8/4K3 w - d6 0 1", "e5d6"),
    ("4k3/8/8/8/r2pP2K/8/8/8 b - e3 0 1", "d4e3"),
];

fn capture(board: &Board, uci: &str) -> Move {
    generate_legal(board)
        .iter()
        .find(|m| m.to_uci_chess960() == uci)
        .unwrap_or_else(|| panic!("{uci} is not legal in {}", board.to_fen(FenStyle::Shredder)))
}

#[test]
fn an_en_passant_capture_that_opens_a_rank_gives_check() {
    for (fen, uci) in DISCOVERIES {
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert!(!board.in_check(), "{fen}: the side to move starts in check");
        let m = capture(&board, uci);
        assert!(m.is_en_passant(), "{fen}: {uci} is not en passant");
        assert!(board.gives_check(m), "{fen}: {uci} is not read as check");
        board.make_move(m);
        assert!(board.in_check(), "{fen}: {uci} does not check once made");
    }
}
