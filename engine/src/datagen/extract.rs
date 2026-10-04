// SPDX-License-Identifier: GPL-3.0-or-later

//! Split into training and holdout by game number. A position is kept if its ply is inside the
//! window, it is not in check, its move is not noisy and its search reported no mate.

use std::io::{BufRead, Write};

use cadence_core::{FenStyle, PieceType};

use super::game::{Ending, Outcome};
use super::record::Record;
use crate::eval::{self, PHASE_MAX};
use crate::score;

/// The random plies not counted.
pub const FIRST_PLY: usize = 8;

pub const LAST_PLY: usize = 240;

/// In pawns; the ends hold everything past them.
pub const BALANCE_CAP: i32 = 9;

/// The last band holds everything past it.
pub const PLY_BAND: usize = 10;

pub const PLY_BANDS: usize = 30;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub games: u64,
    pub refused: u64,
    pub visited: u64,
    pub outside: u64,
    pub in_check: u64,
    pub noisy: u64,
    pub mate: u64,
    pub train: u64,
    pub holdout: u64,
    /// In the order `Ending::word` lists them.
    pub endings: [u64; 5],
    /// White wins, Black wins, draws.
    pub outcomes: [u64; 3],
    pub phase: Vec<u64>,
    /// Offset by [`BALANCE_CAP`].
    pub balance: Vec<u64>,
    /// The random plies not counted.
    pub ply: Vec<u64>,
    pub longest: usize,
}

/// Every `every`-th game by number goes to `holdout`. Comment lines go to both, so the data sets
/// carry the run's provenance.
///
/// # Errors
///
/// A record that does not parse, or an output that cannot be written.
pub fn extract(
    input: impl BufRead,
    train: &mut dyn Write,
    holdout: &mut dyn Write,
    every: u64,
) -> Result<Stats, String> {
    let mut stats = Stats {
        phase: vec![0; PHASE_MAX as usize + 1],
        balance: vec![0; 2 * BALANCE_CAP as usize + 1],
        ply: vec![0; PLY_BANDS],
        ..Stats::default()
    };
    for (i, line) in input.lines().enumerate() {
        let line = line.map_err(|e| format!("line {}: {e}", i + 1))?;
        if line.starts_with('#') {
            writeln!(train, "{line}").map_err(|e| e.to_string())?;
            writeln!(holdout, "{line}").map_err(|e| e.to_string())?;
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let record = Record::parse(&line).map_err(|e| format!("line {}: {e}", i + 1))?;
        let held = every > 0 && record.number % every == 0;
        if held {
            stats.holdout += positions(&record, &mut stats, holdout).map_err(|e| e.to_string())?;
        } else {
            stats.train += positions(&record, &mut stats, train).map_err(|e| e.to_string())?;
        }
    }
    Ok(stats)
}

fn positions(record: &Record, stats: &mut Stats, out: &mut dyn Write) -> std::io::Result<u64> {
    stats.games += 1;
    stats.refused += u64::from(record.refused);
    stats.endings[ending_index(record.ending)] += 1;
    stats.outcomes[match record.outcome {
        Outcome::White => 0,
        Outcome::Black => 1,
        Outcome::Draw => 2,
    }] += 1;
    stats.longest = stats.longest.max(record.moves.len());
    let result = record.outcome.text();
    let mut board = record.start();
    for &m in &record.plies {
        board.play(m);
    }
    let mut kept = 0;
    for (ply, &(m, score)) in record.moves.iter().enumerate() {
        stats.visited += 1;
        if !(FIRST_PLY..=LAST_PLY).contains(&ply) {
            stats.outside += 1;
        } else if board.in_check() {
            stats.in_check += 1;
        } else if m.is_noisy() {
            stats.noisy += 1;
        } else if score::is_mate(score) {
            stats.mate += 1;
        } else {
            writeln!(out, "{} | {result}", board.to_fen(FenStyle::Shredder))?;
            kept += 1;
            stats.phase[eval::phase(&board) as usize] += 1;
            stats.balance
                [(balance(&board).clamp(-BALANCE_CAP, BALANCE_CAP) + BALANCE_CAP) as usize] += 1;
            stats.ply[(ply / PLY_BAND).min(PLY_BANDS - 1)] += 1;
        }
        board.play(m);
    }
    Ok(kept)
}

fn balance(board: &cadence_core::position::Board) -> i32 {
    let values = [
        (PieceType::Pawn, 1),
        (PieceType::Knight, 3),
        (PieceType::Bishop, 3),
        (PieceType::Rook, 5),
        (PieceType::Queen, 9),
    ];
    let white = board.by_colour(cadence_core::Colour::White);
    values
        .iter()
        .map(|&(pt, v)| {
            let all = board.by_type(pt);
            let ours = (all & white).count();
            let theirs = all.count() - ours;
            v * (i32::try_from(ours).unwrap_or(0) - i32::try_from(theirs).unwrap_or(0))
        })
        .sum()
}

fn ending_index(ending: Ending) -> usize {
    match ending {
        Ending::Mate => 0,
        Ending::Stalemate => 1,
        Ending::Repetition => 2,
        Ending::FiftyMoves => 3,
        Ending::InsufficientMaterial => 4,
    }
}

impl Stats {
    /// # Errors
    ///
    /// If `out` cannot be written.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a position count is far below 2^52"
    )]
    pub fn report(&self, out: &mut dyn Write) -> std::io::Result<()> {
        let kept = self.train + self.holdout;
        let per_game = |n: u64| n as f64 / self.games.max(1) as f64;
        writeln!(out, "games {} refused-starts {}", self.games, self.refused)?;
        writeln!(
            out,
            "endings mate {} stalemate {} repetition {} fifty {} material {}",
            self.endings[0], self.endings[1], self.endings[2], self.endings[3], self.endings[4]
        )?;
        writeln!(
            out,
            "outcomes white {} black {} draw {}",
            self.outcomes[0], self.outcomes[1], self.outcomes[2]
        )?;
        writeln!(
            out,
            "plies mean {:.1} longest {}",
            per_game(self.visited),
            self.longest
        )?;
        writeln!(
            out,
            "visited {} outside-plies-{FIRST_PLY}-{LAST_PLY} {} in-check {} noisy {} mate {}",
            self.visited, self.outside, self.in_check, self.noisy, self.mate
        )?;
        writeln!(
            out,
            "kept {kept} train {} holdout {} per-game {:.1} yield {:.3}",
            self.train,
            self.holdout,
            per_game(kept),
            kept as f64 / self.visited.max(1) as f64
        )?;
        let share = |n: u64| 100.0 * n as f64 / kept.max(1) as f64;
        writeln!(out, "phase (0 bare, {PHASE_MAX} full)")?;
        for (p, &n) in self.phase.iter().enumerate() {
            writeln!(out, "  {p:>3} {n:>10} {:>6.2}%", share(n))?;
        }
        writeln!(out, "balance (White's lead in pawns)")?;
        for (i, &n) in self.balance.iter().enumerate() {
            let lead = i32::try_from(i).unwrap_or(i32::MAX) - BALANCE_CAP;
            let edge = if lead.abs() == BALANCE_CAP { "+" } else { " " };
            writeln!(out, "  {lead:>+3}{edge} {n:>10} {:>6.2}%", share(n))?;
        }
        writeln!(out, "ply of the game proper")?;
        for (i, &n) in self.ply.iter().enumerate() {
            let from = i * PLY_BAND;
            let band = if i + 1 == PLY_BANDS {
                format!("{from}+")
            } else {
                format!("{from}-{}", from + PLY_BAND - 1)
            };
            writeln!(out, "  {band:>7} {n:>10} {:>6.2}%", share(n))?;
        }
        Ok(())
    }
}
