// SPDX-License-Identifier: GPL-3.0-or-later

//! A tuning set's text, one position a line as a FEN and White's result in brackets, to the trainer's
//! records: the score zero, since such a set carries none, and the result the side to move's.

use cadence_core::Colour;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_dataset::trainer::{DRAW, LOSS, RECORD_LEN, WIN, encode};

/// What a conversion read, for its manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Read {
    pub positions: u64,
    pub white_to_move: u64,
    /// The side to move's losses, draws and wins.
    pub results: [u64; 3],
}

/// The board and White's result, or why the line is not a position.
///
/// # Errors
///
/// A line with no bracketed result, text after it, a result other than 1.0, 0.5 or 0.0, or a FEN
/// the board refuses.
pub fn parse_line(line: &str) -> Result<(Board, u8), String> {
    let open = line.rfind('[').ok_or("no bracketed result")?;
    let rest = &line[open + 1..];
    let close = rest.find(']').ok_or("no closing bracket")?;
    if !rest[close + 1..].trim().is_empty() {
        return Err("text after the result".to_string());
    }
    let white = match &rest[..close] {
        "1.0" => WIN,
        "0.5" => DRAW,
        "0.0" => LOSS,
        other => return Err(format!("result `{other}` is not 1.0, 0.5 or 0.0")),
    };
    let board = Board::from_fen(line[..open].trim()).map_err(|e| format!("FEN refused: {e:?}"))?;
    Ok((board, white))
}

/// Every line a record, shuffled from `seed` because the trainer's reader takes them in file order.
///
/// # Errors
///
/// The first line `parse_line` refuses, by its number; blank lines are skipped.
pub fn convert(text: &str, seed: u64) -> Result<(Vec<[u8; RECORD_LEN]>, Read), String> {
    let mut records = Vec::new();
    let mut read = Read::default();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let (board, white) = parse_line(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        let to_move = board.side_to_move();
        let result = if to_move == Colour::White {
            white
        } else {
            WIN - white
        };
        records.push(encode(&board, 0, result));
        read.positions += 1;
        read.white_to_move += u64::from(to_move == Colour::White);
        read.results[usize::from(result)] += 1;
    }
    let mut rng = Rng::new(seed);
    for i in (1..records.len()).rev() {
        records.swap(i, rng.below(i + 1));
    }
    Ok((records, read))
}
