// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's feature index against the trainer's, for every perspective, colour, piece and
//! square, compared as integers: a permutation trains a net that loads and plays badly.

use bullet_lib::game::inputs::{Chess768, SparseInputType};
use bulletformat::ChessBoard;
use cadence_core::features::{NUM_INPUTS, feature_index};
use cadence_core::{Colour, Piece, PieceType, Square};

/// The one piece, stored as the trainer stores a position: from the side to move's view.
fn record(piece: Piece, sq: Square, mover: Colour) -> ChessBoard {
    let mut bitboards = [0_u64; 8];
    let bit = 1_u64 << sq.index();
    bitboards[piece.colour().index()] |= bit;
    bitboards[2 + piece.piece_type().index()] |= bit;
    ChessBoard::from_raw(bitboards, mover.index(), 0, 0.5).expect("a one-piece record")
}

/// Each record gives the trainer's index from the mover's view and the other side's, which are
/// the engine's two perspectives.
#[test]
fn every_feature_index_is_the_trainers() {
    assert_eq!(Chess768.num_inputs(), NUM_INPUTS);
    let mut compared = 0;
    for mover in [Colour::White, Colour::Black] {
        for colour in [Colour::White, Colour::Black] {
            for kind in PieceType::ALL {
                for index in 0..64 {
                    let (piece, sq) = (Piece::new(colour, kind), Square::new(index));
                    let mut trainer = Vec::new();
                    Chess768.map_features(&record(piece, sq, mover), |stm, ntm| {
                        trainer.push((stm, ntm));
                    });
                    let engine = (
                        feature_index(mover, piece, sq),
                        feature_index(mover.flip(), piece, sq),
                    );
                    assert_eq!(trainer, [engine], "{piece:?} on {sq:?}, {mover:?} to move");
                    compared += 2;
                }
            }
        }
    }
    assert_eq!(compared, 2 * 2 * 6 * 2 * 64);
}
