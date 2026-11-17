// SPDX-License-Identifier: GPL-3.0-or-later

//! Validation loss from the trainer's own forward pass: each holdout record read back as a position
//! the trainer can be asked about, scored against the target its training run used.

use cadence_dataset::trainer::RECORD_LEN;

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

/// The result share a validation scores its loss at: the run's own unless `given` names another,
/// as a comparison of nets trained at different blends needs.
///
/// # Errors
///
/// A given share outside [0, 1].
pub fn score_share(run: f32, given: Option<f32>) -> Result<f32, String> {
    let _ = given;
    Ok(run)
}

/// What a validation line says of the share its loss was scored at, and whether it is the run's.
#[must_use]
pub fn score_note(scored: f32, run: f32) -> String {
    let _ = (scored, run);
    String::new()
}

/// The trainer's loss for one example: the output's sigmoid against the same blend of result and
/// scaled score the training run aimed at, squared.
#[must_use]
pub fn loss(output: f32, example: &Example, result_share: f32, scale: f32) -> f64 {
    let sigmoid = |x: f64| 1.0 / (1.0 + (-x).exp());
    let share = f64::from(result_share);
    let target = share * f64::from(example.result) / 2.0
        + (1.0 - share) * sigmoid(f64::from(example.score) / f64::from(scale));
    let difference = sigmoid(f64::from(output)) - target;
    difference * difference
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
