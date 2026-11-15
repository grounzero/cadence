// SPDX-License-Identifier: GPL-3.0-or-later

//! Which files a run may train on: each must hash to the training file a manifest names, so a
//! changed byte, a holdout split or another dataset's records are refused before training starts.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use cadence_dataset::sha256::{Sha256, hex};

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
    let first = manifest.lines().next().unwrap_or_default();
    let named: Vec<&str> = if first == CONVERTED.trim_end() {
        manifest
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                match fields.as_slice() {
                    ["file", "train.bin", sha, ..] => Some(*sha),
                    _ => None,
                }
            })
            .collect()
    } else if first == BOOK_HEADER {
        manifest
            .lines()
            .filter_map(|line| line.strip_prefix("records-sha256\t"))
            .collect()
    } else {
        return Err("not a manifest cadence-tools convert or cadence-train book wrote".to_string());
    };
    match named.as_slice() {
        [one] if one.len() == 64 && one.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(one.to_ascii_lowercase())
        }
        [] => Err("the manifest names no training file".to_string()),
        _ => Err("the manifest names no single, well-formed training file".to_string()),
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

/// Every data file hashes to a training file one of the manifests names.
///
/// # Errors
///
/// An unreadable file or manifest, a manifest `training_sha256` refuses, or a data file whose
/// SHA-256 no manifest names as its training file, with both sides spelled out.
pub fn verify(data: &[&Path], manifests: &[&Path]) -> Result<(), String> {
    if manifests.is_empty() {
        return Err("no manifest, and a run trains only on files a manifest names".to_string());
    }
    let mut named = Vec::new();
    for manifest in manifests {
        let text = std::fs::read_to_string(manifest)
            .map_err(|e| format!("{}: {e}", manifest.display()))?;
        let sha = training_sha256(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
        named.push(format!("{sha} ({})", manifest.display()));
    }
    for path in data {
        let sha = file_sha256(path)?;
        if !named.iter().any(|n| n.starts_with(&sha)) {
            return Err(format!(
                "{}: SHA-256 {sha} is not the training file any manifest names: {}",
                path.display(),
                named.join(", ")
            ));
        }
    }
    Ok(())
}

/// The SHA-256 of the one holdout split a manifest vouches for.
///
/// # Errors
///
/// None, until manifests are read for it.
pub fn holdout_sha256(manifest: &str) -> Result<String, String> {
    Ok(format!("{} bytes unread", manifest.len()))
}

/// The holdout file hashes to the holdout split one of the manifests names.
///
/// # Errors
///
/// Every holdout, until one is checked.
pub fn verify_holdout(holdout: &Path, manifests: &[&Path]) -> Result<(), String> {
    let _ = manifests;
    Err(format!("{}: unchecked", holdout.display()))
}
