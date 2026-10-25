// SPDX-License-Identifier: GPL-3.0-or-later

//! One signed score per move given a move made earlier on the path, each keyed by its coloured
//! piece and destination; every offset reads the one table.

use cadence_core::position::Board;
use cadence_core::{Move, Piece};

use crate::history::{HISTORY_MAX, apply};

/// Twelve coloured pieces by sixty-four destinations: a frame's range and a move's column.
pub const PIECE_TO: usize = 12 * 64;

/// Plies back from the node. The colours alternate, so the two offsets never share an entry.
pub const OFFSETS: [usize; 2] = [1, 2];

/// The move made at a ply as its [`piece_to`], or `None` for a null move and a ply nothing wrote.
pub type Frame = Option<u16>;

/// Read before the move is made: a promotion's piece is the pawn, castling's the king.
///
/// # Panics
///
/// If the move's origin is empty.
#[must_use]
pub fn piece_to(board: &Board, m: Move) -> usize {
    let piece = board
        .piece_at(m.from_sq())
        .map_or_else(|| panic!("no piece on {}", m.from_sq()), Piece::index);
    piece * 64 + m.to_sq().index()
}

/// # Panics
///
/// If the move's origin is empty.
#[must_use]
pub fn frame(board: &Board, m: Move) -> Frame {
    Some(piece_to(board, m) as u16)
}

/// Every frame below `ply` was written on the current path, so only one above the root is absent.
#[must_use]
pub fn context(frames: &[Frame], ply: usize, offset: usize) -> Frame {
    if ply >= offset {
        frames[ply - offset]
    } else {
        None
    }
}

/// On the heap, at 1,179,648 bytes: `Search` is returned by value.
pub struct ContinuationHistory {
    rows: Box<[i16]>,
}

// The cap makes the narrowing in `update` lossless.
const _: () = assert!(HISTORY_MAX <= i16::MAX as i32);

impl ContinuationHistory {
    #[must_use]
    pub fn new() -> ContinuationHistory {
        ContinuationHistory {
            rows: vec![0; PIECE_TO * PIECE_TO].into_boxed_slice(),
        }
    }

    /// Called where the butterfly is cleared, so the tables share one lifetime.
    pub fn clear(&mut self) {
        self.rows.fill(0);
    }

    /// Indexed by a move's [`piece_to`].
    #[must_use]
    pub fn row(&self, context: u16) -> &[i16] {
        let start = usize::from(context) * PIECE_TO;
        &self.rows[start..start + PIECE_TO]
    }

    /// The butterfly's update, widened for [`apply`] and narrowed on store.
    pub fn update(&mut self, context: u16, piece_to: usize, bonus: i32) {
        let entry = &mut self.rows[usize::from(context) * PIECE_TO + piece_to];
        *entry = apply(i32::from(*entry), bonus) as i16;
    }
}

impl Default for ContinuationHistory {
    fn default() -> ContinuationHistory {
        ContinuationHistory::new()
    }
}
