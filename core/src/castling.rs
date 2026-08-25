// SPDX-License-Identifier: GPL-3.0-or-later

//! Castling rights and castling geometry.
//!
//! Mutable rights are split from immutable layout. Rights are only ever
//! removed, never granted, and the rook origin a right refers to is fixed at
//! position setup, so the layout never enters the undo stack and is computed
//! once, from the parsed king and rook squares.

use core::mem::size_of;

use crate::attacks;
use crate::bitboard::Bitboard;
use crate::types::{Colour, File, OptSquare, Square};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum CastleSide {
    King = 0,
    Queen = 1,
}

impl CastleSide {
    pub const ALL: [CastleSide; 2] = [CastleSide::King, CastleSide::Queen];
}

/// The slot for `(c, s)`: WK = 0, WQ = 1, BK = 2, BQ = 3. This is the FEN
/// token order `KQkq`, so parsing is a left-to-right scan with no remapping,
/// and it indexes every per-right array in [`CastlingLayout`].
#[inline]
#[must_use]
pub const fn ci(c: Colour, s: CastleSide) -> usize {
    (c as usize) * 2 + (s as usize)
}

/// Four live bits, in slot order. Rights are only ever removed, never
/// granted, except by the FEN parser building the initial set.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash, Debug)]
#[repr(transparent)]
pub struct CastlingRights(u8);

impl CastlingRights {
    pub const NONE: CastlingRights = CastlingRights(0);
    pub const ALL: CastlingRights = CastlingRights(0b1111);

    /// The four low bits of `bits`, anything above them ignored.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u8) -> CastlingRights {
        CastlingRights(bits & 0b1111)
    }

    #[inline]
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// The single bit for `(c, s)`: `1 << ci(c, s)`.
    #[inline]
    #[must_use]
    pub const fn bit(c: Colour, s: CastleSide) -> u8 {
        1 << ci(c, s)
    }

    /// Both of `c`'s bits.
    #[inline]
    #[must_use]
    pub const fn both(c: Colour) -> u8 {
        0b0011 << (2 * c.index())
    }

    #[inline]
    #[must_use]
    pub const fn has(self, c: Colour, s: CastleSide) -> bool {
        self.0 & Self::bit(c, s) != 0
    }

    /// Whether `c` holds either right.
    #[inline]
    #[must_use]
    pub const fn any(self, c: Colour) -> bool {
        self.0 & Self::both(c) != 0
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `self & mask`. The one mutation: `make_move` applies
    /// `update_mask[from] & update_mask[to]` through it.
    #[inline]
    #[must_use]
    pub const fn masked(self, mask: u8) -> CastlingRights {
        CastlingRights(self.0 & mask)
    }

    /// `0..16`, the index into the Zobrist castling table.
    #[inline]
    #[must_use]
    pub const fn zobrist_index(self) -> usize {
        self.0 as usize
    }
}

/// Built once by `from_fen`; constant for the rest of the game; never in the
/// undo record. Every per-right array is indexed by [`ci`].
///
/// The classic 64-entry `const` mask table does not survive DFRC: it keys on
/// absolute squares (`a1` clears White queenside, `h8` clears Black kingside),
/// and under DFRC the rook files vary per game. The table is built at
/// position setup from the parsed rook squares instead. The one-line update
/// in `make_move` is unchanged; only the table's provenance changes.
#[derive(Clone, Copy, Debug)]
pub struct CastlingLayout {
    /// `rights = rights.masked(update_mask[from] & update_mask[to])`. One
    /// branchless line covering king moves, rook moves, rook captures and
    /// rook-takes-rook.
    pub update_mask: [u8; 64],
    pub king_from: [OptSquare; 2],
    pub rook_from: [OptSquare; 4],
    /// g-file / c-file on the king's rank.
    pub king_to: [OptSquare; 4],
    /// f-file / d-file on the king's rank.
    pub rook_to: [OptSquare; 4],
    /// Closed segment `[king_from, king_to]`, **inclusive of both endpoints**.
    /// Inclusivity folds "out of check" and "into check" into one loop.
    pub king_path: [Bitboard; 4],
    /// `(segment[kf, kt] | segment[rf, rt]) & !(kf | rf)`. `Bitboard::FULL`
    /// for absent rights, so any occupancy rejects.
    pub must_be_empty: [Bitboard; 4],
}

impl CastlingLayout {
    /// The layout for a position whose kings stand on `king_from` (per
    /// colour, `NONE` when that colour holds no right) and whose castling
    /// rooks stand on `rook_from` (per slot, `NONE` for an absent right).
    ///
    /// Destinations are fixed by the rules and never derived from direction:
    /// kingside → king g, rook f; queenside → king c, rook d, on the king's
    /// rank. Which slot is which is the caller's statement: a rook in the
    /// kingside slot on a lower file than the king is a caller error, not
    /// something this function reinterprets.
    #[must_use]
    pub fn new(king_from: [OptSquare; 2], rook_from: [OptSquare; 4]) -> CastlingLayout {
        let mut layout = CastlingLayout::none();
        layout.king_from = king_from;
        layout.rook_from = rook_from;
        for c in Colour::ALL {
            let Some(kf) = king_from[c.index()].get() else {
                continue;
            };
            let rank = kf.rank();
            for s in CastleSide::ALL {
                let i = ci(c, s);
                let Some(rf) = rook_from[i].get() else {
                    continue;
                };
                let (kt_file, rt_file) = match s {
                    CastleSide::King => (File::G, File::F),
                    CastleSide::Queen => (File::C, File::D),
                };
                let kt = Square::from_file_rank(kt_file, rank);
                let rt = Square::from_file_rank(rt_file, rank);
                layout.king_to[i] = OptSquare::some(kt);
                layout.rook_to[i] = OptSquare::some(rt);
                layout.king_path[i] = segment(kf, kt);
                layout.must_be_empty[i] =
                    (segment(kf, kt) | segment(rf, rt)) & !(kf.bb() | rf.bb());
                // `&=`, never `=`: a malformed layout degrades toward fewer
                // rights rather than more.
                layout.update_mask[kf.index()] &= !CastlingRights::both(c);
                layout.update_mask[rf.index()] &= !CastlingRights::bit(c, s);
            }
        }
        layout
    }

    /// No rights at all: every mask keeps everything, every path is empty,
    /// every `must_be_empty` is `FULL`.
    #[must_use]
    pub fn none() -> CastlingLayout {
        CastlingLayout {
            update_mask: [0b1111; 64],
            king_from: [OptSquare::NONE; 2],
            rook_from: [OptSquare::NONE; 4],
            king_to: [OptSquare::NONE; 4],
            rook_to: [OptSquare::NONE; 4],
            king_path: [Bitboard::EMPTY; 4],
            must_be_empty: [Bitboard::FULL; 4],
        }
    }
}

/// The closed segment between two squares on one rank, both ends included.
fn segment(a: Square, b: Square) -> Bitboard {
    debug_assert_eq!(a.rank(), b.rank());
    attacks::between(a, b) | a.bb() | b.bb()
}

const _: () = assert!(size_of::<CastlingRights>() == 1);
const _: () = assert!(size_of::<CastleSide>() == 1);
const _: () = assert!(size_of::<CastlingLayout>() == 144);
