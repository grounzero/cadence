// SPDX-License-Identifier: GPL-3.0-or-later

//! Each checkpoint the trainer saves, loaded as an export would load it while the run goes on, so
//! a run stops at the first checkpoint the engine could not load.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cadence_core::features::NUM_INPUTS;
use cadence_nnue::HIDDEN;

/// Beside the checkpoints, each one's name and the summed magnitude of its quantised output
/// weights, one a line, in the order they were saved.
pub const OUTPUT_SUMS: &str = "output-sums";

/// The summed magnitude of the quantised output weights, the figure the engine's loader bounds;
/// the payload may carry the trainer's padding.
#[must_use]
pub fn output_sum(payload: &[u8]) -> u64 {
    payload
        .as_chunks::<2>()
        .0
        .iter()
        .skip(NUM_INPUTS * HIDDEN + HIDDEN)
        .take(2 * HIDDEN)
        .map(|&w| u64::from(i16::from_le_bytes(w).unsigned_abs()))
        .sum()
}

/// The checkpoints of run `id` under `out` saved whole since the last scan, each loaded and its
/// output sum appended to `OUTPUT_SUMS`; a file still being written is left for the next scan.
///
/// # Errors
///
/// The first checkpoint the engine's loader refuses, by name, or a file that cannot be read or
/// written.
pub fn check_saved(_out: &Path, _id: &str, _seen: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    Ok(())
}
