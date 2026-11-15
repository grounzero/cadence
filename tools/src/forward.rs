// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's quantised forward pass against an f32 one written here from the file alone, over
//! random legal positions; with `--raw`, against the trainer's unquantised weights as well.

use std::path::Path;
use std::process::ExitCode;

use cadence_core::chess960::dfrc_fen;
use cadence_core::features::{NUM_INPUTS, feature_index};
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_nnue::net::{HEADER_LEN, PAYLOAD_LEN, QA, QB, SCALE};
use cadence_nnue::{HIDDEN, Network};

/// The fewest positions a reading may rest on.
pub const MIN_POSITIONS: usize = 1_000;
/// The largest difference allowed at any position, in centipawns.
pub const MAX_DELTA: f64 = 5.0;
/// The largest mean difference allowed, in centipawns.
pub const MEAN_DELTA: f64 = 1.0;
const DEFAULT_SEED: u64 = 0x2026_1115_f0f0_0001;
/// Random plies from each start, so the positions run from the opening into the middlegame.
const LONGEST_WALK: usize = 120;

const USAGE: &str = "forward --net FILE [--raw FILE] [--positions N] [--seed HEX]";

/// Weights in the file's order, as numbers in the units the trained network used.
struct Reference {
    columns: Vec<[f32; HIDDEN]>,
    bias: [f32; HIDDEN],
    output: [[f32; HIDDEN]; 2],
    output_bias: f32,
}

impl Reference {
    /// `words` in the file's order, each divided by the scale it was saved at.
    fn from_words(words: &[f32], feature: f32, output: f32, output_bias: f32) -> Reference {
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
        }
    }

    /// The quantised file read as numbers: features at `QA`, outputs at `QB`, the output bias at both.
    fn quantised(file: &[u8]) -> Reference {
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
    fn raw(bytes: &[u8]) -> Result<Reference, String> {
        let want = PAYLOAD_LEN * 2;
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
            for sq in board.occupied() {
                if let Some(piece) = board.piece_at(sq) {
                    let column = &self.columns[feature_index(persp, piece, sq)];
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
pub fn deltas(net: &Network, reference: impl Fn(&Board) -> f64, boards: &[Board]) -> (f64, f64) {
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

/// # Errors
///
/// A missing or unreadable file, one the engine refuses, fewer than [`MIN_POSITIONS`], or a flag
/// it does not know.
pub fn run(args: &[String]) -> Result<ExitCode, String> {
    let (mut net_path, mut raw_path, mut count, mut seed) =
        (None, None, MIN_POSITIONS, DEFAULT_SEED);
    let mut words = args.iter();
    while let Some(flag) = words.next() {
        let value = words
            .next()
            .ok_or_else(|| format!("{flag} needs a value; usage: {USAGE}"))?;
        match flag.as_str() {
            "--net" => net_path = Some(value.clone()),
            "--raw" => raw_path = Some(value.clone()),
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
    let read = |p: &str| std::fs::read(Path::new(p)).map_err(|e| format!("{p}: {e}"));
    let file = read(&net_path)?;
    let net = Network::from_bytes(&file).map_err(|e| format!("{net_path}: {e}"))?;
    let boards = positions(count, seed);
    let mut readings = vec![("the net file", Reference::quantised(&file))];
    if let Some(raw_path) = raw_path {
        let raw = Reference::raw(&read(&raw_path)?).map_err(|e| format!("{raw_path}: {e}"))?;
        readings.push(("the raw weights", raw));
    }
    let mut passed = true;
    for (name, reference) in &readings {
        let (largest, mean) = deltas(&net, |b| reference.evaluate(b), &boards);
        let pass = largest <= MAX_DELTA && mean <= MEAN_DELTA;
        passed &= pass;
        println!(
            "forward: against {name}, {count} positions from seed {seed:016x}: max |delta| \
             {largest:.3} cp, mean {mean:.3} cp, {} (bounds {MAX_DELTA} and {MEAN_DELTA})",
            if pass { "PASS" } else { "FAIL" }
        );
    }
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
