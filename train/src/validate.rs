// SPDX-License-Identifier: GPL-3.0-or-later

//! Validation loss from the trainer's own forward pass: each holdout record read back as a position
//! the trainer can be asked about, scored against the target its training run used.

use cadence_core::position::Board;
use cadence_dataset::trainer::RECORD_LEN;
use cadence_nnue::net::PAYLOAD_LEN;
use cadence_nnue::{AccumulatorStack, Network, Provenance};

/// A trainer commit for a load that names none, since a checkpoint is not yet given one.
const UNNAMED: &str = "0000000000000000000000000000000000000000";

/// One holdout record: its position with the side to move as White, which is how a record stores
/// it, and that side's score and result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Example {
    pub fen: String,
    pub score: i16,
    /// 0 loss, 1 draw, 2 win.
    pub result: u8,
}

/// The FEN letters of the six piece types, in the record's order.
const LETTERS: [char; 6] = ['P', 'N', 'B', 'R', 'Q', 'K'];

/// A record decoded: occupancy, a nibble a piece in square order with bit 3 for the other side's,
/// then the score and the result.
///
/// # Errors
///
/// A nibble naming no piece, more than 32 pieces, or a result past 2.
pub fn example(record: &[u8; RECORD_LEN]) -> Result<Example, String> {
    let mut occupancy = [0; 8];
    occupancy.copy_from_slice(&record[..8]);
    let occupancy = u64::from_le_bytes(occupancy);
    if occupancy.count_ones() > 32 {
        return Err(format!("{} pieces", occupancy.count_ones()));
    }
    let mut squares = [None; 64];
    let mut n = 0;
    for (sq, cell) in squares.iter_mut().enumerate() {
        if occupancy >> sq & 1 == 1 {
            let nibble = record[8 + n / 2] >> (4 * (n % 2)) & 0xf;
            let letter = *LETTERS
                .get(usize::from(nibble & 7))
                .ok_or_else(|| format!("piece nibble {nibble}"))?;
            *cell = Some(if nibble & 8 == 0 {
                letter
            } else {
                letter.to_ascii_lowercase()
            });
            n += 1;
        }
    }
    let mut placement = String::new();
    for rank in (0..8).rev() {
        let mut empty = 0;
        for file in 0..8 {
            match squares[8 * rank + file] {
                Some(letter) => {
                    if empty > 0 {
                        placement.push_str(&empty.to_string());
                        empty = 0;
                    }
                    placement.push(letter);
                }
                None => empty += 1,
            }
        }
        if empty > 0 {
            placement.push_str(&empty.to_string());
        }
        if rank > 0 {
            placement.push('/');
        }
    }
    let result = record[26];
    if result > 2 {
        return Err(format!("result {result}"));
    }
    Ok(Example {
        fen: format!("{placement} w - - 0 1"),
        score: i16::from_le_bytes([record[24], record[25]]),
        result,
    })
}

#[must_use]
pub fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// The target a training run aims at: `result_share` of it from the game's result, the rest from
/// the score scaled and squashed.
#[must_use]
pub fn target(example: &Example, result_share: f32, scale: f32) -> f64 {
    let share = f64::from(result_share);
    share * f64::from(example.result) / 2.0
        + (1.0 - share) * sigmoid(f64::from(example.score) / f64::from(scale))
}

/// The trainer's loss for one example: the output's sigmoid against the same blend of result and
/// scaled score the training run aimed at, squared.
#[must_use]
pub fn loss(output: f32, example: &Example, result_share: f32, scale: f32) -> f64 {
    let difference = sigmoid(f64::from(output)) - target(example, result_share, scale);
    difference * difference
}

/// How far a quantised checkpoint's loss may sit from the trainer's own over the same positions,
/// as a share of the trainer's: rounding moves it far less, and a misread export far more.
pub const QUANTISED_MARGIN: f64 = 0.02;

/// Added to the margin, so a loss near zero is not held to the rounding of its own arithmetic.
pub const QUANTISED_FLOOR: f64 = 1e-6;

/// The loss of a checkpoint's quantised weights over `examples`, scored by the engine's own integer
/// evaluation, held to `QUANTISED_MARGIN` of the trainer's loss `trainer` over the same examples.
///
/// # Errors
///
/// Weights the engine's loader refuses, a position that does not parse, or a loss outside the bar.
pub fn quantised_check(
    quantised: &[u8],
    examples: &[Example],
    result_share: f32,
    scale: f32,
    trainer: f64,
) -> Result<f64, String> {
    let payload = quantised
        .get(..PAYLOAD_LEN)
        .ok_or("shorter than a net's weights")?;
    let unnamed = Provenance::External("checkpoint".to_string());
    let net = Network::from_payload(payload, unnamed, UNNAMED, [0; 32])
        .map_err(|e| format!("the engine's loader refuses it: {e}"))?;
    let mut sum = 0.0;
    for example in examples {
        let board = Board::from_fen(&example.fen).map_err(|e| format!("{}: {e:?}", example.fen))?;
        let stack = AccumulatorStack::new(&net, &board);
        let cp = net.evaluate(stack.current(), board.side_to_move());
        let output = (f64::from(cp) / f64::from(scale)) as f32;
        sum += loss(output, example, result_share, scale);
    }
    let n = u32::try_from(examples.len())
        .ok()
        .filter(|&n| n > 0)
        .ok_or("no example to score")?;
    let loss = sum / f64::from(n);
    if (loss - trainer).abs() > QUANTISED_MARGIN * trainer + QUANTISED_FLOOR {
        return Err(format!(
            "the quantised loss {loss:.6} is outside {QUANTISED_MARGIN} of the trainer's \
             {trainer:.6}, so the export may be misread"
        ));
    }
    Ok(loss)
}

/// The mean of a checkpoint's logged training losses for its own superbatch, from the
/// `superbatch,batch,loss` lines the trainer writes beside it.
#[must_use]
pub fn training_loss(log: &str, superbatch: usize) -> Option<f64> {
    let losses: Vec<f64> = log
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(',');
            let sb: usize = fields.next()?.trim().parse().ok()?;
            let _batch = fields.next()?;
            let loss: f64 = fields.next()?.trim().parse().ok()?;
            (sb == superbatch).then_some(loss)
        })
        .collect();
    let n = u32::try_from(losses.len()).ok().filter(|&n| n > 0)?;
    Some(losses.iter().sum::<f64>() / f64::from(n))
}
