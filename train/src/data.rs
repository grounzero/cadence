// SPDX-License-Identifier: GPL-3.0-or-later

//! Which files a run may train on: each must hash to the training file a manifest names, so a
//! changed byte, a holdout split or another dataset's records are refused before training starts.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use cadence_dataset::sha256::{Sha256, hex, sha256};
use cadence_nnue::Inputs;

use crate::export::CONVERTED;

/// The first line of a book conversion's manifest.
pub const BOOK_HEADER: &str = "# cadence book conversion, written by cadence-train book";

/// The SHA-256, in lowercase hex, of the one training file a manifest vouches for.
///
/// # Errors
///
/// A manifest neither `cadence-tools convert` nor `cadence-train book` wrote, or one naming no
/// training file or more than one.
pub fn training_sha256(manifest: &str) -> Result<String, String> {
    named(manifest, "train.bin", "records-sha256", "training file")
}

/// The SHA-256, in lowercase hex, of the one holdout split a manifest vouches for.
///
/// # Errors
///
/// As `training_sha256`, for the holdout split.
pub fn holdout_sha256(manifest: &str) -> Result<String, String> {
    named(manifest, "holdout.bin", "holdout-sha256", "holdout split")
}

/// The one hash a converter's manifest gives `file` and a book conversion's gives under `key`.
fn named(manifest: &str, file: &str, key: &str, what: &str) -> Result<String, String> {
    let first = manifest.lines().next().unwrap_or_default();
    let named: Vec<&str> = if first == CONVERTED.trim_end() {
        manifest
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                match fields.as_slice() {
                    ["file", name, sha, ..] if *name == file => Some(*sha),
                    _ => None,
                }
            })
            .collect()
    } else if first == BOOK_HEADER {
        manifest
            .lines()
            .filter_map(|line| line.strip_prefix(key)?.strip_prefix('\t'))
            .collect()
    } else {
        return Err("not a manifest cadence-tools convert or cadence-train book wrote".to_string());
    };
    match named.as_slice() {
        [one] if one.len() == 64 && one.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(one.to_ascii_lowercase())
        }
        [] => Err(format!("the manifest names no {what}")),
        _ => Err(format!("the manifest names no single, well-formed {what}")),
    }
}

/// A file's SHA-256 in lowercase hex, read a mebibyte at a time.
///
/// # Errors
///
/// A file that cannot be opened or read.
pub fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            return Ok(hex(&hasher.finish()));
        }
        hasher.update(&buffer[..n]);
    }
}

/// Every data file hashes to a training file one of the manifests names, and no two to the same.
///
/// # Errors
///
/// As `matched`.
pub fn verify(data: &[&Path], manifests: &[&Path]) -> Result<(), String> {
    matched(data, manifests).map(|_| ())
}

/// The manifests whose training file a data file hashes to, once each and in the order given.
///
/// # Errors
///
/// An unreadable file or manifest, a manifest `training_sha256` refuses, a data file whose SHA-256
/// no manifest names as its training file, or two data files with the same records.
pub fn matched<'a>(data: &[&Path], manifests: &[&'a Path]) -> Result<Vec<&'a Path>, String> {
    if manifests.is_empty() {
        return Err("no manifest, and a run trains only on files a manifest names".to_string());
    }
    let mut named = Vec::new();
    for manifest in manifests {
        let text = std::fs::read_to_string(manifest)
            .map_err(|e| format!("{}: {e}", manifest.display()))?;
        let sha = training_sha256(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
        named.push(sha);
    }
    let mut seen: Vec<(String, &Path)> = Vec::new();
    let mut used = vec![false; manifests.len()];
    for path in data {
        let sha = file_sha256(path)?;
        if let Some((_, earlier)) = seen.iter().find(|(s, _)| *s == sha) {
            return Err(format!(
                "{}: the same records as {} (SHA-256 {sha}), and a run reads each file once",
                path.display(),
                earlier.display()
            ));
        }
        let Some(i) = named.iter().position(|n| *n == sha) else {
            let listed: Vec<String> = named
                .iter()
                .zip(manifests)
                .map(|(n, m)| format!("{n} ({})", m.display()))
                .collect();
            return Err(format!(
                "{}: SHA-256 {sha} is not the training file any manifest names: {}",
                path.display(),
                listed.join(", ")
            ));
        };
        used[i] = true;
        seen.push((sha, path));
    }
    Ok(manifests
        .iter()
        .zip(used)
        .filter_map(|(m, u)| u.then_some(*m))
        .collect())
}

/// The holdout file hashes to the holdout split one of the manifests names.
///
/// # Errors
///
/// As `verify`, for the holdout split.
pub fn verify_holdout(holdout: &Path, manifests: &[&Path]) -> Result<(), String> {
    let sha = file_sha256(holdout)?;
    let mut named = Vec::new();
    for manifest in manifests {
        let text = std::fs::read_to_string(manifest)
            .map_err(|e| format!("{}: {e}", manifest.display()))?;
        let want = holdout_sha256(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
        if want == sha {
            return Ok(());
        }
        named.push(format!("{want} ({})", manifest.display()));
    }
    Err(format!(
        "{}: SHA-256 {sha} is not the holdout split any manifest names: {}",
        holdout.display(),
        named.join(", ")
    ))
}

/// Beside a run's checkpoints, the SHA-256 of each manifest its training data matched, one a line,
/// so that an export can name only one of them.
pub const VERIFIED_MANIFESTS: &str = "verified-manifests";

/// Written beside the checkpoints, so an export names the commit that trained them, not its own.
pub const COMMIT_FILE: &str = "trainer-commit";

/// Beside a run's checkpoints, the share of the target the run took from game results, which
/// validation must read its checkpoints with.
pub const RESULT_SHARE_FILE: &str = "result-share";

/// Writes `RESULT_SHARE_FILE` in `out` for the share the run trains with.
///
/// # Errors
///
/// A file that cannot be written.
pub fn record_result_share(out: &Path, share: f32) -> Result<(), String> {
    let path = out.join(RESULT_SHARE_FILE);
    std::fs::write(&path, format!("{share}\n")).map_err(|e| format!("{}: {e}", path.display()))
}

/// The result share the run in `run` recorded, which a share given on the command line must equal.
///
/// # Errors
///
/// No share recorded in `run`, one that does not parse, or a given share that differs from it.
pub fn run_result_share(run: &Path, given: Option<f32>) -> Result<f32, String> {
    let path = run.join(RESULT_SHARE_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "{}: {e}, so the run's result share is not known",
            path.display()
        )
    })?;
    let recorded: f32 = text
        .trim()
        .parse()
        .map_err(|_| format!("{}: {} is not a share", path.display(), text.trim()))?;
    match given {
        Some(share) if share.to_bits() != recorded.to_bits() => Err(format!(
            "--result-share {share} differs from the {recorded} the run recorded in {}",
            path.display()
        )),
        _ => Ok(recorded),
    }
}

/// The file a run leaves in its directory naming the input layout it trains with, which an export
/// writes into the net's header and a validation must build its trainer by.
pub const INPUTS_FILE: &str = "inputs";
const PLAIN: &str = "plain";
const MIRRORED: &str = "mirrored";

/// The layout a name on the command line or in a run's record stands for.
///
/// # Errors
///
/// A name that is neither layout's.
pub fn inputs_named(name: &str) -> Result<Inputs, String> {
    match name {
        PLAIN => Ok(Inputs::Plain),
        MIRRORED => Ok(Inputs::Mirrored),
        other => Err(format!(
            "{other} is not an input layout: {PLAIN} or {MIRRORED}"
        )),
    }
}

/// Writes `INPUTS_FILE` in `out` for the layout the run trains with.
///
/// # Errors
///
/// A file that cannot be written.
pub fn record_inputs(out: &Path, inputs: Inputs) -> Result<(), String> {
    let path = out.join(INPUTS_FILE);
    let name = match inputs {
        Inputs::Plain => PLAIN,
        Inputs::Mirrored => MIRRORED,
    };
    std::fs::write(&path, format!("{name}\n")).map_err(|e| format!("{}: {e}", path.display()))
}

/// The layout the run in `run` recorded; a run from before the record was kept is plain.
///
/// # Errors
///
/// A record that cannot be read or names no layout.
pub fn run_inputs(run: &Path) -> Result<Inputs, String> {
    let path = run.join(INPUTS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => inputs_named(text.trim()).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Inputs::Plain),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// A run's directory holds no other run's record, since a second run would overwrite the first's.
///
/// # Errors
///
/// A directory holding `COMMIT_FILE`, `VERIFIED_MANIFESTS`, `RESULT_SHARE_FILE` or `INPUTS_FILE`.
pub fn fresh_run(out: &Path) -> Result<(), String> {
    for name in [
        COMMIT_FILE,
        VERIFIED_MANIFESTS,
        RESULT_SHARE_FILE,
        INPUTS_FILE,
    ] {
        if out.join(name).exists() {
            return Err(format!(
                "{} holds another run's {name}, and a run never shares its directory",
                out.display()
            ));
        }
    }
    Ok(())
}

/// Writes `VERIFIED_MANIFESTS` in `out` for the manifests the run verified.
///
/// # Errors
///
/// An unreadable manifest, or a file that cannot be written.
pub fn record_manifests(out: &Path, manifests: &[&Path]) -> Result<(), String> {
    let mut text = String::new();
    for manifest in manifests {
        let _ = writeln!(text, "{}\t{}", file_sha256(manifest)?, manifest.display());
    }
    let path = out.join(VERIFIED_MANIFESTS);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The manifest an export names is one the run in `run` verified.
///
/// # Errors
///
/// No `VERIFIED_MANIFESTS` in `run`, or a manifest it does not list.
pub fn verify_export(run: &Path, manifest: &[u8]) -> Result<(), String> {
    let path = run.join(VERIFIED_MANIFESTS);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let sha = hex(&sha256(manifest));
    if text
        .lines()
        .any(|line| line.split('\t').next() == Some(sha.as_str()))
    {
        return Ok(());
    }
    Err(format!(
        "the manifest, SHA-256 {sha}, is not one the run verified, as {} lists them",
        path.display()
    ))
}
