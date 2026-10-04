// SPDX-License-Identifier: GPL-3.0-or-later

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

/// FEN token order, `KQkq`, so parsing needs no remapping.
#[inline]
#[must_use]
pub const fn ci(c: Colour, s: CastleSide) -> usize {
    (c as usize) * 2 + (s as usize)
}

/// Only ever removed, except by the FEN parser.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash, Debug)]
#[repr(transparent)]
pub struct CastlingRights(u8);

impl CastlingRights {
    pub const NONE: CastlingRights = CastlingRights(0);
    pub const ALL: CastlingRights = CastlingRights(0b1111);

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

    #[inline]
    #[must_use]
    pub const fn bit(c: Colour, s: CastleSide) -> u8 {
        1 << ci(c, s)
    }

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

    /// The one mutation.
    #[inline]
    #[must_use]
    pub const fn masked(self, mask: u8) -> CastlingRights {
        CastlingRights(self.0 & mask)
    }

    #[inline]
    #[must_use]
    pub const fn zobrist_index(self) -> usize {
        self.0 as usize
    }
}

/// Constant for the game, so never in the undo record.
#[derive(Clone, Copy, Debug)]
pub struct CastlingLayout {
    /// One branchless update covers king moves, rook moves and rook captures.
    pub update_mask: [u8; 64],
    pub king_from: [OptSquare; 2],
    pub rook_from: [OptSquare; 4],
    pub king_to: [OptSquare; 4],
    pub rook_to: [OptSquare; 4],
    /// Both ends included, so out of check and into check are one loop.
    pub king_path: [Bitboard; 4],
    /// `FULL` for an absent right, so any occupancy rejects it.
    pub must_be_empty: [Bitboard; 4],
}

impl CastlingLayout {
    /// Destinations come from the rules, never from direction.
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
                // `&=`, never `=`: a malformed layout degrades toward fewer rights rather than
                // more.
                layout.update_mask[kf.index()] &= !CastlingRights::both(c);
                layout.update_mask[rf.index()] &= !CastlingRights::bit(c, s);
            }
        }
        layout
    }

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

/// Both ends included.
fn segment(a: Square, b: Square) -> Bitboard {
    debug_assert_eq!(a.rank(), b.rank());
    attacks::between(a, b) | a.bb() | b.bb()
}

const _: () = assert!(size_of::<CastlingRights>() == 1);
const _: () = assert!(size_of::<CastleSide>() == 1);
const _: () = assert!(size_of::<CastlingLayout>() == 144);
