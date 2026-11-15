// SPDX-License-Identifier: GPL-3.0-or-later

//! A quantised checkpoint to the engine's file: the weights as the trainer saved them, already the
//! engine's order, behind a header naming the provenance, the recipe's commit and the data.

use core::fmt;

use cadence_dataset::sha256::sha256;
use cadence_nnue::net::PAYLOAD_LEN;
use cadence_nnue::{LoadError, Network, Provenance};

/// The trainer pads every quantised file to a multiple of this many bytes.
pub const PADDING: usize = 64;

/// The first line of every manifest the converter writes, which a self-play net's data must have.
pub const CONVERTED: &str = "# cadence conversion manifest, written by cadence-tools convert\n";

#[derive(Debug, PartialEq, Eq)]
pub enum ExportError {
    Length {
        want: usize,
        got: usize,
    },
    Padding,
    /// A self-play provenance claimed for data whose manifest the converter did not write.
    NotConverted,
    Load(LoadError),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExportError::Length { want, got } => {
                write!(
                    f,
                    "{got} bytes where a padded checkpoint of this shape is {want}"
                )
            }
            ExportError::Padding => write!(f, "the padding after the weights is not zero"),
            ExportError::NotConverted => write!(
                f,
                "a self-play net's manifest is one cadence-tools convert wrote, and this is not"
            ),
            ExportError::Load(e) => write!(f, "the engine's loader refuses it: {e}"),
        }
    }
}

/// The result is what the engine loads, refused by the same checks as a file.
///
/// # Errors
///
/// A checkpoint of the wrong length or with padding that is not zero, a self-play claim for data
/// the converter did not write, or anything `Network::from_payload` refuses.
pub fn export(
    quantised: &[u8],
    provenance: Provenance,
    trainer: &str,
    manifest: &[u8],
) -> Result<Network, ExportError> {
    let want = PAYLOAD_LEN.next_multiple_of(PADDING);
    if quantised.len() != want {
        return Err(ExportError::Length {
            want,
            got: quantised.len(),
        });
    }
    let (payload, pad) = quantised.split_at(PAYLOAD_LEN);
    if pad.iter().any(|&b| b != 0) {
        return Err(ExportError::Padding);
    }
    if provenance == Provenance::SelfPlay && !manifest.starts_with(CONVERTED.as_bytes()) {
        return Err(ExportError::NotConverted);
    }
    Network::from_payload(payload, provenance, trainer, sha256(manifest)).map_err(ExportError::Load)
}
