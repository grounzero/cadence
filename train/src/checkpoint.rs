// SPDX-License-Identifier: GPL-3.0-or-later

//! Each checkpoint the trainer saves, loaded as an export would load it while the run goes on, so
//! a run stops at the first checkpoint the engine could not load.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use cadence_core::features::NUM_INPUTS;
use cadence_nnue::net::{HEADER_LEN, payload_len};
use cadence_nnue::{HIDDEN, Provenance, WIDE};

use crate::export::{PADDING, export};

/// A trainer commit for a load that names none, since a checkpoint is not yet given one.
const UNNAMED: &str = "0000000000000000000000000000000000000000";

/// Beside the checkpoints, each one's name and the summed magnitude of its quantised output
/// weights, one a line, in the order they were saved.
pub const OUTPUT_SUMS: &str = "output-sums";

/// The summed magnitude of the quantised output weights, the figure the engine's loader bounds, at
/// the width the payload's length names; the payload may carry the trainer's padding.
#[must_use]
pub fn output_sum(payload: &[u8]) -> u64 {
    let hidden = if payload.len() >= payload_len(WIDE) {
        WIDE
    } else {
        HIDDEN
    };
    payload
        .as_chunks::<2>()
        .0
        .iter()
        .skip(NUM_INPUTS * hidden + hidden)
        .take(2 * hidden)
        .map(|&w| u64::from(i16::from_le_bytes(w).unsigned_abs()))
        .sum()
}

/// The checkpoints of run `id` under `out` saved whole since the last scan, at the run's width
/// `hidden`, each loaded and its output sum appended to `OUTPUT_SUMS`; a file still being written
/// is left for the next scan.
///
/// # Errors
///
/// The first checkpoint the engine's loader refuses, by name, or a file that cannot be read or
/// written.
pub fn check_saved(
    out: &Path,
    id: &str,
    hidden: usize,
    seen: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    let whole = payload_len(hidden).next_multiple_of(PADDING);
    let mut saved = Vec::new();
    for entry in std::fs::read_dir(out).map_err(|e| format!("{}: {e}", out.display()))? {
        let dir = entry.map_err(|e| format!("{}: {e}", out.display()))?.path();
        let superbatch = dir.file_name().and_then(|n| {
            n.to_str()?
                .strip_prefix(id)?
                .strip_prefix('-')?
                .parse()
                .ok()
        });
        if let Some(superbatch) = superbatch.filter(|_: &usize| !seen.contains(&dir)) {
            saved.push((superbatch, dir));
        }
    }
    saved.sort();
    for (_, dir) in saved {
        let file = dir.join("quantised.bin");
        if std::fs::metadata(&file).map_or(true, |m| m.len() != whole as u64) {
            continue;
        }
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let unnamed = Provenance::External("checkpoint".to_string());
        let net = export(&bytes, unnamed, UNNAMED, b"")
            .map_err(|e| format!("{}: {e}, so the run is stopped", dir.display()))?;
        let name = dir
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let line = format!("{name}\t{}\n", output_sum(&net.to_bytes()[HEADER_LEN..]));
        let sums = out.join(OUTPUT_SUMS);
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sums)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .map_err(|e| format!("{}: {e}", sums.display()))?;
        seen.insert(dir);
    }
    Ok(())
}

/// After training, every checkpoint the run was to save, each of which must have been loaded, so a
/// file the scans never judged is refused rather than passed; the count is of those loaded.
///
/// # Errors
///
/// As `check_saved`, or an expected checkpoint that was never loaded, by name.
pub fn check_all(
    out: &Path,
    id: &str,
    hidden: usize,
    expected: &[PathBuf],
    seen: &mut BTreeSet<PathBuf>,
) -> Result<usize, String> {
    check_saved(out, id, hidden, seen)?;
    if let Some(missing) = expected.iter().find(|c| !seen.contains(*c)) {
        return Err(format!(
            "{}: never loaded, its quantised.bin missing or not a whole checkpoint's length",
            missing.display()
        ));
    }
    Ok(expected.len())
}
