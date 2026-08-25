// SPDX-License-Identifier: GPL-3.0-or-later

//! The accumulator delta produced by `make_move`.

use crate::types::{OptSquare, Piece, Square};
use core::mem::{align_of, size_of};

/// Capacity for every piece change one legal move can produce.
pub const MAX_DIRTY: usize = 4;

/// The reachable maximum: moving piece, capture, castling rook. The extra
/// slot is headroom, and is what lazy updates would need once they coalesce
/// plies. Asserted by the move generator's tests, not by the type.
pub const MAX_DIRTY_REACHABLE: usize = 3;

/// One piece's movement, in the form the accumulator consumes.
///
/// | `from`  | `to`    | Meaning                 |
/// |---------|---------|-------------------------|
/// | `Some`  | `Some`  | moved                   |
/// | `Some`  | `None`  | left the board          |
/// | `None`  | `Some`  | appeared (promotion)    |
/// | `None`  | `None`  | never emitted           |
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirtyPiece {
    pub piece: Piece,
    pub from: OptSquare,
    pub to: OptSquare,
}

impl DirtyPiece {
    /// A piece that moved from `from` to `to`.
    #[inline]
    #[must_use]
    pub const fn moved(piece: Piece, from: Square, to: Square) -> DirtyPiece {
        DirtyPiece {
            piece,
            from: OptSquare::some(from),
            to: OptSquare::some(to),
        }
    }

    /// A piece that left the board: a capture victim, or a pawn that promoted.
    #[inline]
    #[must_use]
    pub const fn removed(piece: Piece, from: Square) -> DirtyPiece {
        DirtyPiece {
            piece,
            from: OptSquare::some(from),
            to: OptSquare::NONE,
        }
    }

    /// A piece that appeared: the promoted piece.
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

/// **Ordering contract: all `from` subtractions, then all `to` additions.**
///
/// In DFRC castling a square can be a `to` in one entry and a `from` in
/// another: king and rook swapping squares, or the king landing on the
/// rook's origin. Within each pass the order is free; across the two it is
/// not. Applying the delta in emission order without that split takes an
/// occupancy count to `2` or `-1`.
///
/// `push` performs a bounds check. It does not mask the index. Masking an
/// out-of-range write silently corrupts a neighbouring entry and surfaces
/// somewhere else entirely; a bounds check panics and names the line. The
/// capacity being a power of two is not a reason to mask it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DirtyPieces {
    entries: [DirtyPiece; MAX_DIRTY],
    len: u8,
}

impl DirtyPieces {
    /// No entries. What a null move returns, and what `make_move` starts
    /// from.
    pub const EMPTY: DirtyPieces = DirtyPieces {
        entries: [DirtyPiece {
            piece: Piece::WPawn,
            from: OptSquare::NONE,
            to: OptSquare::NONE,
        }; MAX_DIRTY],
        len: 0,
    };

    /// Append an entry. Bounds-checked, not masked: a fifth entry panics and
    /// names this line rather than overwriting the first.
    ///
    /// # Panics
    ///
    /// If the delta already holds `MAX_DIRTY` entries.
    #[inline]
    pub fn push(&mut self, entry: DirtyPiece) {
        // Indexing checks the bound; the panic names this line.
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

    /// The populated entries, in emission order.
    ///
    /// Emission order is **not** application order: every `from` subtraction
    /// must be applied before any `to` addition. See the ordering contract
    /// above.
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[DirtyPiece] {
        &self.entries[..usize::from(self.len)]
    }
}

// --- layout guards --------------------------------------------------------
//
// 13 bytes, alignment 1. The threshold that matters is 16: at or below it
// both AAPCS64 and SysV return the aggregate in registers rather than through
// memory, and `make_move` returns one of these at every node. A two-byte
// `Option<Square>` in `DirtyPiece` would take these to 5 and 21 and push the
// return through memory, which is the whole reason `OptSquare` exists.
const _: () = assert!(size_of::<DirtyPiece>() == 3);
const _: () = assert!(align_of::<DirtyPiece>() == 1);
const _: () = assert!(size_of::<DirtyPieces>() == 13);
const _: () = assert!(align_of::<DirtyPieces>() == 1);
const _: () = assert!(size_of::<DirtyPieces>() <= 16);
const _: () = assert!(MAX_DIRTY_REACHABLE <= MAX_DIRTY);
