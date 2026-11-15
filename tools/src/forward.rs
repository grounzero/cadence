// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's quantised forward pass against an f32 one written here from the file alone, over
//! random legal positions; with `--raw`, against the trainer's unquantised weights as well.

use std::process::ExitCode;

/// # Errors
///
/// Every reading, until the comparison is written.
pub fn run(_args: &[String]) -> Result<ExitCode, String> {
    Err("forward is not built".to_string())
}
