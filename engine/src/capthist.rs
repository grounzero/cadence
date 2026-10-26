// SPDX-License-Identifier: GPL-3.0-or-later

//! One signed score per capture, keyed by its coloured piece, its destination and the captured
//! piece's type; a promotion that captures nothing has no entry.

use cadence_core::position::Board;
use cadence_core::{Move, Piece, PieceType};

use crate::conthist::{self, PIECE_TO};
use crate::history::{HISTORY_MAX, apply};

/// The captured piece's type, six columns with the king's unreachable.
pub const VICTIMS: usize = 6;

pub const ENTRIES: usize = PIECE_TO * VICTIMS;

/// The entry a capture reads and writes, or `None` for a move that captures nothing. En passant's
/// victim is a pawn, and a promotion's mover is its pawn.
///
/// # Panics
///
/// If a capture's squares do not hold pieces.
#[must_use]
pub fn index(board: &Board, m: Move) -> Option<usize> {
    let victim = if m.is_en_passant() {
        PieceType::Pawn
    } else if m.is_capture() {
        board
            .piece_at(m.to_sq())
            .map_or_else(|| panic!("no piece on {}", m.to_sq()), Piece::piece_type)
    } else {
        return None;
    };
    Some(conthist::piece_to(board, m) * VICTIMS + victim.index())
}

/// On the heap, at 9,216 bytes: `Search` is returned by value.
pub struct CaptureHistory {
    entries: Box<[i16]>,
}

// The cap makes the narrowing in `update` lossless.
const _: () = assert!(HISTORY_MAX <= i16::MAX as i32);

impl CaptureHistory {
    #[must_use]
    pub fn new() -> CaptureHistory {
        CaptureHistory {
            entries: vec![0; ENTRIES].into_boxed_slice(),
        }
    }

    /// Called where the butterfly is cleared, so the tables share one lifetime.
    pub fn clear(&mut self) {
        self.entries.fill(0);
    }

    /// Indexed by [`index`].
    #[must_use]
    pub fn entries(&self) -> &[i16] {
        &self.entries
    }

    /// The butterfly's update, widened for [`apply`] and narrowed on store.
    pub fn update(&mut self, index: usize, bonus: i32) {
        let entry = &mut self.entries[index];
        *entry = apply(i32::from(*entry), bonus) as i16;
    }
}

impl Default for CaptureHistory {
    fn default() -> CaptureHistory {
        CaptureHistory::new()
    }
}
