// SPDX-License-Identifier: GPL-3.0-or-later

//! A quantised checkpoint to the engine's file: the weights as the trainer saved them, already the
//! engine's order, behind a header naming the provenance, the recipe's commit and the data.

use core::fmt;

use cadence_core::features::NUM_INPUTS;
use cadence_dataset::sha256::sha256;
use cadence_nnue::net::{PAYLOAD_LEN, QA, QB, payload_len};
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

/// The hidden width a padded quantised checkpoint of this length holds, if it is either's.
#[must_use]
pub fn width_of(quantised_len: usize) -> Option<usize> {
    [HIDDEN, WIDE]
        .into_iter()
        .find(|&h| payload_len(h).next_multiple_of(PADDING) == quantised_len)
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
    /// Unquantised weights of another length than the checkpoint's width has.
    RawLength {
        want: usize,
        got: usize,
    },
    /// Unquantised output weights that do not quantise to the checkpoint's own, so are not its.
    RawDisagrees {
        word: usize,
    },
    /// A scaled output weight or bias past what its 16 bits hold.
    Scaled {
        word: usize,
    },
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
            ExportError::RawLength { want, got } => {
                write!(
                    f,
                    "{got} bytes of unquantised weights where this width has {want}"
                )
            }
            ExportError::RawDisagrees { word } => write!(
                f,
                "the unquantised weights are not this checkpoint's: output word {word} quantises \
                 to another value than the checkpoint holds"
            ),
            ExportError::Scaled { word } => {
                write!(
                    f,
                    "output word {word} times the factor does not fit 16 bits"
                )
            }
            ExportError::Load(e) => write!(f, "the engine's loader refuses it: {e}"),
        }
    }
}

/// The checkpoint with its output weights and bias replaced by the unquantised ones times `factor`,
/// quantised once as the trainer quantises them; the first layer's words are left as they are.
///
/// # Errors
///
/// A checkpoint of neither width, unquantised weights of another length or that do not quantise to
/// the checkpoint's own output layer, or a scaled word past 16 bits.
pub fn fold(quantised: &[u8], raw: &[u8], factor: f32) -> Result<Vec<u8>, ExportError> {
    let Some(hidden) = width_of(quantised.len()) else {
        return Err(ExportError::Length {
            want: PAYLOAD_LEN.next_multiple_of(PADDING),
            got: quantised.len(),
        });
    };
    let want = payload_len(hidden) * 2;
    if raw.len() != want {
        return Err(ExportError::RawLength {
            want,
            got: raw.len(),
        });
    }
    let first = NUM_INPUTS * hidden + hidden;
    let mut folded = quantised.to_vec();
    for word in 0..=2 * hidden {
        let at = first + word;
        let mut float = [0; 4];
        float.copy_from_slice(&raw[4 * at..4 * at + 4]);
        // The bias is saved at both scales, the weights at the output's alone.
        let scale = if word == 2 * hidden { QA * QB } else { QB };
        let unit = f64::from(scale) * f64::from(f32::from_le_bytes(float));
        let held = i16::from_le_bytes([quantised[2 * at], quantised[2 * at + 1]]);
        if unit.round() as i64 != i64::from(held) {
            return Err(ExportError::RawDisagrees { word });
        }
        let scaled = (unit * f64::from(factor)).round();
        if scaled.abs() > f64::from(i16::MAX) {
            return Err(ExportError::Scaled { word });
        }
        folded[2 * at..2 * at + 2].copy_from_slice(&(scaled as i16).to_le_bytes());
    }
    Ok(folded)
}

/// The result is what the engine loads, at the width the checkpoint's length names, refused by the
/// same checks as a file.
///
/// # Errors
///
/// A checkpoint of neither width's length or with padding that is not the trainer's fill, a
/// self-play or canary claim for data the converter did not write, or anything the loader refuses.
pub fn export(
    quantised: &[u8],
    provenance: Provenance,
    trainer: &str,
    manifest: &[u8],
) -> Result<AnyNetwork, ExportError> {
    let Some(hidden) = width_of(quantised.len()) else {
        return Err(ExportError::Length {
            want: PAYLOAD_LEN.next_multiple_of(PADDING),
            got: quantised.len(),
        });
    };
    let (payload, pad) = quantised.split_at(payload_len(hidden));
    if pad.iter().zip(FILL.iter().cycle()).any(|(b, f)| b != f) {
        return Err(ExportError::Padding);
    }
    let self_play_data = matches!(provenance, Provenance::SelfPlay | Provenance::Canary);
    if self_play_data && !manifest.starts_with(CONVERTED.as_bytes()) {
        return Err(ExportError::NotConverted);
    }
    let manifest = sha256(manifest);
    let net = if hidden == WIDE {
        Network::<WIDE>::load_payload(payload, provenance, trainer, manifest).map(AnyNetwork::from)
    } else {
        Network::<HIDDEN>::load_payload(payload, provenance, trainer, manifest)
            .map(AnyNetwork::from)
    };
    net.map_err(ExportError::Load)
}
