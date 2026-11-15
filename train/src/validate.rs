// SPDX-License-Identifier: GPL-3.0-or-later

//! Validation loss from the trainer's own forward pass: each holdout record read back as a position
//! the trainer can be asked about, scored against the target its training run used.

use cadence_dataset::trainer::RECORD_LEN;

/// One holdout record: its position with the side to move as White, which is how a record stores
/// it, and that side's score and result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Example {
    pub fen: String,
    pub score: i16,
    /// 0 loss, 1 draw, 2 win.
    pub result: u8,
}

/// A record decoded.
///
/// # Errors
///
/// None, until records are decoded.
pub fn example(record: &[u8; RECORD_LEN]) -> Result<Example, String> {
    let _ = record;
    Ok(Example {
        fen: String::new(),
        score: 0,
        result: 0,
    })
}

/// The trainer's loss for one example.
#[must_use]
pub fn loss(output: f32, example: &Example, result_share: f32, scale: f32) -> f64 {
    let _ = (output, example, result_share, scale);
    0.0
}

/// The mean of a checkpoint's logged training losses for its own superbatch.
#[must_use]
pub fn training_loss(log: &str, superbatch: usize) -> Option<f64> {
    let _ = (log, superbatch);
    None
}
