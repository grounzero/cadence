// SPDX-License-Identifier: GPL-3.0-or-later

//! The network's shape, its file and its loading: a 128-byte header naming the format, the
//! architecture, the trainer and the provenance, then the weights, refused at load if any could overflow.

use core::fmt;
use std::sync::Arc;

use cadence_core::features::{KING_BUCKETS, NUM_INPUTS};

/// Units a perspective at the narrow width, which every width-generic type here takes unless told
/// otherwise. The shipped net may be wider, so a caller that loads it takes `AnyNetwork`.
pub const HIDDEN: usize = 256;
/// The other width a net may have; its header names which of the two it is.
pub const WIDE: usize = 512;
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
/// The feature weights, their biases, both perspectives' output weights and the output bias, for a
/// first layer of one row a piece and square; `Inputs::payload_len` is any layout's.
#[must_use]
pub const fn payload_len(hidden: usize) -> usize {
    Inputs::Plain.payload_len(hidden)
}
/// A net of the narrow width's payload.
pub const PAYLOAD_LEN: usize = payload_len(HIDDEN);

/// The net this build ships, read in from one fixed path when the crate compiles and replaced at
/// each promotion; an empty file ships none.
pub const EMBEDDED: &[u8] = include_bytes!("../net.cdnn");

/// Each compiled constant the header repeats, at its offset, with the rows its layout has and the
/// width a load asks for, so a file for another build, layout or width is refused.
const fn architecture(rows: usize, hidden: usize) -> [(&'static str, usize, u16); 5] {
    [
        ("inputs", 6, rows as u16),
        ("hidden width", WIDTH_AT, hidden as u16),
        ("QA", 12, QA as u16),
        ("QB", 14, QB as u16),
        ("scale", 16, SCALE as u16),
    ]
}
const WIDTH_AT: usize = 8;
const ACTIVATION_AT: usize = 10;
const PROVENANCE_LEN_AT: usize = 11;
/// The byte naming how pieces index the first layer; a build from before it was read refuses
/// anything but zero here, so a mirrored net never loads as a plain one.
const INPUTS_AT: usize = 18;
const RESERVED: core::ops::Range<usize> = 19..24;
/// The trainer crate's commit, 40 lowercase hexadecimal digits.
const TRAINER: core::ops::Range<usize> = 24..64;
/// The SHA-256 of the manifest the training data was built from.
const MANIFEST: core::ops::Range<usize> = 64..96;
/// ASCII without spaces, zero-padded: `self-play`, or the external dataset's name.
const PROVENANCE: core::ops::Range<usize> = 96..HEADER_LEN;
const SELF_PLAY: &str = "self-play";
const CANARY: &str = "canary";

/// At most 32 pieces stand on a board, so no lane ever sums more feature weights than this.
const MAX_ACTIVE: usize = 32;

/// How a perspective's pieces index the first layer: as they stand, or with the board flipped left
/// to right while that perspective's own king is on files e to h. `Bucketed` flips it so too, and
/// reads the block of rows its own king's bucket names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inputs {
    Plain,
    Mirrored,
    Bucketed,
}

impl Inputs {
    /// The first layer's rows: one a piece and square, and under `Bucketed` as many a king bucket.
    #[must_use]
    pub const fn rows(self) -> usize {
        match self {
            Inputs::Plain | Inputs::Mirrored => NUM_INPUTS,
            Inputs::Bucketed => KING_BUCKETS * NUM_INPUTS,
        }
    }

    /// The bytes of weights a net of this layout holds at `hidden` units a perspective.
    #[must_use]
    pub const fn payload_len(self, hidden: usize) -> usize {
        2 * (self.rows() * hidden + hidden + 2 * hidden + 1)
    }

    const fn byte(self) -> u8 {
        match self {
            Inputs::Plain => 0,
            Inputs::Mirrored => 1,
            Inputs::Bucketed => 2,
        }
    }

    const fn named(byte: u8) -> Option<Inputs> {
        match byte {
            0 => Some(Inputs::Plain),
            1 => Some(Inputs::Mirrored),
            2 => Some(Inputs::Bucketed),
            _ => None,
        }
    }

    /// The layout a header's byte asks for, plain where it names none. A length or an inputs word
    /// is weighed against the rows it implies before the byte itself is refused.
    fn implied(byte: u8) -> Inputs {
        Inputs::named(byte).unwrap_or(Inputs::Plain)
    }
}

/// Where the net's training data came from: only a self-play net may generate more data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    SelfPlay,
    /// A short run on self-play data that rehearses the pipeline, named apart so it is never taken
    /// for the net it rehearses; it never generates data.
    Canary,
    /// Trained on the named dataset, as the one-time validation net is; it never generates data.
    External(String),
}

impl fmt::Display for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provenance::SelfPlay => f.write_str(SELF_PLAY),
            Provenance::Canary => f.write_str(CANARY),
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
    /// A hidden width that is neither of the two this build reads.
    Width(u16),
    /// A constant of the file's architecture that is not the one this build compiles.
    Architecture {
        field: &'static str,
        file: u16,
        build: u16,
    },
    /// An input layout byte that names no layout this build reads.
    Inputs(u8),
    Reserved,
    Trainer,
    Provenance,
    /// A net that loads but may not generate data, named by its provenance.
    NotSelfPlay(String),
    /// A canary net, which loads but may not generate data.
    Canary,
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
            LoadError::Width(w) => {
                write!(
                    f,
                    "hidden width {w}, where this build reads {HIDDEN} or {WIDE}"
                )
            }
            LoadError::Architecture { field, file, build } => {
                write!(
                    f,
                    "the file's {field} is {file}, where this build's is {build}"
                )
            }
            LoadError::Reserved => write!(f, "the header's reserved bytes are not zero"),
            LoadError::Trainer => write!(f, "the trainer commit is not 40 lowercase hex digits"),
            LoadError::Provenance => write!(f, "the provenance is not 1 to 32 ASCII characters"),
            LoadError::Canary => write!(f, "a canary net, which may not generate data"),
            LoadError::NotSelfPlay(name) => {
                write!(
                    f,
                    "trained on {name}, not self-play, so it may not generate data"
                )
            }
            LoadError::Inputs(v) => {
                write!(
                    f,
                    "input layout {v}, where this build reads 0 plain, 1 mirrored or 2 king buckets"
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
    inputs: Inputs,
    provenance: Provenance,
    trainer: [u8; 40],
    manifest: [u8; 32],
}

/// A name the field cannot hold is written so that `read_header` refuses it.
fn write_header(origin: &Origin, hidden: usize) -> [u8; HEADER_LEN] {
    let mut out = [0; HEADER_LEN];
    out[..4].copy_from_slice(&MAGIC);
    out[4..6].copy_from_slice(&FORMAT.to_le_bytes());
    for (_, at, value) in architecture(origin.inputs.rows(), hidden) {
        out[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    out[ACTIVATION_AT] = SCRELU;
    out[INPUTS_AT] = origin.inputs.byte();
    let name = origin.provenance.to_string();
    out[PROVENANCE_LEN_AT] = u8::try_from(name.len()).unwrap_or(0);
    out[TRAINER].copy_from_slice(&origin.trainer);
    out[MANIFEST].copy_from_slice(&origin.manifest);
    let field = &mut out[PROVENANCE];
    let n = name.len().min(field.len());
    field[..n].copy_from_slice(&name.as_bytes()[..n]);
    out
}

fn read_header(h: &[u8; HEADER_LEN], hidden: usize) -> Result<Origin, LoadError> {
    let word = |at: usize| u16::from_le_bytes([h[at], h[at + 1]]);
    if h[..4] != MAGIC {
        return Err(LoadError::Magic);
    }
    if word(4) != FORMAT {
        return Err(LoadError::Format(word(4)));
    }
    let activation = ("activation", ACTIVATION_AT, u16::from(SCRELU));
    let rows = Inputs::implied(h[INPUTS_AT]).rows();
    for (field, at, build) in architecture(rows, hidden) {
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
    let layout = h[INPUTS_AT];
    let inputs = Inputs::named(layout).ok_or(LoadError::Inputs(layout))?;
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
    } else if name == CANARY {
        Provenance::Canary
    } else {
        Provenance::External(name)
    };
    Ok(Origin {
        inputs,
        provenance,
        trainer,
        manifest,
    })
}

/// The weights, laid out as the trainer exports them: one column of `H` per input feature, `H`
/// being the hidden width the header names. A bucketed net's columns stand a king bucket at a time.
pub struct Network<const H: usize = HIDDEN> {
    pub(crate) weights: Box<[[i16; H]]>,
    pub(crate) bias: [i16; H],
    /// The side to move's perspective first, then the other's.
    pub(crate) output: [[i16; H]; 2],
    pub(crate) output_bias: i16,
    origin: Origin,
}

/// The narrow width's names, so a caller that never asks for a width gets 256 units a perspective.
/// `AnyNetwork` holds a net of either width.
impl Network {
    /// # Errors
    ///
    /// A file of the wrong length or header, or weights that could overflow, named by `LoadError`.
    pub fn from_bytes(bytes: &[u8]) -> Result<Network, LoadError> {
        Network::load(bytes)
    }

    /// A trainer's weights in file order, behind a header naming where they came from, loaded and
    /// checked as a file is.
    ///
    /// # Errors
    ///
    /// As `from_bytes`: a payload of the wrong length, a trainer commit or provenance the header
    /// cannot hold, or weights that could overflow.
    pub fn from_payload(
        payload: &[u8],
        provenance: Provenance,
        trainer: &str,
        manifest: [u8; 32],
    ) -> Result<Network, LoadError> {
        Network::load_payload(payload, provenance, trainer, manifest)
    }

    /// The loader for any net that generates data: `from_bytes`, and a self-play provenance.
    ///
    /// # Errors
    ///
    /// As `from_bytes`, `LoadError::Canary` for a canary net, and `LoadError::NotSelfPlay` for a
    /// net trained on an external dataset.
    pub fn for_datagen(bytes: &[u8]) -> Result<Network, LoadError> {
        Network::load_for_datagen(bytes)
    }

    /// The loader for the net a build ships: `None` for no bytes, else `for_datagen`'s answer,
    /// because only a self-play net is ever shipped.
    #[must_use]
    pub fn shipped(bytes: &[u8]) -> Option<Result<Network, LoadError>> {
        (!bytes.is_empty()).then(|| Network::for_datagen(bytes))
    }

    /// A net from seeded weights small enough to load, for tests and timing.
    #[must_use]
    pub fn seeded(seed: u64, provenance: Provenance) -> Network {
        Network::generate(seed, provenance)
    }
}

impl<const H: usize> Network<H> {
    /// `from_bytes` at width `H`, refusing a header that names another. The length is weighed
    /// first, against the rows the header's layout byte implies.
    ///
    /// # Errors
    ///
    /// A file of the wrong length or header, or weights that could overflow, named by `LoadError`.
    pub fn load(bytes: &[u8]) -> Result<Network<H>, LoadError> {
        let layout = bytes.get(INPUTS_AT).copied().unwrap_or(0);
        let want = HEADER_LEN + Inputs::implied(layout).payload_len(H);
        let Some((header, _)) = bytes
            .split_first_chunk::<HEADER_LEN>()
            .filter(|_| bytes.len() == want)
        else {
            return Err(LoadError::Length {
                want,
                got: bytes.len(),
            });
        };
        let origin = read_header(header, H)?;
        let mut words = bytes[HEADER_LEN..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&w| i16::from_le_bytes(w));
        let mut next = || words.next().unwrap_or(0);
        let mut weights = zeroed_columns::<H>(origin.inputs.rows());
        fill(weights.as_flattened_mut(), &mut next);
        let mut bias = [0; H];
        fill(&mut bias, &mut next);
        let mut output = [[0; H]; 2];
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

    /// `from_payload` at width `H`.
    ///
    /// # Errors
    ///
    /// As `load`: a payload of the wrong length, a trainer commit or provenance the header cannot
    /// hold, or weights that could overflow.
    pub fn load_payload(
        payload: &[u8],
        provenance: Provenance,
        trainer: &str,
        manifest: [u8; 32],
    ) -> Result<Network<H>, LoadError> {
        Network::load_payload_as(payload, Inputs::Plain, provenance, trainer, manifest)
    }

    /// `load_payload` behind a header naming `inputs`, the layout the weights were trained with.
    ///
    /// # Errors
    ///
    /// As `load_payload`.
    pub fn load_payload_as(
        payload: &[u8],
        inputs: Inputs,
        provenance: Provenance,
        trainer: &str,
        manifest: [u8; 32],
    ) -> Result<Network<H>, LoadError> {
        let trainer: [u8; 40] = trainer
            .as_bytes()
            .try_into()
            .map_err(|_| LoadError::Trainer)?;
        let origin = Origin {
            inputs,
            provenance,
            trainer,
            manifest,
        };
        let mut bytes = write_header(&origin, H).to_vec();
        bytes.extend_from_slice(payload);
        Network::load(&bytes)
    }

    /// `for_datagen` at width `H`.
    ///
    /// # Errors
    ///
    /// As `load`, `LoadError::Canary` for a canary net, and `LoadError::NotSelfPlay` for a net
    /// trained on an external dataset.
    pub fn load_for_datagen(bytes: &[u8]) -> Result<Network<H>, LoadError> {
        let net = Network::<H>::load(bytes)?;
        match &net.origin.provenance {
            Provenance::SelfPlay => Ok(net),
            Provenance::Canary => Err(LoadError::Canary),
            Provenance::External(name) => Err(LoadError::NotSelfPlay(name.clone())),
        }
    }

    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.origin.provenance
    }

    /// The input layout the header names, which every index into the first layer follows.
    #[must_use]
    pub fn inputs(&self) -> Inputs {
        self.origin.inputs
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

    /// The file `load` reads, for a net built in memory.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = write_header(&self.origin, H).to_vec();
        out.reserve(self.origin.inputs.payload_len(H));
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
    /// computes the same numbers. A lane is bounded within each king bucket's rows, since a
    /// perspective reads all its pieces from one bucket.
    fn check(&self) -> Result<(), LoadError> {
        let mut magnitudes = [0_i64; NUM_INPUTS];
        for lane in 0..H {
            for bucket in self.weights.as_chunks::<NUM_INPUTS>().0 {
                for (m, column) in magnitudes.iter_mut().zip(bucket) {
                    *m = i64::from(column[lane]).abs();
                }
                magnitudes.select_nth_unstable_by(MAX_ACTIVE - 1, |a, b| b.cmp(a));
                let largest: i64 = magnitudes[..MAX_ACTIVE].iter().sum();
                let bound = i64::from(self.bias[lane]).abs() + largest;
                if bound > i64::from(i16::MAX) {
                    return Err(LoadError::Lane { lane, bound });
                }
            }
        }
        for (perspective, weights) in self.output.iter().enumerate() {
            let mut sum = 0_i64;
            for (i, &weight) in weights.iter().enumerate() {
                if !(-127..=127).contains(&weight) {
                    let index = perspective * H + i;
                    return Err(LoadError::OutputWeight { index, weight });
                }
                sum += i64::from(QA * QA) * i64::from(weight).abs();
            }
            if sum > i64::from(i32::MAX) {
                return Err(LoadError::OutputSum { bound: sum });
            }
        }
        Ok(())
    }

    /// `seeded` at width `H`, its output weights drawn small enough that the wide net's sum loads.
    #[must_use]
    pub fn generate(seed: u64, provenance: Provenance) -> Network<H> {
        Network::generate_as(seed, Inputs::Plain, provenance)
    }

    /// `generate` with the rows of `inputs`, behind a header that names it.
    #[must_use]
    pub fn generate_as(seed: u64, inputs: Inputs, provenance: Provenance) -> Network<H> {
        let mut state = seed;
        let mut draw = |span: i16| {
            let (out, next) = cadence_core::rng::splitmix64(state);
            state = next;
            let span_u = u64::from(span.unsigned_abs()) * 2 + 1;
            i16::try_from(out % span_u).unwrap_or(0) - span
        };
        let mut weights = zeroed_columns::<H>(inputs.rows());
        fill(weights.as_flattened_mut(), || draw(400));
        let mut bias = [0; H];
        fill(&mut bias, || draw(200));
        let mut output = [[0; H]; 2];
        fill(output.as_flattened_mut(), || draw(60));
        Network {
            weights,
            bias,
            output,
            output_bias: draw(500),
            origin: Origin {
                inputs,
                provenance,
                trainer: [b'0'; 40],
                manifest: [0; 32],
            },
        }
    }
}

/// A net of either width, as its header names it, shared by every search that scores with it.
#[derive(Clone)]
pub enum AnyNetwork {
    Narrow(Arc<Network<HIDDEN>>),
    Wide(Arc<Network<WIDE>>),
}

impl AnyNetwork {
    /// The width the header names picks the loader, once its magic and format are this build's.
    ///
    /// # Errors
    ///
    /// As `file_len`, then as `Network::load`.
    pub fn from_bytes(bytes: &[u8]) -> Result<AnyNetwork, LoadError> {
        if AnyNetwork::shape(bytes)?.1 == WIDE {
            Ok(Network::<WIDE>::load(bytes)?.into())
        } else {
            Ok(Network::<HIDDEN>::load(bytes)?.into())
        }
    }

    /// `from_bytes`, and a self-play provenance.
    ///
    /// # Errors
    ///
    /// As `from_bytes`, `LoadError::Canary` and `LoadError::NotSelfPlay`.
    pub fn for_datagen(bytes: &[u8]) -> Result<AnyNetwork, LoadError> {
        if AnyNetwork::shape(bytes)?.1 == WIDE {
            Ok(Network::<WIDE>::load_for_datagen(bytes)?.into())
        } else {
            Ok(Network::<HIDDEN>::load_for_datagen(bytes)?.into())
        }
    }

    /// The length of the file whose first bytes are `head`, from its header alone, so a foreign
    /// file is named as one before its length is weighed.
    ///
    /// # Errors
    ///
    /// `Magic`, `Format`, `Width` or `Inputs` for a header this build does not read, and `Length`
    /// for one too short to name a width.
    pub fn file_len(head: &[u8]) -> Result<usize, LoadError> {
        let (inputs, hidden) = AnyNetwork::shape(head)?;
        Ok(HEADER_LEN + inputs.payload_len(hidden))
    }

    /// The layout and the width the header in `head` names, once its magic and format are this
    /// build's.
    fn shape(head: &[u8]) -> Result<(Inputs, usize), LoadError> {
        if !head.starts_with(&MAGIC) {
            return Err(LoadError::Magic);
        }
        let Some(h) = head.first_chunk::<HEADER_LEN>() else {
            return Err(LoadError::Length {
                want: HEADER_LEN + payload_len(HIDDEN),
                got: head.len(),
            });
        };
        let word = |at: usize| u16::from_le_bytes([h[at], h[at + 1]]);
        if word(4) != FORMAT {
            return Err(LoadError::Format(word(4)));
        }
        let hidden @ (WIDE | HIDDEN) = usize::from(word(WIDTH_AT)) else {
            return Err(LoadError::Width(word(WIDTH_AT)));
        };
        let layout = h[INPUTS_AT];
        let inputs = Inputs::named(layout).ok_or(LoadError::Inputs(layout))?;
        Ok((inputs, hidden))
    }

    /// The loader for the net a build ships, of either width: `None` for no bytes.
    #[must_use]
    pub fn shipped(bytes: &[u8]) -> Option<Result<AnyNetwork, LoadError>> {
        (!bytes.is_empty()).then(|| AnyNetwork::for_datagen(bytes))
    }

    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        match self {
            AnyNetwork::Narrow(net) => net.provenance(),
            AnyNetwork::Wide(net) => net.provenance(),
        }
    }

    #[must_use]
    pub fn trainer(&self) -> String {
        match self {
            AnyNetwork::Narrow(net) => net.trainer(),
            AnyNetwork::Wide(net) => net.trainer(),
        }
    }

    #[must_use]
    pub fn manifest(&self) -> [u8; 32] {
        match self {
            AnyNetwork::Narrow(net) => net.manifest(),
            AnyNetwork::Wide(net) => net.manifest(),
        }
    }

    /// The units a perspective, which the header names.
    #[must_use]
    pub fn hidden(&self) -> usize {
        match self {
            AnyNetwork::Narrow(_) => HIDDEN,
            AnyNetwork::Wide(_) => WIDE,
        }
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            AnyNetwork::Narrow(net) => net.to_bytes(),
            AnyNetwork::Wide(net) => net.to_bytes(),
        }
    }
}

impl From<Network<HIDDEN>> for AnyNetwork {
    fn from(net: Network<HIDDEN>) -> AnyNetwork {
        AnyNetwork::Narrow(Arc::new(net))
    }
}

impl From<Network<WIDE>> for AnyNetwork {
    fn from(net: Network<WIDE>) -> AnyNetwork {
        AnyNetwork::Wide(Arc::new(net))
    }
}

impl From<Arc<Network<HIDDEN>>> for AnyNetwork {
    fn from(net: Arc<Network<HIDDEN>>) -> AnyNetwork {
        AnyNetwork::Narrow(net)
    }
}

impl From<Arc<Network<WIDE>>> for AnyNetwork {
    fn from(net: Arc<Network<WIDE>>) -> AnyNetwork {
        AnyNetwork::Wide(net)
    }
}

fn zeroed_columns<const H: usize>(rows: usize) -> Box<[[i16; H]]> {
    vec![[0; H]; rows].into_boxed_slice()
}

fn fill(cells: &mut [i16], mut next: impl FnMut() -> i16) {
    for cell in cells {
        *cell = next();
    }
}
