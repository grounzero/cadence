// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::Write;

/// Flushed: a GUI that sent `isready` blocks until it sees `readyok`, so a buffered reply is a
/// hang.
pub(super) fn say(line: std::fmt::Arguments<'_>) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
