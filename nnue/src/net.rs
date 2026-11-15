// SPDX-License-Identifier: GPL-3.0-or-later

//! The network's shape, its file and its loading: a 128-byte header naming the format, the
//! architecture, the trainer and the provenance, then the weights, refused at load if any could overflow.

use core::fmt;

use cadence_core::features::NUM_INPUTS;

/// Units a perspective; widening is one constant and a new net.
pub const HIDDEN: usize = 256;
/// The activation's ceiling and the feature layer's scale.
pub const QA: i32 = 255;
/// The output layer's scale.
pub const QB: i32 = 64;
/// Centipawns per unit of the trained output.
pub const SCALE: i32 = 400;

const FORMAT: u16 = 1;
pub const HEADER_LEN: usize = 128;
/// The feature weights, their biases, both perspectives' output weights and the output bias.
pub const PAYLOAD_LEN: usize = 2 * (NUM_INPUTS * HIDDEN + HIDDEN + 2 * HIDDEN + 1);
const SELF_PLAY: &str = "self-play";

/// Where the net's training data came from: only a self-play net may generate more data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    SelfPlay,
    /// Trained on the named dataset, as the one-time validation net is; it never generates data.
    External(String),
}

impl fmt::Display for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provenance::SelfPlay => f.write_str(SELF_PLAY),
            Provenance::External(name) => f.write_str(name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Length {
        want: usize,
        got: usize,
    },
    Magic,
    Format(u16),
    /// A constant of the file's architecture that is not the one this build compiles.
    Architecture {
        field: &'static str,
        file: u16,
        build: u16,
    },
    Reserved,
    Trainer,
    Provenance,
    /// A net that loads but may not generate data, named by its provenance.
    NotSelfPlay(String),
    /// A lane whose bias and 32 largest weights could leave `i16`.
    Lane {
        lane: usize,
        bound: i64,
    },
    /// An output weight outside [-127, 127], where the activation times it could leave `i16`.
    OutputWeight {
        index: usize,
        weight: i16,
    },
    /// The output layer's worst-case sum could leave `i32`.
    OutputSum {
        bound: i64,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Length { want, got } => write!(f, "{got} bytes where a net is {want}"),
            LoadError::Format(v) => write!(f, "format {v}, where this build reads {FORMAT}"),
            other => write!(f, "{other:?}"),
        }
    }
}

/// The weights, laid out as the trainer exports them: one column of `HIDDEN` per input feature.
pub struct Network {
    provenance: Provenance,
}

impl Network {
    /// # Errors
    ///
    /// Every file, until the loader is built.
    pub fn from_bytes(bytes: &[u8]) -> Result<Network, LoadError> {
        Err(LoadError::Length {
            want: HEADER_LEN + PAYLOAD_LEN,
            got: bytes.len(),
        })
    }

    /// # Errors
    ///
    /// As `from_bytes`.
    pub fn for_datagen(bytes: &[u8]) -> Result<Network, LoadError> {
        Network::from_bytes(bytes)
    }

    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    #[must_use]
    pub fn trainer(&self) -> String {
        String::new()
    }

    #[must_use]
    pub fn manifest(&self) -> [u8; 32] {
        [0; 32]
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        Vec::new()
    }

    #[must_use]
    pub fn seeded(_seed: u64, provenance: Provenance) -> Network {
        Network { provenance }
    }
}
