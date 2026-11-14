// SPDX-License-Identifier: GPL-3.0-or-later

//! The 32-byte position record the trainer reads, from the side to move's view: occupancy, a nibble
//! a piece, the score, the result and the two king squares. Written before a trainer is chosen, so a
//! different one changes this module alone.

use cadence_core::Colour;
use cadence_core::position::Board;
use cadence_core::types::Square;

pub const RECORD_LEN: usize = 32;

/// The side to move's result.
pub const LOSS: u8 = 0;
pub const DRAW: u8 = 1;
pub const WIN: u8 = 2;

/// Black to move is seen flipped, its pieces as the mover's, so every record reads as White to
/// move; `score` and `result` are already the side to move's.
///
/// # Panics
///
/// If an occupied square holds no piece, which a board never allows.
#[must_use]
pub fn encode(board: &Board, score: i16, result: u8) -> [u8; RECORD_LEN] {
    let mover = board.side_to_move();
    let seen = |sq: Square| -> usize {
        if mover == Colour::White {
            sq.index()
        } else {
            sq.flip_vertical().index()
        }
    };
    let mut occupancy = 0_u64;
    let mut pieces = [0_u8; 64];
    for sq in board.occupied() {
        let piece = board
            .piece_at(sq)
            .expect("an occupied square holds a piece");
        let theirs = u8::from(piece.colour() != mover);
        let at = seen(sq);
        occupancy |= 1 << at;
        pieces[at] = piece.piece_type() as u8 | theirs << 3;
    }
    let mut out = [0_u8; RECORD_LEN];
    out[..8].copy_from_slice(&occupancy.to_le_bytes());
    let mut n = 0;
    for (at, &piece) in pieces.iter().enumerate() {
        if occupancy >> at & 1 == 1 {
            out[8 + n / 2] |= piece << (4 * (n % 2));
            n += 1;
        }
    }
    out[24..26].copy_from_slice(&score.to_le_bytes());
    out[26] = result;
    out[27] = seen(board.king_square(mover)) as u8;
    out[28] = seen(board.king_square(mover.flip())) as u8;
    out
}

#[cfg(test)]
mod tests {
    use cadence_core::Colour;
    use cadence_core::position::Board;

    use super::{LOSS, RECORD_LEN, WIN, encode};

    /// Read from the layout alone: each occupied square in order, a nibble each.
    fn decode(r: &[u8; RECORD_LEN]) -> (Vec<(usize, u8)>, i16, u8, usize, usize) {
        let mut occupancy = [0; 8];
        occupancy.copy_from_slice(&r[..8]);
        let occupancy = u64::from_le_bytes(occupancy);
        let mut pieces = Vec::new();
        for sq in 0..64 {
            if occupancy >> sq & 1 == 1 {
                let n = pieces.len();
                pieces.push((sq, (r[8 + n / 2] >> (4 * (n % 2))) & 0xf));
            }
        }
        let score = i16::from_le_bytes([r[24], r[25]]);
        (pieces, score, r[26], usize::from(r[27]), usize::from(r[28]))
    }

    /// The board's own pieces, as square, whether White's, and type.
    fn pieces_of(board: &Board) -> Vec<(usize, bool, u8)> {
        let mut want: Vec<(usize, bool, u8)> = board
            .occupied()
            .into_iter()
            .map(|sq| {
                let p = board.piece_at(sq).expect("a piece");
                (
                    sq.index(),
                    p.colour() == Colour::White,
                    p.piece_type() as u8,
                )
            })
            .collect();
        want.sort_unstable();
        want
    }

    /// A Black-to-move record, flipped back and its colours swapped, is the board read White-relative,
    /// with the score negated and the result turned to White's.
    #[test]
    fn a_black_to_move_record_reads_back_as_the_board() {
        let board = Board::from_fen("r3k2r/ppp2ppp/2n5/3qp3/3P4/2N2N2/PPP2PPP/R2QK2R b KQkq - 0 9")
            .expect("a position");
        let (pieces, score, result, ksq, opp) = decode(&encode(&board, -37, WIN));
        let mut seen: Vec<(usize, bool, u8)> = pieces
            .iter()
            .map(|&(sq, nibble)| (sq ^ 0b11_1000, nibble >> 3 == 1, nibble & 7))
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, pieces_of(&board));
        assert_eq!(-score, 37, "the score White-relative");
        assert_eq!(result, WIN, "Black to move and winning");
        assert_eq!(ksq ^ 0b11_1000, board.king_square(Colour::Black).index());
        assert_eq!(opp ^ 0b11_1000, board.king_square(Colour::White).index());
    }

    #[test]
    fn a_white_to_move_record_is_not_flipped() {
        let board = Board::from_fen(cadence_core::START_FEN).expect("the start");
        let (pieces, score, result, ksq, opp) = decode(&encode(&board, 12, LOSS));
        let mut seen: Vec<(usize, bool, u8)> = pieces
            .iter()
            .map(|&(sq, nibble)| (sq, nibble >> 3 == 0, nibble & 7))
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, pieces_of(&board));
        assert_eq!((score, result, ksq, opp), (12, LOSS, 4, 60));
    }
}
