// SPDX-License-Identifier: GPL-3.0-or-later

//! A static evaluation is wrong in ways that repeat, so the difference is held per pawn structure
//! and side and offered back to any rule comparing a static score against a bound.

use cadence_core::Colour;

use crate::score::{self, Score};

/// Enough that a game's pawn keys rarely collide, small enough to sit in cache beside the history.
const TABLE_SLOTS: usize = 1 << 14;

/// So the weighted mean does not round a small persistent correction away.
pub const GRAIN: i32 = 256;

/// The weighted mean's denominator. An observation moves an entry at most `MAX_WEIGHT /
/// WEIGHT_UNIT` of the distance to itself.
const WEIGHT_UNIT: i32 = 256;
pub const MAX_WEIGHT: i32 = 16;

/// Keeps a corrected evaluation off the decided scale, below the mates and the tablebase band.
pub const MAX_CORRECTION: Score = 256;
const _: () = assert!(score::MAX_EVAL + MAX_CORRECTION < score::TB_WIN_IN_MAX_PLY);

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

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Any slot's correction is within `MAX_CORRECTION`, too little to make an evaluation decisive.
    #[kani::proof]
    fn a_corrected_evaluation_is_never_decisive() {
        let history = CorrectionHistory {
            slots: vec![kani::any(); 1].into_boxed_slice(),
        };
        let correction = history.correction(0, Colour::White);
        assert!(correction.abs() <= MAX_CORRECTION);
        let eval: Score = kani::any();
        kani::assume(eval > -score::MAX_EVAL && eval < score::MAX_EVAL);
        assert!(!score::is_decisive(eval + correction));
    }

    /// An update keeps its slot within the bound `correction` assumes, for any delta and depth.
    #[kani::proof]
    fn an_update_keeps_its_slot_in_bounds() {
        let slot: i32 = kani::any();
        kani::assume((-MAX_CORRECTION * GRAIN..=MAX_CORRECTION * GRAIN).contains(&slot));
        let mut history = CorrectionHistory {
            slots: vec![slot; 1].into_boxed_slice(),
        };
        history.update(0, Colour::White, kani::any(), kani::any());
        assert!((-MAX_CORRECTION * GRAIN..=MAX_CORRECTION * GRAIN).contains(&history.slots[0]));
    }

    /// Every pawn key and side index a slot of the table.
    #[kani::proof]
    fn every_key_indexes_a_slot() {
        let side = if kani::any() {
            Colour::White
        } else {
            Colour::Black
        };
        assert!(CorrectionHistory::index(kani::any(), side) < 2 * TABLE_SLOTS);
    }
}
