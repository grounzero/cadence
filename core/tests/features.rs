// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `features`: the train/play contract, stated as data.
//!
//! `feature_index` is the single definition inference and the training-data
//! writer both consume. Its ordering is frozen by compile-time pins the
//! moment it exists, and checked against Bullet's `Chess768` as integers
//! before any training run. What can be checked here, with no trainer, is
//! that it computes exactly this ordering:
//!
//! ```text
//! rel_colour = piece.colour ^ perspective       (0 = us)
//! rel_sq     = sq ^ (perspective == Black ? 56 : 0)
//! index      = rel_colour * 384 + piece_type * 64 + rel_sq
//! ```
//!
//! and that the ordering has the properties the network relies on: each
//! perspective is a bijection onto `0..768`, and Black's view of a position
//! is White's view of the colour-swapped, vertically mirrored one.

use cadence_core::types::{Colour, Piece, PieceType, Square};
use cadence_core::{NUM_INPUTS, feature_index};

fn stated(persp: Colour, piece: Piece, sq: Square) -> usize {
    let rel_colour = piece.colour().index() ^ persp.index();
    let flip = if persp == Colour::Black { 56 } else { 0 };
    let rel_sq = sq.index() ^ flip;
    rel_colour * 384 + piece.piece_type().index() * 64 + rel_sq
}

#[test]
fn feature_index_is_the_stated_ordering_for_all_1536_inputs() {
    for persp in Colour::ALL {
        for piece in Piece::ALL {
            for sq in Square::all() {
                let got = feature_index(persp, piece, sq);
                assert_eq!(got, stated(persp, piece, sq), "{persp:?} {piece:?} {sq}");
                assert!(got < NUM_INPUTS);
            }
        }
    }
    assert_eq!(NUM_INPUTS, 768);
}

#[test]
fn each_perspective_is_a_bijection_onto_the_input_range() {
    for persp in Colour::ALL {
        let mut seen = vec![false; NUM_INPUTS];
        for piece in Piece::ALL {
            for sq in Square::all() {
                let i = feature_index(persp, piece, sq);
                assert!(!seen[i], "{persp:?}: index {i} used twice ({piece:?} {sq})");
                seen[i] = true;
            }
        }
        assert!(seen.iter().all(|s| *s), "{persp:?}: an input is never used");
    }
}

/// Black's view of `(piece, sq)` is White's view of the other-colour piece
/// on the mirrored square. This is the perspective flip in one line.
#[test]
fn black_perspective_is_the_colour_swapped_mirror_of_white() {
    for piece in Piece::ALL {
        let swapped = Piece::new(piece.colour().flip(), piece.piece_type());
        for sq in Square::all() {
            assert_eq!(
                feature_index(Colour::Black, piece, sq),
                feature_index(Colour::White, swapped, sq.flip_vertical()),
                "{piece:?} {sq}"
            );
        }
    }
}

/// The corners of the ordering by name: the same values the crate pins at
/// compile time, restated so a report names them.
#[test]
fn named_pins() {
    assert_eq!(feature_index(Colour::White, Piece::WPawn, Square::A1), 0);
    assert_eq!(feature_index(Colour::White, Piece::WPawn, Square::H8), 63);
    assert_eq!(feature_index(Colour::White, Piece::WKnight, Square::A1), 64);
    assert_eq!(feature_index(Colour::White, Piece::WKing, Square::H8), 383);
    assert_eq!(feature_index(Colour::White, Piece::BPawn, Square::A1), 384);
    assert_eq!(feature_index(Colour::White, Piece::BKing, Square::H8), 767);
    // Black's own pawn on its relative a1 (the a8 square) is input 0.
    assert_eq!(feature_index(Colour::Black, Piece::BPawn, Square::A8), 0);
    assert_eq!(feature_index(Colour::Black, Piece::BKing, Square::H1), 383);
    assert_eq!(feature_index(Colour::Black, Piece::WPawn, Square::A8), 384);
    assert_eq!(
        feature_index(Colour::Black, Piece::WKing, Square::E1),
        384 + 5 * 64 + 60
    );
    // A piece type block is 64 wide, colour block 384.
    for pt in PieceType::ALL {
        assert_eq!(
            feature_index(Colour::White, Piece::new(Colour::White, pt), Square::A1),
            pt.index() * 64
        );
    }
}
