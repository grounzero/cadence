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
    let mirror = if mirrors(king) { 7 } else { 0 };
    feature_index(persp, piece, sq) ^ mirror
}

/// Whether a king on this square has its perspective's board flipped: the half of files e to h.
#[inline]
#[must_use]
pub const fn mirrors(king: Square) -> bool {
    king.file().index() > 3
}

/// How many buckets a king-bucketed net splits its first layer into, each a full set of
/// `NUM_INPUTS` rows.
pub const KING_BUCKETS: usize = 4;

/// The bucket of a king on files a to d, a rank to a row from its own back rank: the back-rank
/// corner, the back-rank centre, the second rank, and everything beyond. The trainer is handed
/// this table as it stands, so the two sides cannot hold different ones.
#[rustfmt::skip]
pub const KING_BUCKET: [usize; 32] = [
    0, 0, 1, 1,
    2, 2, 2, 2,
    3, 3, 3, 3,
    3, 3, 3, 3,
    3, 3, 3, 3,
    3, 3, 3, 3,
    3, 3, 3, 3,
    3, 3, 3, 3,
];

/// The bucket of this perspective's own king, read as the mirrored index reads it: the rank from
/// its own back rank and the file folded onto a to d. A king that changes bucket changes every row
/// its perspective reads, as one that changes half does.
#[inline]
#[must_use]
pub const fn king_bucket(persp: Colour, king: Square) -> usize {
    let file = king.file().index();
    let folded = if file > 3 { 7 - file } else { file };
    KING_BUCKET[king.rank().relative(persp).index() * 4 + folded]
}

/// The mirrored index inside the block of rows its perspective's king bucket owns. A net trained
/// with king buckets over mirrored inputs is read by this index alone.
#[inline]
#[must_use]
pub const fn bucketed_feature_index(
    persp: Colour,
    king: Square,
    piece: Piece,
    sq: Square,
) -> usize {
    king_bucket(persp, king) * NUM_INPUTS + mirrored_feature_index(persp, king, piece, sq)
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
// King buckets: the back rank splits between the b and c files after mirroring, and each
// perspective counts ranks from its own side.
const _: () = assert!(king_bucket(Colour::White, Square::B1) == 0);
const _: () = assert!(king_bucket(Colour::White, Square::C1) == 1);
const _: () = assert!(king_bucket(Colour::White, Square::E1) == 1);
const _: () = assert!(king_bucket(Colour::White, Square::G1) == 0);
const _: () = assert!(king_bucket(Colour::White, Square::H2) == 2);
const _: () = assert!(king_bucket(Colour::White, Square::E8) == 3);
const _: () = assert!(king_bucket(Colour::Black, Square::E8) == 1);
const _: () = assert!(king_bucket(Colour::Black, Square::G8) == 0);
const _: () = assert!(king_bucket(Colour::Black, Square::A7) == 2);
const _: () = assert!(king_bucket(Colour::Black, Square::E1) == 3);
const _: () =
    assert!(bucketed_feature_index(Colour::White, Square::G1, Piece::WPawn, Square::A1) == 7);
const _: () =
    assert!(bucketed_feature_index(Colour::White, Square::E1, Piece::WPawn, Square::A1) == 775);
const _: () =
    assert!(bucketed_feature_index(Colour::White, Square::A3, Piece::WPawn, Square::A1) == 2304);
const _: () =
    assert!(bucketed_feature_index(Colour::Black, Square::G8, Piece::BKing, Square::G8) == 321);
const _: () =
    assert!(bucketed_feature_index(Colour::Black, Square::E7, Piece::WPawn, Square::A8) == 1927);
