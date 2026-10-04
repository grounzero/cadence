// SPDX-License-Identifier: GPL-3.0-or-later

//! A static evaluation is wrong in ways that repeat, so the difference is held per pawn structure
//! and side and offered back to any rule comparing a static score against a bound.

use cadence_core::Colour;

use crate::score::Score;

/// Enough that a game's pawn keys rarely collide, small enough to sit in cache beside the history.
const TABLE_SLOTS: usize = 1 << 14;

/// So the weighted mean does not round a small persistent correction away.
pub const GRAIN: i32 = 256;

/// The weighted mean's denominator. An observation moves an entry at most `MAX_WEIGHT /
/// WEIGHT_UNIT` of the distance to itself.
const WEIGHT_UNIT: i32 = 256;
pub const MAX_WEIGHT: i32 = 16;

/// Keeps a corrected evaluation off the mate scale.
pub const MAX_CORRECTION: Score = 256;

/// A larger disagreement is a tactic, not an evaluation error, and would dominate the mean.
pub const MAX_DELTA: Score = 1024;

/// Per `Search`, not shared between threads, which keeps a node count a function of the code.
pub struct CorrectionHistory {
    slots: Box<[i32]>,
}

impl CorrectionHistory {
    #[must_use]
    pub fn new() -> CorrectionHistory {
        CorrectionHistory {
            slots: vec![0; 2 * TABLE_SLOTS].into_boxed_slice(),
        }
    }

    pub fn clear(&mut self) {
        self.slots.fill(0);
    }

    #[must_use]
    pub fn correction(&self, pawn_key: u64, side: Colour) -> Score {
        let stored = self.slots[Self::index(pawn_key, side)];
        (stored / GRAIN).clamp(-MAX_CORRECTION, MAX_CORRECTION)
    }

    /// A deeper search is a better opinion, so it moves the entry further, up to the weight
    /// ceiling.
    pub fn update(&mut self, pawn_key: u64, side: Colour, delta: Score, depth: u32) {
        let delta = delta.clamp(-MAX_DELTA, MAX_DELTA);
        let weight = (i32::try_from(depth).unwrap_or(MAX_WEIGHT) + 1).min(MAX_WEIGHT);
        let slot = &mut self.slots[Self::index(pawn_key, side)];
        let next = (*slot * (WEIGHT_UNIT - weight) + delta * GRAIN * weight) / WEIGHT_UNIT;
        *slot = next.clamp(-MAX_CORRECTION * GRAIN, MAX_CORRECTION * GRAIN);
    }

    /// Separate halves per side: the evaluation corrected is already relative to the side to move.
    fn index(pawn_key: u64, side: Colour) -> usize {
        side.index() * TABLE_SLOTS + (pawn_key % TABLE_SLOTS as u64) as usize
    }
}

impl Default for CorrectionHistory {
    fn default() -> CorrectionHistory {
        CorrectionHistory::new()
    }
}
