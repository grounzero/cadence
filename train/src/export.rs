// SPDX-License-Identifier: GPL-3.0-or-later

//! A quantised checkpoint to the engine's file: the weights as the trainer saved them, already the
//! engine's order, behind a header naming the provenance, the recipe's commit and the data.

use core::fmt;

use cadence_dataset::sha256::sha256;
use cadence_nnue::net::payload_len;
use cadence_nnue::{AnyNetwork, HIDDEN, LoadError, Network, Provenance, WIDE};

/// The trainer pads every quantised file to a multiple of this many bytes.
pub const PADDING: usize = 64;

/// The wide recipe's clip on the output weights, which quantises to 64, the most at which 512 of
/// them at the activation's ceiling keep one perspective's sum inside `i32`.
pub const WIDE_OUTPUT_CLIP: f32 = 1.0;

/// The clip the recipe sets on the output weights at `hidden`, `None` where the optimiser's own
/// 1.98 already keeps the sum inside the loader's bound.
#[must_use]
pub fn output_clip(hidden: usize) -> Option<f32> {
    (hidden == WIDE).then_some(WIDE_OUTPUT_CLIP)
}

/// The hidden width a padded quantised checkpoint of this length and `buckets` output buckets
/// holds, if it is either's.
#[must_use]
pub fn width_of(quantised_len: usize, buckets: usize) -> Option<usize> {
    [HIDDEN, WIDE]
        .into_iter()
        .find(|&h| payload_len(h, buckets).next_multiple_of(PADDING) == quantised_len)
}

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

/// The result is what the engine loads, at the `buckets` its run recorded and the width the
/// checkpoint's length names for them, refused by the same checks as a file.
///
/// # Errors
///
/// A checkpoint of neither width's length at `buckets` or with padding that is not the trainer's
/// fill, a self-play or canary claim for data the converter did not write, or anything the loader
/// refuses.
pub fn export(
    quantised: &[u8],
    buckets: usize,
    provenance: Provenance,
    trainer: &str,
    manifest: &[u8],
) -> Result<AnyNetwork, ExportError> {
    let Some(hidden) = width_of(quantised.len(), buckets) else {
        return Err(ExportError::Length {
            want: payload_len(HIDDEN, buckets).next_multiple_of(PADDING),
            got: quantised.len(),
        });
    };
    let (payload, pad) = quantised.split_at(payload_len(hidden, buckets));
    if pad.iter().zip(FILL.iter().cycle()).any(|(b, f)| b != f) {
        return Err(ExportError::Padding);
    }
    let self_play_data = matches!(provenance, Provenance::SelfPlay | Provenance::Canary);
    if self_play_data && !manifest.starts_with(CONVERTED.as_bytes()) {
        return Err(ExportError::NotConverted);
    }
    let manifest = sha256(manifest);
    let net = if hidden == WIDE {
        Network::<WIDE>::load_payload(payload, buckets, provenance, trainer, manifest)
            .map(AnyNetwork::from)
    } else {
        Network::<HIDDEN>::load_payload(payload, buckets, provenance, trainer, manifest)
            .map(AnyNetwork::from)
    };
    net.map_err(ExportError::Load)
}
