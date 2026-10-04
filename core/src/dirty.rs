// SPDX-License-Identifier: GPL-3.0-or-later

use crate::types::{OptSquare, Piece, Square};
use core::mem::{align_of, size_of};

pub const MAX_DIRTY: usize = 4;

/// Moving piece, capture, castling rook; the fourth slot is headroom.
pub const MAX_DIRTY_REACHABLE: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirtyPiece {
    pub piece: Piece,
    pub from: OptSquare,
    pub to: OptSquare,
}

impl DirtyPiece {
    #[inline]
    #[must_use]
    pub const fn moved(piece: Piece, from: Square, to: Square) -> DirtyPiece {
        DirtyPiece {
            piece,
            from: OptSquare::some(from),
            to: OptSquare::some(to),
        }
    }

    /// A capture victim, or a pawn that promoted.
    #[inline]
    #[must_use]
    pub const fn removed(piece: Piece, from: Square) -> DirtyPiece {
        DirtyPiece {
            piece,
            from: OptSquare::some(from),
            to: OptSquare::NONE,
        }
    }

    /// The promoted piece.
    #[inline]
    #[must_use]
    pub const fn added(piece: Piece, to: Square) -> DirtyPiece {
        DirtyPiece {
            piece,
            from: OptSquare::NONE,
            to: OptSquare::some(to),
        }
    }
}

/// Every `from` subtraction before any `to` addition: in DFRC castling one square can be both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirtyPieces {
    entries: [DirtyPiece; MAX_DIRTY],
    len: u8,
}

impl DirtyPieces {
    pub const EMPTY: DirtyPieces = DirtyPieces {
        entries: [DirtyPiece {
            piece: Piece::WPawn,
            from: OptSquare::NONE,
            to: OptSquare::NONE,
        }; MAX_DIRTY],
        len: 0,
    };

    /// # Panics
    ///
    /// If the delta already holds `MAX_DIRTY` entries; bounds-checked so it names the line rather
    /// than overwriting.
    #[inline]
    pub fn push(&mut self, entry: DirtyPiece) {
        self.entries[usize::from(self.len)] = entry;
        self.len += 1;
    }

    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Emission order, which is not application order.
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[DirtyPiece] {
        &self.entries[..usize::from(self.len)]
    }
}

// --- layout guards --------------------------------------------------------
// At most 16 bytes, so both ABIs return it in registers from `make_move`.
const _: () = assert!(size_of::<DirtyPiece>() == 3);
const _: () = assert!(align_of::<DirtyPiece>() == 1);
const _: () = assert!(size_of::<DirtyPieces>() == 13);
const _: () = assert!(align_of::<DirtyPieces>() == 1);
const _: () = assert!(size_of::<DirtyPieces>() <= 16);
const _: () = assert!(MAX_DIRTY_REACHABLE <= MAX_DIRTY);
