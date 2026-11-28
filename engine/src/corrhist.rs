// SPDX-License-Identifier: GPL-3.0-or-later

//! A static evaluation is wrong in ways that repeat, so the difference is held per pawn structure,
//! per each side's other pieces, per side to move and per pair of moves that reached the position,
//! and offered back to any rule comparing a static score against a bound.

use cadence_core::position::Board;
use cadence_core::{Colour, PieceType};

use crate::conthist::{self, Frame, PIECE_TO};
use crate::score::{self, Score};

/// Enough that a game's pawn keys rarely collide, small enough to sit in cache beside the history.
const TABLE_SLOTS: usize = 1 << 14;

/// So the weighted mean does not round a small persistent correction away.
pub const GRAIN: i32 = 256;

/// The weighted mean's denominator. An observation moves an entry at most `MAX_WEIGHT /
/// WEIGHT_UNIT` of the distance to itself.
const WEIGHT_UNIT: i32 = 256;
pub const MAX_WEIGHT: i32 = 16;

/// Keeps a corrected evaluation off the decided scale, below the mates and the tablebase band. It
/// bounds each table and their sum.
pub const MAX_CORRECTION: Score = 256;
const _: () = assert!(score::MAX_EVAL + MAX_CORRECTION < score::TB_WIN_IN_MAX_PLY);

/// A larger disagreement is a tactic, not an evaluation error, and would dominate the mean.
pub const MAX_DELTA: Score = 1024;

/// The pawn table, then one for each side's other pieces, each with a half per side to move.
const TABLES: usize = 3;

/// The most one table's slot holds, which an update clamps it to.
const LIMIT: i32 = MAX_CORRECTION * GRAIN;

/// What a position reads the tables by, taken once per node.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CorrectionKeys {
    pub pawn: u64,
    /// Indexed by colour: that side's knights, bishops, rooks, queens and king.
    pub non_pawn: [u64; 2],
    /// The move before the last and the last, as one continuation entry. `None` after a null move
    /// and within two plies of the root.
    pub continuation: Option<u32>,
}

impl CorrectionKeys {
    #[must_use]
    pub fn of(board: &Board) -> CorrectionKeys {
        let mut non_pawn = [0; 2];
        for colour in Colour::ALL {
            non_pawn[colour.index()] = non_pawn_key(board, colour);
        }
        CorrectionKeys {
            pawn: board.pawn_key(),
            non_pawn,
            continuation: None,
        }
    }

    /// The board's keys with the two moves that reached it, read from the path's frames.
    #[must_use]
    pub fn on_path(board: &Board, frames: &[Frame], ply: usize) -> CorrectionKeys {
        CorrectionKeys {
            continuation: pair(frames, ply),
            ..CorrectionKeys::of(board)
        }
    }
}

/// Below `PIECE_TO` squared for any two frames, each below `PIECE_TO`.
fn pair(frames: &[Frame], ply: usize) -> Option<u32> {
    let before = conthist::context(frames, ply, 2)?;
    let last = conthist::context(frames, ply, 1)?;
    Some(u32::from(before) * PIECE_TO as u32 + u32::from(last))
}

/// Read from the piece sets rather than kept move by move. Each multiply spreads its low bits
/// upward and the rotation brings them back down, where the index reads them.
fn non_pawn_key(board: &Board, colour: Colour) -> u64 {
    const MIX: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut key = 0_u64;
    for piece in [
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
        PieceType::King,
    ] {
        key = (key ^ board.pieces(colour, piece).0)
            .wrapping_mul(MIX)
            .rotate_left(32);
    }
    key ^ (key >> 29)
}

/// Per `Search`, not shared between threads, which keeps a node count a function of the code.
pub struct CorrectionHistory {
    slots: Box<[i32]>,
}

impl CorrectionHistory {
    #[must_use]
    pub fn new() -> CorrectionHistory {
        CorrectionHistory {
            slots: vec![0; TABLES * 2 * TABLE_SLOTS].into_boxed_slice(),
        }
    }

    pub fn clear(&mut self) {
        self.slots.fill(0);
    }

    /// For the gates: whether every entry reads as a new table's.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.slots.iter().all(|&v| v == 0)
    }

    /// For the gates: the correction a position reads, from its own keys and side to move.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn correction_at(&self, board: &Board) -> Score {
        self.correction(&CorrectionKeys::of(board), board.side_to_move())
    }

    /// For the gates: an update at a position, from its own keys and side to move.
    #[cfg(feature = "counters")]
    pub fn update_at(&mut self, board: &Board, delta: Score, depth: u32) {
        self.update(
            &CorrectionKeys::of(board),
            board.side_to_move(),
            delta,
            depth,
        );
    }

    /// The tables' sum, so a structure seen before still corrects a new set of pieces in part.
    #[must_use]
    pub fn correction(&self, keys: &CorrectionKeys, side: Colour) -> Score {
        read(
            Self::indices(keys, side)
                .iter()
                .map(|&i| self.slots[i])
                .sum(),
        )
    }

    /// A deeper search is a better opinion, so it moves each table's entry further, up to the
    /// weight ceiling.
    pub fn update(&mut self, keys: &CorrectionKeys, side: Colour, delta: Score, depth: u32) {
        for i in Self::indices(keys, side) {
            self.slots[i] = fold(self.slots[i], delta, depth);
        }
    }

    fn indices(keys: &CorrectionKeys, side: Colour) -> [usize; TABLES] {
        [
            Self::index(0, keys.pawn, side),
            Self::index(1, keys.non_pawn[0], side),
            Self::index(2, keys.non_pawn[1], side),
        ]
    }

    /// Separate halves per side: the evaluation corrected is already relative to the side to move.
    fn index(table: usize, key: u64, side: Colour) -> usize {
        (table * 2 + side.index()) * TABLE_SLOTS + (key % TABLE_SLOTS as u64) as usize
    }
}

impl Default for CorrectionHistory {
    fn default() -> CorrectionHistory {
        CorrectionHistory::new()
    }
}

/// A weighted mean of the slot and the delta, whose weight grows with depth up to the ceiling.
fn fold(slot: i32, delta: Score, depth: u32) -> i32 {
    let delta = delta.clamp(-MAX_DELTA, MAX_DELTA);
    let weight = i32::try_from(depth)
        .unwrap_or(MAX_WEIGHT)
        .saturating_add(1)
        .min(MAX_WEIGHT);
    let next = (slot * (WEIGHT_UNIT - weight) + delta * GRAIN * weight) / WEIGHT_UNIT;
    next.clamp(-LIMIT, LIMIT)
}

fn read(sum: i32) -> Score {
    (sum / GRAIN).clamp(-MAX_CORRECTION, MAX_CORRECTION)
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Any sum of three slots reads within `MAX_CORRECTION`, too little to make an evaluation
    /// decisive.
    #[kani::proof]
    fn a_corrected_evaluation_is_never_decisive() {
        let slot = || {
            let v: i32 = kani::any();
            kani::assume((-LIMIT..=LIMIT).contains(&v));
            v
        };
        let correction = read(slot() + slot() + slot());
        assert!(correction.abs() <= MAX_CORRECTION);
        let eval: Score = kani::any();
        kani::assume(eval > -score::MAX_EVAL && eval < score::MAX_EVAL);
        assert!(!score::is_decisive(eval + correction));
    }

    /// An update keeps its slot within the limit, for any delta and depth.
    #[kani::proof]
    fn an_update_keeps_its_slot_in_bounds() {
        let slot: i32 = kani::any();
        kani::assume((-LIMIT..=LIMIT).contains(&slot));
        let next = fold(slot, kani::any(), kani::any());
        assert!((-LIMIT..=LIMIT).contains(&next));
    }

    /// Every table, key and side index a slot of the tables.
    #[kani::proof]
    fn every_key_indexes_a_slot() {
        let side = if kani::any() {
            Colour::White
        } else {
            Colour::Black
        };
        let table: usize = kani::any();
        kani::assume(table < TABLES);
        assert!(CorrectionHistory::index(table, kani::any(), side) < TABLES * 2 * TABLE_SLOTS);
    }
}
