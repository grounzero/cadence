// SPDX-License-Identifier: GPL-3.0-or-later

//! NNUE feature indexing: the train/play contract.
//!
//! This module depends on `types` and nothing else, deliberately. It is the
//! one definition in the crate that is a contract with an external system, and
//! it must not be able to drift with the board representation.

use crate::types::{Colour, Piece, Square};

/// 2 colours x 6 piece types x 64 squares.
pub const NUM_INPUTS: usize = 768;

/// Perspective-flipped index into the input layer.
///
/// This ordering **is** the train/play contract: inference and the training
/// data writer both consume it, so a permutation here is invisible to
/// forward-pass agreement and shows up only as a net that trains well and
/// plays badly. The ordering is frozen by the `const` pins below, and checked
/// against Bullet's own indexing as integers before any training run.
///
/// ```text
/// rel_colour = piece.colour ^ perspective       (0 = us)
/// rel_sq     = sq ^ (perspective == Black ? 56 : 0)
/// index      = rel_colour * 384 + piece_type * 64 + rel_sq
/// ```
///
/// Required to be `const fn`, because the pins are
/// `const _: () = assert!(feature_index(..) == ..)`. That constrains how it
/// may be written: compare discriminants (`persp as u8 == 0`), never a derived
/// `PartialEq`, which is not const-callable (E0015).
#[inline]
#[must_use]
pub const fn feature_index(persp: Colour, piece: Piece, sq: Square) -> usize {
    let rel_colour = piece.colour().index() ^ persp.index();
    let flip = if (persp as u8) == 0 { 0 } else { 56 };
    let rel_sq = sq.index() ^ flip;
    rel_colour * 384 + piece.piece_type().index() * 64 + rel_sq
}

// --- the pins ----------------------------------------------------------------
//
// The corners of the ordering, frozen at compile time so the discriminants of
// Colour, PieceType, Piece and Square cannot drift and need reordering to
// match Bullet after the first net is trained. Every one of these is a claim
// about the contract, not about the implementation; change one and the nets
// already trained stop loading correctly.
const _: () = assert!(feature_index(Colour::White, Piece::WPawn, Square::A1) == 0);
const _: () = assert!(feature_index(Colour::White, Piece::WPawn, Square::H8) == 63);
const _: () = assert!(feature_index(Colour::White, Piece::WKnight, Square::A1) == 64);
const _: () = assert!(feature_index(Colour::White, Piece::WBishop, Square::A1) == 128);
const _: () = assert!(feature_index(Colour::White, Piece::WRook, Square::A1) == 192);
const _: () = assert!(feature_index(Colour::White, Piece::WQueen, Square::A1) == 256);
const _: () = assert!(feature_index(Colour::White, Piece::WKing, Square::A1) == 320);
const _: () = assert!(feature_index(Colour::White, Piece::WKing, Square::H8) == 383);
const _: () = assert!(feature_index(Colour::White, Piece::BPawn, Square::A1) == 384);
const _: () = assert!(feature_index(Colour::White, Piece::BKing, Square::H8) == 767);
// Black's perspective: its own pieces come first, squares mirrored.
const _: () = assert!(feature_index(Colour::Black, Piece::BPawn, Square::A8) == 0);
const _: () = assert!(feature_index(Colour::Black, Piece::BPawn, Square::H1) == 63);
const _: () = assert!(feature_index(Colour::Black, Piece::BKing, Square::H1) == 383);
const _: () = assert!(feature_index(Colour::Black, Piece::WPawn, Square::A8) == 384);
const _: () = assert!(feature_index(Colour::Black, Piece::WKing, Square::E1) == 384 + 320 + 60);
const _: () = assert!(feature_index(Colour::Black, Piece::WKing, Square::H1) == 767);
