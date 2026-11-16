// SPDX-License-Identifier: GPL-3.0-or-later

//! A quantised checkpoint to the engine's file: the weights as the trainer saved them, already the
//! engine's order, behind a header naming the provenance, the recipe's commit and the data.

use core::fmt;

use cadence_dataset::sha256::sha256;
use cadence_nnue::net::PAYLOAD_LEN;
use cadence_nnue::{LoadError, Network, Provenance};

/// The trainer pads every quantised file to a multiple of this many bytes.
pub const PADDING: usize = 64;

/// What the pinned trainer pads with, repeated from the first byte after the weights.
pub const FILL: &[u8; 6] = b"bullet";

/// The first line of every manifest the converter writes, which a self-play net's data must have.
pub const CONVERTED: &str = "# cadence conversion manifest, written by cadence-tools convert\n";

#[derive(Debug, PartialEq, Eq)]
pub enum ExportError {
    Length {
        want: usize,
        got: usize,
    },
    Padding,
    /// A self-play or canary claim for data whose manifest the converter did not write.
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
            ExportError::Padding => {
                write!(f, "the padding after the weights is not the trainer's fill")
            }
            ExportError::NotConverted => write!(
                f,
                "a self-play or canary net needs a manifest cadence-tools convert wrote"
            ),
            ExportError::Load(e) => write!(f, "the engine's loader refuses it: {e}"),
        }
    }
}

/// The result is what the engine loads, refused by the same checks as a file.
///
/// # Errors
///
/// A checkpoint of the wrong length or with padding that is not the trainer's fill, a self-play or
/// canary claim for data the converter did not write, or anything `Network::from_payload` refuses.
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
    if pad.iter().zip(FILL.iter().cycle()).any(|(b, f)| b != f) {
        return Err(ExportError::Padding);
    }
    let self_play_data = matches!(provenance, Provenance::SelfPlay | Provenance::Canary);
    if self_play_data && !manifest.starts_with(CONVERTED.as_bytes()) {
        return Err(ExportError::NotConverted);
    }
    Network::from_payload(payload, provenance, trainer, sha256(manifest)).map_err(ExportError::Load)
}
