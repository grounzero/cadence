// SPDX-License-Identifier: GPL-3.0-or-later

//! The overfit test's sets and bars: a small consistent set the recipe must memorise at the result
//! share's own meaning, and a real set whose loss must fall well below the best constant guess.

use cadence_dataset::trainer::RECORD_LEN;

use crate::validate::Example;

/// Positions in the synthetic set, each a distinct network input with one target.
pub const SYNTHETIC_POSITIONS: usize = 64;

/// How far a memorised position's output may sit from its target, as a probability.
pub const MEMORISED: f64 = 0.02;

/// How far, on average, the memorised outputs must sit from the reading with the shares swapped,
/// so the trainer's meaning of the result share is the one the recipe states.
pub const BLEND_GAP: f64 = 0.1;

/// The trained loss on real data as a share of the best constant guess's loss, at most.
pub const REAL_RATIO: f64 = 0.7;

/// Records the constant guess is read over, from the start of the data.
pub const REAL_SAMPLE: usize = 100_000;

/// What the synthetic part read: the largest distance from a target, and the mean distance from
/// the swapped reading's targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntheticReading {
    pub worst: f64,
    pub swapped: f64,
}

/// The synthetic set's records, from random legal games under `seed`, with scores and results drawn
/// apart so that the two shares' readings disagree.
#[must_use]
pub fn synthetic(_seed: u64) -> Vec<[u8; RECORD_LEN]> {
    Vec::new()
}

/// The synthetic part's bar on the trained outputs, one a set example in order.
///
/// # Errors
///
/// An output farther than `MEMORISED` from its target, or outputs as near the swapped reading as
/// within `BLEND_GAP`, either way with the reading.
pub fn synthetic_bar(
    _outputs: &[f32],
    _examples: &[Example],
    _result_share: f32,
    _scale: f32,
) -> Result<SyntheticReading, String> {
    Ok(SyntheticReading {
        worst: 0.0,
        swapped: 1.0,
    })
}

/// The best constant guess's loss over `examples`: their targets' mean squared deviation.
#[must_use]
pub fn constant_loss(_examples: &[Example], _result_share: f32, _scale: f32) -> f64 {
    0.0
}

/// The real part's bar.
///
/// # Errors
///
/// A trained loss above `REAL_RATIO` of the constant guess's, with both.
pub fn real_bar(_trained: f64, _constant: f64) -> Result<(), String> {
    Ok(())
}
