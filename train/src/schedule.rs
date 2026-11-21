// SPDX-License-Identifier: GPL-3.0-or-later

//! The learning rate's two schedules over a run: the recipe's cosine, and steps between the same
//! two rates.

use bullet_lib::trainer::schedule::lr::{CosineDecayLR, StepLR};

pub const INITIAL_LR: f32 = 0.001;
/// Five falls of 0.3 below the first rate by the last superbatch.
pub const FINAL_LR: f32 = INITIAL_LR * 0.3 * 0.3 * 0.3 * 0.3 * 0.3;
const GAMMA: f32 = 0.3;
const STEPS: usize = 5;

/// The recipe's: from `INITIAL_LR` to `FINAL_LR` along a cosine.
#[must_use]
pub fn cosine(superbatches: usize) -> CosineDecayLR {
    CosineDecayLR {
        initial_lr: INITIAL_LR,
        final_lr: FINAL_LR,
        final_superbatch: superbatches,
    }
}

/// The same five falls of 0.3 taken as steps, the last in the run's final stretch, so it ends
/// where the cosine does and differs only in the decay's shape.
#[must_use]
pub fn stepped(superbatches: usize) -> StepLR {
    StepLR {
        start: INITIAL_LR,
        gamma: GAMMA,
        step: superbatches.div_ceil(STEPS + 1).max(1),
    }
}
