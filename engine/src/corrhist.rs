// SPDX-License-Identifier: GPL-3.0-or-later

//! The running difference between what the evaluation says about a pawn
//! structure and what searching it returned.
//!
//! A static evaluation is systematically wrong in ways that repeat: the same
//! pawn structure tends to be misread the same way every time it is reached.
//! This holds that difference per structure and per side and offers it back,
//! so a rule comparing a static score against a bound compares a corrected
//! one.

use cadence_core::Colour;

use crate::score::Score;

/// Slots per side. Sixteen thousand structures is enough that a game's
/// distinct pawn keys rarely collide, and the table stays small enough to
/// sit in cache beside the history.
const TABLE_SLOTS: usize = 1 << 14;

/// The fixed-point scale an entry is held at, so the weighted mean below
/// does not round a small persistent correction away. Nothing outside this
/// module sees it.
pub const GRAIN: i32 = 256;

/// The denominator of the weighted mean, and the largest weight one
/// observation may carry. A new observation moves an entry by at most
/// `MAX_WEIGHT / WEIGHT_UNIT` of the distance to itself.
const WEIGHT_UNIT: i32 = 256;
pub const MAX_WEIGHT: i32 = 16;

/// The largest correction offered, in centipawns. It keeps a correction
/// incommensurable with the mate scale, so no corrected evaluation can
/// imply a mate that is not there.
pub const MAX_CORRECTION: Score = 256;

/// The largest difference folded in, in centipawns. A node whose search
/// disagreed with the evaluation by more than this is a tactic rather than
/// an evaluation error, and would otherwise dominate the mean.
pub const MAX_DELTA: Score = 1024;

/// A correction per pawn structure per side, cleared when a search starts.
///
/// It is 128 kibibytes and lives on the heap for the reason `History` does.
/// Nothing here is shared between threads: the table belongs to one
/// `Search`, which is what keeps a node count a function of the code.
pub struct CorrectionHistory {
    slots: Box<[i32]>,
}

impl CorrectionHistory {
    /// An empty table, which corrects everything by nothing.
    #[must_use]
    pub fn new() -> CorrectionHistory {
        CorrectionHistory {
            slots: vec![0; 2 * TABLE_SLOTS].into_boxed_slice(),
        }
    }

    /// Forget everything, which is what the start of a search wants.
    pub fn clear(&mut self) {
        self.slots.fill(0);
    }

    /// The correction this structure has earned, in centipawns.
    #[must_use]
    pub fn correction(&self, pawn_key: u64, side: Colour) -> Score {
        let stored = self.slots[Self::index(pawn_key, side)];
        (stored / GRAIN).clamp(-MAX_CORRECTION, MAX_CORRECTION)
    }

    /// Fold one observation in, weighted by the depth that produced it.
    ///
    /// A deeper search is a better opinion, so it moves the entry further,
    /// up to the weight ceiling. The difference is clamped first, because a
    /// tactic is not an evaluation error.
    pub fn update(&mut self, pawn_key: u64, side: Colour, delta: Score, depth: u32) {
        let delta = delta.clamp(-MAX_DELTA, MAX_DELTA);
        let weight = (i32::try_from(depth).unwrap_or(MAX_WEIGHT) + 1).min(MAX_WEIGHT);
        let slot = &mut self.slots[Self::index(pawn_key, side)];
        let next = (*slot * (WEIGHT_UNIT - weight) + delta * GRAIN * weight) / WEIGHT_UNIT;
        *slot = next.clamp(-MAX_CORRECTION * GRAIN, MAX_CORRECTION * GRAIN);
    }

    /// The two sides keep separate halves, because the evaluation this
    /// corrects is already relative to the side to move.
    fn index(pawn_key: u64, side: Colour) -> usize {
        side.index() * TABLE_SLOTS + (pawn_key % TABLE_SLOTS as u64) as usize
    }
}

impl Default for CorrectionHistory {
    fn default() -> CorrectionHistory {
        CorrectionHistory::new()
    }
}
