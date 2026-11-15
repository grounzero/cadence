// SPDX-License-Identifier: GPL-3.0-or-later

//! A tuning set's text, one position a line as a FEN and White's result in brackets, to the trainer's
//! records: the score zero, since such a set carries none, and the result the side to move's.

use cadence_core::position::Board;
use cadence_dataset::trainer::RECORD_LEN;

/// What a conversion read, for its manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Read {
    pub positions: u64,
    pub white_to_move: u64,
    /// The side to move's losses, draws and wins.
    pub results: [u64; 3],
}

/// The board and White's result, or why the line is not a position.
///
/// # Errors
///
/// Every line, until the reading is written.
pub fn parse_line(line: &str) -> Result<(Board, u8), String> {
    Err(format!("not read: {line}"))
}

/// Every line a record, shuffled from `seed`.
///
/// # Errors
///
/// Every text, until the conversion is written.
pub fn convert(text: &str, seed: u64) -> Result<(Vec<[u8; RECORD_LEN]>, Read), String> {
    Err(format!("not converted: {} bytes, seed {seed}", text.len()))
}
