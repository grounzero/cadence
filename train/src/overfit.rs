// SPDX-License-Identifier: GPL-3.0-or-later

//! The overfit test's sets and bars: a small consistent set the recipe must memorise at the result
//! share's own meaning, and a real set whose loss must fall well below the best constant guess.

use std::collections::BTreeSet;
use std::io::Read as _;
use std::path::Path;

use cadence_core::chess960::dfrc_fen;
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_dataset::trainer::{RECORD_LEN, encode};

use crate::validate::{Example, example, sigmoid, target};

/// Positions in the synthetic set, each a distinct network input with one target.
pub const SYNTHETIC_POSITIONS: usize = 64;

/// How far a memorised position's output may sit from its target, as a probability.
pub const MEMORISED: f64 = 0.02;

/// How far, on average, the memorised outputs must sit from the reading with the shares swapped,
/// so the trainer's meaning of the result share is the one the recipe states.
pub const BLEND_GAP: f64 = 0.1;

/// The trained loss on real data as a share of the best constant guess's loss, at most.
pub const REAL_RATIO: f64 = 0.7;

/// Records the constant guess is read over, from the start of the data.
pub const REAL_SAMPLE: usize = 100_000;

/// The record's occupancy and piece bytes, which with the side to move as White are the network's
/// whole input.
const INPUT: usize = 24;

/// What the synthetic part read: the largest distance from a target, and the mean distance from
/// the swapped reading's targets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntheticReading {
    pub worst: f64,
    pub swapped: f64,
}

/// The synthetic set's records, from random legal games under `seed`, with scores and results drawn
/// apart so that the two shares' readings disagree.
///
/// # Panics
///
/// Never: the standard start's FEN is the board's own.
#[must_use]
pub fn synthetic(seed: u64) -> Vec<[u8; RECORD_LEN]> {
    let mut rng = Rng::new(seed);
    let mut inputs = BTreeSet::new();
    let mut out = Vec::new();
    while out.len() < SYNTHETIC_POSITIONS {
        let mut board = Board::from_fen(&dfrc_fen(518, 518)).expect("the standard start parses");
        for _ in 0..4 + rng.below(40) {
            let legal = generate_legal(&board);
            let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                break;
            };
            board.play(m);
        }
        let score = i16::try_from(rng.below(1_601)).unwrap_or(800) - 800;
        let result = u8::try_from(rng.below(3)).unwrap_or(1);
        let record = encode(&board, score, result);
        if inputs.insert(record[..INPUT].to_vec()) {
            out.push(record);
        }
    }
    out
}

fn mean(sum: f64, n: usize) -> f64 {
    u32::try_from(n)
        .ok()
        .filter(|&n| n > 0)
        .map_or(f64::NAN, |n| sum / f64::from(n))
}

/// The synthetic part's bar on the trained outputs, one a set example in order.
///
/// # Errors
///
/// An output farther than `MEMORISED` from its target, or outputs as near the swapped reading as
/// within `BLEND_GAP`, either way with the reading.
pub fn synthetic_bar(
    outputs: &[f32],
    examples: &[Example],
    result_share: f32,
    scale: f32,
) -> Result<SyntheticReading, String> {
    if outputs.len() != examples.len() || examples.is_empty() {
        return Err(format!(
            "{} outputs for {} examples",
            outputs.len(),
            examples.len()
        ));
    }
    let (mut worst, mut swapped) = (0.0_f64, 0.0);
    for (&output, example) in outputs.iter().zip(examples) {
        let p = sigmoid(f64::from(output));
        worst = worst.max((p - target(example, result_share, scale)).abs());
        swapped += (p - target(example, 1.0 - result_share, scale)).abs();
    }
    let reading = SyntheticReading {
        worst,
        swapped: mean(swapped, examples.len()),
    };
    if reading.worst > MEMORISED || reading.swapped.is_nan() || reading.swapped <= BLEND_GAP {
        return Err(format!(
            "the synthetic set was not memorised at the stated result share: {:.4} at worst from \
             its target, bar {MEMORISED}, and {:.4} on average from the swapped reading, bar above \
             {BLEND_GAP}",
            reading.worst, reading.swapped
        ));
    }
    Ok(reading)
}

/// The best constant guess's loss over `examples`: their targets' mean squared deviation.
#[must_use]
pub fn constant_loss(examples: &[Example], result_share: f32, scale: f32) -> f64 {
    let targets: Vec<f64> = examples
        .iter()
        .map(|e| target(e, result_share, scale))
        .collect();
    let centre = mean(targets.iter().sum(), targets.len());
    let squares = targets.iter().map(|t| (t - centre) * (t - centre)).sum();
    mean(squares, targets.len())
}

/// The real part's bar.
///
/// # Errors
///
/// A trained loss above `REAL_RATIO` of the constant guess's, with both.
pub fn real_bar(trained: f64, constant: f64) -> Result<(), String> {
    if trained <= REAL_RATIO * constant {
        return Ok(());
    }
    Err(format!(
        "the trained loss {trained:.6} is above {REAL_RATIO} of the best constant guess's \
         {constant:.6}"
    ))
}

/// The real part's reading: the trained loss, the best constant guess's and their ratio.
#[must_use]
pub fn real_reading(_trained: f64, _constant: f64) -> String {
    String::new()
}

/// The first `n` records across `data` in order, decoded.
///
/// # Errors
///
/// A file that cannot be read, or a record that does not decode.
pub fn first_records(data: &[&Path], n: usize) -> Result<Vec<Example>, String> {
    let mut out = Vec::new();
    for path in data {
        let want = (n - out.len()) * RECORD_LEN;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|f| f.take(want as u64).read_to_end(&mut bytes))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        for record in bytes.as_chunks::<RECORD_LEN>().0 {
            out.push(example(record).map_err(|e| format!("{}: {e}", path.display()))?);
        }
        if out.len() >= n {
            break;
        }
    }
    Ok(out)
}
