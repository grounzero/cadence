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

const MAGIC: [u8; 4] = *b"CDNN";
const FORMAT: u16 = 1;
/// The activation byte naming the one this build computes: clamped to [0, QA], then squared.
const SCRELU: u8 = 0;
pub const HEADER_LEN: usize = 128;
/// The feature weights, their biases, both perspectives' output weights and the output bias.
pub const PAYLOAD_LEN: usize = 2 * (NUM_INPUTS * HIDDEN + HIDDEN + 2 * HIDDEN + 1);

/// Each compiled constant the header repeats, at its offset, so a file for another build is refused.
const ARCHITECTURE: [(&str, usize, u16); 5] = [
    ("inputs", 6, NUM_INPUTS as u16),
    ("hidden width", 8, HIDDEN as u16),
    ("QA", 12, QA as u16),
    ("QB", 14, QB as u16),
    ("scale", 16, SCALE as u16),
];
const ACTIVATION_AT: usize = 10;
const PROVENANCE_LEN_AT: usize = 11;
const RESERVED: core::ops::Range<usize> = 18..24;
/// The trainer crate's commit, 40 lowercase hexadecimal digits.
const TRAINER: core::ops::Range<usize> = 24..64;
/// The SHA-256 of the manifest the training data was built from.
const MANIFEST: core::ops::Range<usize> = 64..96;
/// ASCII without spaces, zero-padded: `self-play`, or the external dataset's name.
const PROVENANCE: core::ops::Range<usize> = 96..HEADER_LEN;
const SELF_PLAY: &str = "self-play";

/// At most 32 pieces stand on a board, so no lane ever sums more feature weights than this.
const MAX_ACTIVE: usize = 32;

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
            LoadError::Magic => write!(f, "not a Cadence net file"),
            LoadError::Format(v) => write!(f, "format {v}, where this build reads {FORMAT}"),
            LoadError::Architecture { field, file, build } => {
                write!(
                    f,
                    "the file's {field} is {file}, where this build's is {build}"
                )
            }
            LoadError::Reserved => write!(f, "the header's reserved bytes are not zero"),
            LoadError::Trainer => write!(f, "the trainer commit is not 40 lowercase hex digits"),
            LoadError::Provenance => write!(f, "the provenance is not 1 to 32 ASCII characters"),
            LoadError::NotSelfPlay(name) => {
                write!(
                    f,
                    "trained on {name}, not self-play, so it may not generate data"
                )
            }
            LoadError::Lane { lane, bound } => {
                write!(
                    f,
                    "lane {lane} could reach {bound}, past the accumulator's 32767"
                )
            }
            LoadError::OutputWeight { index, weight } => {
                write!(f, "output weight {index} is {weight}, outside -127 to 127")
            }
            LoadError::OutputSum { bound } => {
                write!(f, "the output layer could sum to {bound}, past i32")
            }
        }
    }
}

/// What the header names beside the architecture.
struct Origin {
    provenance: Provenance,
    trainer: [u8; 40],
    manifest: [u8; 32],
}

fn read_header(h: &[u8; HEADER_LEN]) -> Result<Origin, LoadError> {
    let word = |at: usize| u16::from_le_bytes([h[at], h[at + 1]]);
    if h[..4] != MAGIC {
        return Err(LoadError::Magic);
    }
    if word(4) != FORMAT {
        return Err(LoadError::Format(word(4)));
    }
    let activation = ("activation", ACTIVATION_AT, u16::from(SCRELU));
    for (field, at, build) in ARCHITECTURE {
        if word(at) != build {
            return Err(LoadError::Architecture {
                field,
                file: word(at),
                build,
            });
        }
    }
    if u16::from(h[activation.1]) != activation.2 {
        let (field, at, build) = activation;
        return Err(LoadError::Architecture {
            field,
            file: u16::from(h[at]),
            build,
        });
    }
    if h[RESERVED].iter().any(|&b| b != 0) {
        return Err(LoadError::Reserved);
    }
    let mut trainer = [0; 40];
    trainer.copy_from_slice(&h[TRAINER]);
    if !trainer
        .iter()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return Err(LoadError::Trainer);
    }
    let mut manifest = [0; 32];
    manifest.copy_from_slice(&h[MANIFEST]);
    let n = usize::from(h[PROVENANCE_LEN_AT]);
    let field = &h[PROVENANCE];
    if n == 0 || n > field.len() {
        return Err(LoadError::Provenance);
    }
    let (name, pad) = field.split_at(n);
    if !name.iter().all(u8::is_ascii_graphic) || pad.iter().any(|&b| b != 0) {
        return Err(LoadError::Provenance);
    }
    let name: String = name.iter().map(|&b| char::from(b)).collect();
    let provenance = if name == SELF_PLAY {
        Provenance::SelfPlay
    } else {
        Provenance::External(name)
    };
    Ok(Origin {
        provenance,
        trainer,
        manifest,
    })
}

/// The weights, laid out as the trainer exports them: one column of `HIDDEN` per input feature.
pub struct Network {
    pub(crate) weights: Box<[[i16; HIDDEN]; NUM_INPUTS]>,
    pub(crate) bias: [i16; HIDDEN],
    /// The side to move's perspective first, then the other's.
    pub(crate) output: [[i16; HIDDEN]; 2],
    pub(crate) output_bias: i16,
    origin: Origin,
}

impl Network {
    /// # Errors
    ///
    /// A file of the wrong length or header, or weights that could overflow, named by `LoadError`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Network, LoadError> {
        let want = HEADER_LEN + PAYLOAD_LEN;
        let Some((header, _)) = bytes
            .split_first_chunk::<HEADER_LEN>()
            .filter(|_| bytes.len() == want)
        else {
            return Err(LoadError::Length {
                want,
                got: bytes.len(),
            });
        };
        let origin = read_header(header)?;
        let mut words = bytes[HEADER_LEN..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&w| i16::from_le_bytes(w));
        let mut next = || words.next().unwrap_or(0);
        let mut weights = zeroed_columns();
        fill(weights.as_flattened_mut(), &mut next);
        let mut bias = [0; HIDDEN];
        fill(&mut bias, &mut next);
        let mut output = [[0; HIDDEN]; 2];
        fill(output.as_flattened_mut(), &mut next);
        let output_bias = next();
        let net = Network {
            weights,
            bias,
            output,
            output_bias,
            origin,
        };
        net.check()?;
        Ok(net)
    }

    /// A trainer's weights in file order, behind a header naming where they came from.
    ///
    /// # Errors
    ///
    /// Every payload, until the header is written for one.
    pub fn from_payload(
        payload: &[u8],
        provenance: Provenance,
        trainer: &str,
        manifest: [u8; 32],
    ) -> Result<Network, LoadError> {
        drop(provenance);
        let _ = (trainer, manifest);
        Err(LoadError::Length {
            want: PAYLOAD_LEN,
            got: payload.len(),
        })
    }

    /// The loader for any net that generates data: `from_bytes`, and a self-play provenance.
    ///
    /// # Errors
    ///
    /// As `from_bytes`, and `LoadError::NotSelfPlay` for a net trained on an external dataset.
    pub fn for_datagen(bytes: &[u8]) -> Result<Network, LoadError> {
        let net = Network::from_bytes(bytes)?;
        match &net.origin.provenance {
            Provenance::SelfPlay => Ok(net),
            Provenance::External(name) => Err(LoadError::NotSelfPlay(name.clone())),
        }
    }

    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.origin.provenance
    }

    /// The trainer crate's commit, as 40 hexadecimal digits.
    #[must_use]
    pub fn trainer(&self) -> String {
        self.origin.trainer.iter().map(|&b| char::from(b)).collect()
    }

    #[must_use]
    pub fn manifest(&self) -> [u8; 32] {
        self.origin.manifest
    }

    /// The file `from_bytes` reads, for a net built in memory.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0; HEADER_LEN];
        out[..4].copy_from_slice(&MAGIC);
        out[4..6].copy_from_slice(&FORMAT.to_le_bytes());
        for (_, at, value) in ARCHITECTURE {
            out[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
        out[ACTIVATION_AT] = SCRELU;
        let name = self.origin.provenance.to_string();
        out[PROVENANCE_LEN_AT] = u8::try_from(name.len()).unwrap_or(0);
        out[TRAINER].copy_from_slice(&self.origin.trainer);
        out[MANIFEST].copy_from_slice(&self.origin.manifest);
        let field = &mut out[PROVENANCE];
        let n = name.len().min(field.len());
        field[..n].copy_from_slice(&name.as_bytes()[..n]);
        out.reserve(PAYLOAD_LEN);
        let words = self
            .weights
            .iter()
            .flatten()
            .chain(&self.bias)
            .chain(self.output.iter().flatten())
            .chain(core::iter::once(&self.output_bias));
        for w in words {
            out.extend_from_slice(&w.to_le_bytes());
        }
        out
    }

    /// No lane, product or sum the inference forms may overflow, so every build and every target
    /// computes the same numbers.
    fn check(&self) -> Result<(), LoadError> {
        let mut magnitudes = [0_i64; NUM_INPUTS];
        for lane in 0..HIDDEN {
            for (m, column) in magnitudes.iter_mut().zip(self.weights.iter()) {
                *m = i64::from(column[lane]).abs();
            }
            magnitudes.select_nth_unstable_by(MAX_ACTIVE - 1, |a, b| b.cmp(a));
            let bound =
                i64::from(self.bias[lane]).abs() + magnitudes[..MAX_ACTIVE].iter().sum::<i64>();
            if bound > i64::from(i16::MAX) {
                return Err(LoadError::Lane { lane, bound });
            }
        }
        let mut sum = 0_i64;
        for (index, &weight) in self.output.iter().flatten().enumerate() {
            if !(-127..=127).contains(&weight) {
                return Err(LoadError::OutputWeight { index, weight });
            }
            sum += i64::from(QA * QA) * i64::from(weight).abs();
        }
        if sum > i64::from(i32::MAX) {
            return Err(LoadError::OutputSum { bound: sum });
        }
        Ok(())
    }

    /// A net from seeded weights small enough to load, for tests and timing.
    #[must_use]
    pub fn seeded(seed: u64, provenance: Provenance) -> Network {
        let mut state = seed;
        let mut draw = |span: i16| {
            let (next, out) = cadence_core::rng::splitmix64(state);
            state = next;
            let span_u = u64::from(span.unsigned_abs()) * 2 + 1;
            i16::try_from(out % span_u).unwrap_or(0) - span
        };
        let mut weights = zeroed_columns();
        fill(weights.as_flattened_mut(), || draw(400));
        let mut bias = [0; HIDDEN];
        fill(&mut bias, || draw(200));
        let mut output = [[0; HIDDEN]; 2];
        fill(output.as_flattened_mut(), || draw(60));
        Network {
            weights,
            bias,
            output,
            output_bias: draw(500),
            origin: Origin {
                provenance,
                trainer: [b'0'; 40],
                manifest: [0; 32],
            },
        }
    }
}

fn zeroed_columns() -> Box<[[i16; HIDDEN]; NUM_INPUTS]> {
    vec![[0; HIDDEN]; NUM_INPUTS]
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| unreachable!("a vector of NUM_INPUTS columns is NUM_INPUTS long"))
}

fn fill(cells: &mut [i16], mut next: impl FnMut() -> i16) {
    for cell in cells {
        *cell = next();
    }
}
