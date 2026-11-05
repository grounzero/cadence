// SPDX-License-Identifier: GPL-3.0-or-later

//! In check, an en passant capture is legal exactly when nothing attacks the king once both pawns
//! have moved: it takes the checker, or it lands between a slider and the king, which only a FEN
//! can set up.

mod support;

use cadence_core::position::Board;
use cadence_core::{FenStyle, Move, generate_legal};
use support::naive;

/// Black to move and in check, with the capture `e4d3` available and whether it is legal.
const CASES: [(&str, &str, bool); 5] = [
    // The checker is the pawn the capture takes.
    ("8/8/8/4k3/3Pp3/8/8/4K3 b - d3 0 1", "the pushed pawn", true),
    // The capture lands on d3, between the bishop and the king.
    (
        "8/8/k7/8/3Pp3/8/8/4KB2 b - d3 0 1",
        "a bishop through d3",
        true,
    ),
    (
        "8/8/8/8/3Pp3/k6R/8/4K3 b - d3 0 1",
        "a rook through d3",
        true,
    ),
    (
        "R7/8/8/8/3Pp3/k7/8/4K3 b - d3 0 1",
        "a rook on the file",
        false,
    ),
    ("8/8/8/8/3Pp3/1k6/8/N3K3 b - d3 0 1", "a knight", false),
];

#[test]
fn a_capture_in_check_is_generated_exactly_when_it_leaves_the_king_safe() {
    for (fen, checker, legal) in CASES {
        let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert!(board.in_check(), "{fen}: not in check");
        let capture = |moves: &[Move]| {
            moves
                .iter()
                .any(|m| m.is_en_passant() && m.to_uci_chess960() == "e4d3")
        };
        let fast = capture(generate_legal(&board).as_slice());
        let slow = capture(naive::legal(&board).as_slice());
        assert_eq!(
            slow,
            legal,
            "the oracle, checked by {checker}: {}",
            board.to_fen(FenStyle::Shredder)
        );
        assert_eq!(fast, legal, "checked by {checker}: {fen}");
    }
}
