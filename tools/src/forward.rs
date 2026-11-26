// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's quantised forward pass against an f32 one written here from the net file alone,
//! which decides the exit; `--raw` adds a training-time reading against the trainer's unquantised
//! weights that decides nothing.

use std::path::Path;
use std::process::ExitCode;

use cadence_core::chess960::dfrc_fen;
use cadence_core::features::{NUM_INPUTS, feature_index, mirrored_feature_index};
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_nnue::net::{HEADER_LEN, QA, QB, SCALE, payload_len};
use cadence_nnue::{AnyNetwork, Inputs, Network};

/// The fewest positions a reading may rest on.
pub const MIN_POSITIONS: usize = 1_000;
/// The largest difference allowed at any position, in centipawns.
pub const MAX_DELTA: f64 = 5.0;
/// The largest mean difference allowed, in centipawns.
pub const MEAN_DELTA: f64 = 1.0;
const DEFAULT_SEED: u64 = 0x2026_1115_f0f0_0001;
/// Random plies from each start, so the positions run from the opening into the middlegame.
const LONGEST_WALK: usize = 120;

const USAGE: &str = "forward --net FILE [--raw FILE [--against FILE] [--scale-factor F [--unscaled \
                     FILE]] [--correct-bias OUT]] [--positions N] [--seed HEX] [--fens FILE]";

/// Weights in the file's order, as numbers in the units the trained network used, `H` a perspective.
struct Reference<const H: usize> {
    columns: Vec<[f32; H]>,
    bias: [f32; H],
    output: [[f32; H]; 2],
    output_bias: f32,
    /// The layout the net file's header names, which the reference indexes by as the engine does.
    inputs: Inputs,
}

impl<const H: usize> Reference<H> {
    /// `words` in the file's order, each divided by the scale it was saved at.
    fn from_words(words: &[f32], feature: f32, output: f32, output_bias: f32) -> Reference<H> {
        let mut at = words.iter().copied();
        let mut take = |scale: f32| at.next().unwrap_or(0.0) / scale;
        let columns = (0..NUM_INPUTS)
            .map(|_| core::array::from_fn(|_| take(feature)))
            .collect();
        let bias = core::array::from_fn(|_| take(feature));
        let output = core::array::from_fn(|_| core::array::from_fn(|_| take(output)));
        Reference {
            columns,
            bias,
            output,
            output_bias: take(output_bias),
            inputs: Inputs::Plain,
        }
    }

    /// The same weights read under the layout a net's header names.
    fn laid_out(mut self, inputs: Inputs) -> Reference<H> {
        self.inputs = inputs;
        self
    }

    /// The quantised file read as numbers: features at `QA`, outputs at `QB`, the output bias at both.
    fn quantised(file: &[u8]) -> Reference<H> {
        let words: Vec<f32> = file[HEADER_LEN..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&w| f32::from(i16::from_le_bytes(w)))
            .collect();
        let (qa, qb) = (scale(QA), scale(QB));
        Reference::from_words(&words, qa, qb, qa * qb)
    }

    /// The trainer's unquantised file, the same order in `f32` with no padding.
    fn raw(bytes: &[u8]) -> Result<Reference<H>, String> {
        let want = payload_len(H) * 2;
        if bytes.len() != want {
            return Err(format!("{} bytes where a raw file is {want}", bytes.len()));
        }
        let words: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&w| f32::from_le_bytes(w))
            .collect();
        Ok(Reference::from_words(&words, 1.0, 1.0, 1.0))
    }

    /// Centipawns from the mover's view: each sum clamped to [0, 1] and squared, then weighted.
    fn evaluate(&self, board: &Board) -> f64 {
        let mover = board.side_to_move();
        let mut out = self.output_bias;
        for (persp, weights) in [mover, mover.flip()].into_iter().zip(&self.output) {
            let mut sums = self.bias;
            let king = board.king_square(persp);
            for sq in board.occupied() {
                if let Some(piece) = board.piece_at(sq) {
                    let index = match self.inputs {
                        Inputs::Plain => feature_index(persp, piece, sq),
                        Inputs::Mirrored => mirrored_feature_index(persp, king, piece, sq),
                    };
                    let column = &self.columns[index];
                    for (s, w) in sums.iter_mut().zip(column) {
                        *s += w;
                    }
                }
            }
            for (s, w) in sums.iter().zip(weights) {
                let c = s.clamp(0.0, 1.0);
                out += c * c * w;
            }
        }
        f64::from(out) * f64::from(SCALE)
    }
}

fn scale(q: i32) -> f32 {
    f32::from(i16::try_from(q).unwrap_or(i16::MAX))
}

/// Random legal positions, each the end of a random walk from a random DFRC start.
#[must_use]
pub fn positions(count: usize, seed: u64) -> Vec<Board> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(count);
    while out.len() < count {
        let (white, black) = (rng.below(960), rng.below(960));
        let fen = dfrc_fen(white as u32, black as u32);
        let Ok(mut board) = Board::from_fen(&fen) else {
            continue;
        };
        for _ in 0..rng.below(LONGEST_WALK + 1) {
            let legal = generate_legal(&board);
            let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                break;
            };
            board.play(m);
        }
        if !generate_legal(&board).is_empty() {
            out.push(board);
        }
    }
    out
}

/// The largest and the mean difference, in centipawns.
#[must_use]
pub fn deltas<const H: usize>(
    net: &Network<H>,
    reference: impl Fn(&Board) -> f64,
    boards: &[Board],
) -> (f64, f64) {
    let mut largest = 0.0_f64;
    let mut total = 0.0;
    for board in boards {
        let engine = net.evaluate(&net.refresh(board), board.side_to_move());
        let delta = (f64::from(engine) - reference(board)).abs();
        largest = largest.max(delta);
        total += delta;
    }
    let n = u32::try_from(boards.len()).unwrap_or(u32::MAX);
    (largest, total / f64::from(n.max(1)))
}

/// What a net's differences from a reference come to over a set of positions, in centipawns.
#[derive(Clone, Copy, Debug)]
pub struct Spread {
    /// The largest difference of either sign.
    pub largest: f64,
    /// The mean size of a difference.
    pub mean: f64,
    /// The mean difference with its sign, the part common to every position.
    pub offset: f64,
    /// The mean size of a difference once that common part is taken out.
    pub about_offset: f64,
}

/// The engine's evaluation less the reference's at each position, summed up as a `Spread`.
#[must_use]
pub fn spread<const H: usize>(
    net: &Network<H>,
    reference: impl Fn(&Board) -> f64,
    boards: &[Board],
) -> Spread {
    let signed: Vec<f64> = boards
        .iter()
        .map(|b| f64::from(net.evaluate(&net.refresh(b), b.side_to_move())) - reference(b))
        .collect();
    let n = f64::from(u32::try_from(signed.len()).unwrap_or(u32::MAX).max(1));
    let offset = signed.iter().sum::<f64>() / n;
    Spread {
        largest: signed.iter().fold(0.0, |m, d| m.max(d.abs())),
        mean: signed.iter().map(|d| d.abs()).sum::<f64>() / n,
        offset,
        about_offset: signed.iter().map(|d| (d - offset).abs()).sum::<f64>() / n,
    }
}

/// Positions read from a file, one a line: a FEN, or its first four fields as an opening book
/// gives them.
///
/// # Errors
///
/// A line the board does not accept, named by its number.
pub fn listed(text: &str) -> Result<Vec<Board>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() {
            continue;
        }
        let fen = match fields.len() {
            n if n >= 6 && fields[4].parse::<u32>().is_ok() => fields[..6].join(" "),
            n if n >= 4 => format!("{} 0 1", fields[..4].join(" ")),
            _ => return Err(format!("line {}: not a position", i + 1)),
        };
        out.push(Board::from_fen(&fen).map_err(|e| format!("line {}: {e:?}", i + 1))?);
    }
    Ok(out)
}

/// The trainer's own outputs, one `centipawns<TAB>FEN` a line, against the raw weights' reference
/// and the net file's integer path at the same positions; a reading, which decides nothing.
///
/// # Errors
///
/// A line that is not a number, a tab and a FEN the board accepts.
fn against<const H: usize>(
    net: &Network<H>,
    raw: &Reference<H>,
    text: &str,
) -> Result<String, String> {
    let (mut n, mut raw_sum, mut raw_max, mut net_sum, mut net_max) =
        (0_u32, 0.0, 0.0_f64, 0.0, 0.0_f64);
    let mut worst = (0.0_f64, String::new());
    for (i, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let (cp, fen) = line
            .split_once('\t')
            .ok_or_else(|| format!("line {}: no tab", i + 1))?;
        let trainer: f64 = cp
            .trim()
            .parse()
            .map_err(|_| format!("line {}: `{cp}`", i + 1))?;
        let board = Board::from_fen(fen.trim()).map_err(|e| format!("line {}: {e:?}", i + 1))?;
        let reference = raw.evaluate(&board);
        let engine = f64::from(net.evaluate(&net.refresh(&board), board.side_to_move()));
        let (dr, dn) = ((trainer - reference).abs(), (trainer - engine).abs());
        if dr > worst.0 {
            worst = (
                dr,
                format!(
                    "{} (trainer {trainer:.2} cp, reference {reference:.2} cp)",
                    fen.trim()
                ),
            );
        }
        raw_max = raw_max.max(dr);
        net_max = net_max.max(dn);
        raw_sum += dr;
        net_sum += dn;
        n += 1;
    }
    if n == 0 {
        return Err("no positions".to_string());
    }
    let count = f64::from(n);
    Ok(format!(
        "forward: against the trainer's own evaluation of {n} positions, a reading that decides \
         nothing: the raw weights' reference max |delta| {raw_max:.3} cp, mean {:.3} cp; the net \
         file's integer path max |delta| {net_max:.3} cp, mean {:.3} cp\n  largest reference \
         difference: {}",
        raw_sum / count,
        net_sum / count,
        worst.1
    ))
}

/// # Errors
///
/// A missing or unreadable file, one the engine refuses, fewer than [`MIN_POSITIONS`], or a flag
/// it does not know.
pub fn run(args: &[String]) -> Result<ExitCode, String> {
    let (mut net_path, mut raw_path, mut count, mut seed) =
        (None, None, MIN_POSITIONS, DEFAULT_SEED);
    let (mut against_path, mut factor, mut unscaled_path) = (None, None, None);
    let (mut fens_path, mut corrected_path) = (None, None);
    let mut words = args.iter();
    while let Some(flag) = words.next() {
        let value = words
            .next()
            .ok_or_else(|| format!("{flag} needs a value; usage: {USAGE}"))?;
        match flag.as_str() {
            "--net" => net_path = Some(value.clone()),
            "--raw" => raw_path = Some(value.clone()),
            "--against" => against_path = Some(value.clone()),
            "--unscaled" => unscaled_path = Some(value.clone()),
            "--fens" => fens_path = Some(value.clone()),
            "--correct-bias" => corrected_path = Some(value.clone()),
            "--scale-factor" => match value.parse::<f64>() {
                Ok(f) if f.is_finite() && f > 0.0 => factor = Some(f),
                _ => return Err(format!("--scale-factor {value}, where a factor is above 0")),
            },
            "--positions" => count = value.parse().map_err(|_| format!("--positions {value}"))?,
            "--seed" => {
                seed = u64::from_str_radix(value, 16).map_err(|_| format!("--seed {value}"))?;
            }
            other => return Err(format!("unknown flag {other}; usage: {USAGE}")),
        }
    }
    let net_path = net_path.ok_or_else(|| format!("--net is needed; usage: {USAGE}"))?;
    if count < MIN_POSITIONS {
        return Err(format!(
            "{count} positions, where a reading needs {MIN_POSITIONS}"
        ));
    }
    if corrected_path.is_some() && raw_path.is_none() {
        return Err("--correct-bias reads --raw, the weights the offset is measured from".into());
    }
    let boards = match &fens_path {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let boards = listed(&text).map_err(|e| format!("{path}: {e}"))?;
            if boards.len() < MIN_POSITIONS {
                return Err(format!(
                    "{path}: {} positions, where a reading needs {MIN_POSITIONS}",
                    boards.len()
                ));
            }
            boards
        }
        None => positions(count, seed),
    };
    let read = |p: &str| std::fs::read(Path::new(p)).map_err(|e| format!("{p}: {e}"));
    let file = read(&net_path)?;
    let net = AnyNetwork::from_bytes(&file).map_err(|e| format!("{net_path}: {e}"))?;
    let raw = match raw_path {
        Some(p) => Some((read(&p)?, p)),
        None => None,
    };
    let against = match against_path {
        Some(path) => {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
            Some((text, path))
        }
        None => None,
    };
    let unscaled = match unscaled_path {
        Some(p) => Some((read(&p)?, p)),
        None => None,
    };
    let reading = Reading {
        factor,
        unscaled: unscaled
            .as_ref()
            .map(|(bytes, p)| (bytes.as_slice(), p.as_str())),
        file: &file,
        boards: &boards,
        listed: fens_path.as_deref(),
        corrected: corrected_path.as_deref(),
        raw: raw
            .as_ref()
            .map(|(bytes, p)| (bytes.as_slice(), p.as_str())),
        against: against
            .as_ref()
            .map(|(text, p)| (text.as_str(), p.as_str())),
        seed,
    };
    let passed = match net {
        AnyNetwork::Narrow(net) => reading.report(&net)?,
        AnyNetwork::Wide(net) => reading.report(&net)?,
    };
    println!(
        "  not examined: positions in check or late endgames beyond a {LONGEST_WALK}-ply walk"
    );
    println!("  not examined: the feature index, which the train crate's index test compares");
    Ok(if passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// What one `forward` run reads, apart from the net, whose width decides the reference's.
struct Reading<'a> {
    file: &'a [u8],
    boards: &'a [Board],
    raw: Option<(&'a [u8], &'a str)>,
    against: Option<(&'a str, &'a str)>,
    seed: u64,
    /// What the net's output layer was scaled by at export, the raw weights' evaluation times it
    /// being what the net is then read against.
    factor: Option<f64>,
    /// The same checkpoint's export without the factor, which the scaled net's error is held to.
    unscaled: Option<(&'a [u8], &'a str)>,
    /// The file the positions were read from, where they were not drawn from the seed.
    listed: Option<&'a str>,
    /// Where to write the net with its output bias moved against the common offset.
    corrected: Option<&'a str>,
}

impl Reading<'_> {
    /// Prints each comparison and answers whether the net file's agreement passed.
    fn report<const H: usize>(&self, net: &Network<H>) -> Result<bool, String> {
        let raw = match self.raw {
            Some((bytes, p)) => Some(
                Reference::<H>::raw(bytes)
                    .map_err(|e| format!("{p}: {e}"))?
                    .laid_out(net.inputs()),
            ),
            None => None,
        };
        if self.factor.is_some() && (raw.is_none() || self.against.is_some()) {
            return Err("--scale-factor reads --raw, and not --against".to_string());
        }
        if self.unscaled.is_some() && self.factor.is_none() {
            return Err("--unscaled is read with --scale-factor".to_string());
        }
        if let Some((text, path)) = self.against {
            let raw = raw
                .as_ref()
                .ok_or("--against needs --raw, the unquantised weights the trainer evaluated")?;
            println!(
                "{}",
                against(net, raw, text).map_err(|e| format!("{path}: {e}"))?
            );
        }
        let within = |largest: f64, mean: f64| largest <= MAX_DELTA && mean <= MEAN_DELTA;
        let quantised = Reference::<H>::quantised(self.file).laid_out(net.inputs());
        let (largest, mean) = deltas(net, |b| quantised.evaluate(b), self.boards);
        let passed = within(largest, mean);
        let source = match self.listed {
            Some(path) => format!("from {path}"),
            None => format!("from seed {:016x}", self.seed),
        };
        println!(
            "forward: against the net file, {} positions {source}, hidden width {H}: max \
             |delta| {largest:.3} cp, mean {mean:.3} cp, {} (bounds {MAX_DELTA} and {MEAN_DELTA})",
            self.boards.len(),
            if passed { "PASS" } else { "FAIL" }
        );
        let Some(raw) = raw else {
            return Ok(passed);
        };
        let factor = self.factor.unwrap_or(1.0);
        let now = spread(net, |b| factor * raw.evaluate(b), self.boards);
        if let Some(path) = self.corrected {
            self.correct(net, &raw, factor, now.offset, path)?;
        }
        let Some((bytes, path)) = self.unscaled else {
            println!(
                "forward: against the raw weights{}, a training-time reading that decides \
                 nothing: max |delta| {:.3} cp, mean {:.3} cp, {} the bounds; a common offset of \
                 {:+.3} cp, and about it mean {:.3} cp",
                self.factor
                    .map_or_else(String::new, |f| format!(" times {f}")),
                now.largest,
                now.mean,
                if within(now.largest, now.mean) {
                    "within"
                } else {
                    "outside"
                },
                now.offset,
                now.about_offset
            );
            return Ok(passed);
        };
        let before = Network::<H>::load(bytes).map_err(|e| format!("{path}: {e}"))?;
        let was = spread(&before, |b| raw.evaluate(b), self.boards);
        // Scaling up multiplies the error the net had; scaling down may keep it and no more. The
        // average is read about each net's own common offset, which rounding draws afresh.
        let grown = factor.max(1.0);
        let kept = now.largest <= grown * was.largest + MAX_DELTA
            && now.about_offset <= grown * was.about_offset + MEAN_DELTA;
        println!(
            "forward: after scaling by {factor}, against the raw weights times it: max |delta| \
             {:.3} cp; a common offset of {:+.3} cp, and about it mean {:.3} cp, {} (the unscaled \
             net's {:.3}, and about its offset of {:+.3} its mean {:.3}{}, and at most {MAX_DELTA} \
             and {MEAN_DELTA} more); the plain mean is {:.3} cp, and an exact scaling would leave \
             {:.3} and {:.3}",
            now.largest,
            now.offset,
            now.about_offset,
            if kept { "PASS" } else { "FAIL" },
            was.largest,
            was.offset,
            was.about_offset,
            if factor > 1.0 {
                " times the factor"
            } else {
                ""
            },
            now.mean,
            factor * was.largest,
            factor * was.about_offset
        );
        Ok(passed && kept)
    }

    /// Writes the net with its output bias moved against `offset`, the mean of its evaluation
    /// less the raw weights' times the factor, and prints the offset the written net has left.
    fn correct<const H: usize>(
        &self,
        net: &Network<H>,
        raw: &Reference<H>,
        factor: f64,
        offset: f64,
        path: &str,
    ) -> Result<(), String> {
        let mut bytes = net.to_bytes();
        let at = bytes.len() - 2;
        let bias = i16::from_le_bytes([bytes[at], bytes[at + 1]]);
        // One unit of the bias is SCALE / (QA x QB) of a centipawn.
        let units = (-offset * f64::from(QA * QB) / f64::from(SCALE)).round();
        let moved = f64::from(bias) + units;
        if !(f64::from(i16::MIN)..=f64::from(i16::MAX)).contains(&moved) {
            return Err(format!(
                "an offset of {offset:+.3} cp would take the output bias to {moved}, outside 16 bits"
            ));
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "held inside i16 just above"
        )]
        let moved = moved as i16;
        bytes[at..].copy_from_slice(&moved.to_le_bytes());
        let corrected = Network::<H>::load(&bytes).map_err(|e| format!("{path}: {e}"))?;
        let after = spread(&corrected, |b| factor * raw.evaluate(b), self.boards);
        std::fs::write(Path::new(path), &bytes).map_err(|e| format!("{path}: {e}"))?;
        println!(
            "forward: output bias moved from {bias} to {moved}, {:+.3} cp, against a common offset \
             of {offset:+.3} cp; the net written to {path} has an offset of {:+.3} cp, max |delta| \
             {:.3} cp and mean {:.3} cp against the raw weights{}",
            f64::from(moved - bias) * f64::from(SCALE) / f64::from(QA * QB),
            after.offset,
            after.largest,
            after.mean,
            self.factor
                .map_or_else(String::new, |f| format!(" times {f}"))
        );
        Ok(())
    }
}
