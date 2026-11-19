// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's 32-byte record against the trainer's own, built by the pinned `bulletformat` from
//! the same position, score and result by both its constructors, compared byte for byte.

use bulletformat::{BulletFormat, ChessBoard};
use cadence_core::Colour;
use cadence_core::position::Board;
use cadence_dataset::trainer::{DRAW, LOSS, RECORD_LEN, WIN, encode};

const FENS: [&str; 6] = [
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1",
    "r1bq1rk1/pp2bppp/2n1pn2/3p4/2PP4/2N1PN2/PP3PPP/R2QKB1R w KQ - 0 9",
    "r1bq1rk1/pp2bppp/2n1pn2/3p4/2PP4/2N1PN2/PP3PPP/R2QKB1R b KQ - 0 9",
    "6k1/5ppp/8/8/8/8/PPP5/1K6 b - - 0 40",
    "8/2k5/8/1p6/8/5P2/6K1/8 w - - 0 50",
];

/// The trainer's record as it lies in a file, which its loader reads by layout.
fn bytes(board: ChessBoard) -> Vec<u8> {
    ChessBoard::as_bytes_slice(&[board]).to_vec()
}

/// Built from the text form, which takes the score and result as White's.
fn from_text(fen: &str, white_score: i16, white_result: u8) -> Vec<u8> {
    let wdl = ["0.0", "0.5", "1.0"][usize::from(white_result)];
    let text = format!("{fen} | {white_score} | {wdl}");
    bytes(text.parse().unwrap_or_else(|e| panic!("{text}: {e}")))
}

/// Built from bitboards in the order White, Black, then pawn to king.
fn from_bitboards(board: &Board, white_score: i16, white_result: u8) -> Vec<u8> {
    let mut bitboards = [0_u64; 8];
    for sq in board.occupied() {
        let piece = board
            .piece_at(sq)
            .expect("an occupied square holds a piece");
        let bit = 1_u64 << sq.index();
        bitboards[piece.colour().index()] |= bit;
        bitboards[2 + piece.piece_type().index()] |= bit;
    }
    let mover = board.side_to_move().index();
    let result = f32::from(white_result) / 2.0;
    let built = ChessBoard::from_raw(bitboards, mover, white_score, result);
    bytes(built.unwrap_or_else(|e| panic!("from bitboards: {e}")))
}

/// Every byte the engine writes is the trainer's, the king squares included, for either side to
/// move and every result.
#[test]
fn each_record_is_the_trainers_byte_for_byte() {
    let mut compared = 0;
    for fen in FENS {
        let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        let white = board.side_to_move() == Colour::White;
        for (score, result) in [(137, LOSS), (-52, DRAW), (0, WIN)] {
            let ours = encode(&board, score, result).to_vec();
            let (white_score, white_result) = if white {
                (score, result)
            } else {
                (-score, WIN - result)
            };
            for theirs in [
                from_text(fen, white_score, white_result),
                from_bitboards(&board, white_score, white_result),
            ] {
                assert_eq!(theirs.len(), RECORD_LEN);
                let differ: Vec<usize> =
                    (0..RECORD_LEN).filter(|&i| ours[i] != theirs[i]).collect();
                assert_eq!(
                    ours, theirs,
                    "{fen}, {score}, {result}: bytes {differ:?} differ"
                );
                compared += 1;
            }
        }
    }
    assert_eq!(compared, FENS.len() * 3 * 2);
}
