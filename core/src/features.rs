// SPDX-License-Identifier: GPL-3.0-or-later

//! The train/play contract. Depends on `types` alone, deliberately.

use crate::types::{Colour, Piece, Square};

/// 2 colours x 6 piece types x 64 squares.
pub const NUM_INPUTS: usize = 768;

/// This ordering is the contract: a permutation passes forward-pass agreement and shows up only
/// as a net that trains well and plays badly.
#[inline]
#[must_use]
pub const fn feature_index(persp: Colour, piece: Piece, sq: Square) -> usize {
    let rel_colour = piece.colour().index() ^ persp.index();
    let flip = if (persp as u8) == 0 { 0 } else { 56 };
    let rel_sq = sq.index() ^ flip;
    rel_colour * 384 + piece.piece_type().index() * 64 + rel_sq
}

/// The same index with the perspective's board flipped left to right while its own king stands on
/// files e to h, so one set of weights serves both halves of the board. The trainer's mirrored
/// inputs flip on that half, and a net trained with them is read by this index alone.
#[inline]
#[must_use]
pub const fn mirrored_feature_index(
    persp: Colour,
    king: Square,
    piece: Piece,
    sq: Square,
) -> usize {
    let mirror = if king.file().index() > 3 { 7 } else { 0 };
    feature_index(persp, piece, sq) ^ mirror
}

// --- the pins ----------------------------------------------------------------
// Change one and every trained net stops loading correctly.
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
// Mirrored: a king on files a to d leaves the index as it is, and one on e to h flips the file.
const _: () =
    assert!(mirrored_feature_index(Colour::White, Square::D1, Piece::WPawn, Square::A1) == 0);
const _: () =
    assert!(mirrored_feature_index(Colour::White, Square::E1, Piece::WPawn, Square::A1) == 7);
const _: () =
    assert!(mirrored_feature_index(Colour::White, Square::E1, Piece::WKing, Square::E1) == 323);
const _: () =
    assert!(mirrored_feature_index(Colour::Black, Square::H8, Piece::BKing, Square::H8) == 320);
const _: () =
    assert!(mirrored_feature_index(Colour::Black, Square::A8, Piece::WPawn, Square::A8) == 384);
