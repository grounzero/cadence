// SPDX-License-Identifier: GPL-3.0-or-later

//! The game record: one game per line, fields separated by ` | `, in game-number order. A record
//! holds what replays the game and not the positions, so filtering again never generates again.

use std::fmt::Write as _;

use cadence_core::chess960::{ARRAYS, dfrc_fen};
use cadence_core::position::Board;
use cadence_core::{Move, generate_legal, parse_uci};

use super::game::{Ending, Outcome, Played};
use super::opening::Opening;
use crate::position::Position;
use crate::score::Score;

/// One game as the file holds it: number, arrays, random plies, moves each with its score, result
/// and ending, and the starts the screen refused before this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub number: u64,
    pub white: u32,
    pub black: u32,
    pub plies: Vec<Move>,
    pub moves: Vec<(Move, Score)>,
    pub outcome: Outcome,
    pub ending: Ending,
    pub refused: u32,
}

impl Record {
    #[must_use]
    pub fn new(number: u64, opening: &Opening, played: Played, refused: u32) -> Record {
        Record {
            number,
            white: opening.white,
            black: opening.black,
            plies: opening.plies.clone(),
            moves: played.moves,
            outcome: played.outcome,
            ending: played.ending,
            refused,
        }
    }

    /// The start before the random plies.
    ///
    /// # Panics
    ///
    /// If the arrays do not make a legal start. [`Record::parse`] refuses such numbers first.
    #[must_use]
    pub fn start(&self) -> Position {
        let fen = dfrc_fen(self.white, self.black);
        Position::new(Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}")))
    }

    /// The line, without its newline. Moves are spelled king-takes-rook, so a castle reads the
    /// same in every start.
    #[must_use]
    pub fn line(&self) -> String {
        let plies: Vec<String> = self.plies.iter().map(|m| m.to_uci_chess960()).collect();
        let mut moves = String::new();
        for (m, score) in &self.moves {
            let _ = write!(moves, " {}:{score}", m.to_uci_chess960());
        }
        format!(
            "{} | {} {} | {} | {} | {} {} | {}",
            self.number,
            self.white,
            self.black,
            plies.join(" "),
            moves.trim_start(),
            self.outcome.text(),
            self.ending.word(),
            self.refused
        )
    }

    /// Reads a line [`Record::line`] wrote, replaying every move against the legal list so that a
    /// record that parses is a game the rules allow.
    ///
    /// # Errors
    ///
    /// A field missing or malformed, or a move that is not legal where it stands. The message
    /// says which and not where, which the caller knows.
    pub fn parse(line: &str) -> Result<Record, String> {
        let fields: Vec<&str> = line.split(" | ").collect();
        let [number, arrays, plies, moves, result, refused] = fields[..] else {
            return Err(format!("{} fields and not 6", fields.len()));
        };
        let number = number.trim().parse().map_err(|_| "bad game number")?;
        let (white, black) = arrays.split_once(' ').ok_or("bad arrays")?;
        let white: u32 = white.parse().map_err(|_| "bad white array")?;
        let black: u32 = black.parse().map_err(|_| "bad black array")?;
        if white >= ARRAYS || black >= ARRAYS {
            return Err("array number out of range".to_string());
        }
        let (outcome, ending) = result.split_once(' ').ok_or("bad result")?;
        let outcome = Outcome::from_text(outcome).ok_or("bad outcome")?;
        let ending = Ending::from_word(ending).ok_or("bad ending")?;
        let refused = refused.trim().parse().map_err(|_| "bad refused count")?;
        let mut record = Record {
            number,
            white,
            black,
            plies: Vec::new(),
            moves: Vec::new(),
            outcome,
            ending,
            refused,
        };
        let mut board = record.start();
        for text in plies.split_whitespace() {
            let m = legal_move(&board, text)?;
            board.play(m);
            record.plies.push(m);
        }
        for pair in moves.split_whitespace() {
            let (text, score) = pair.split_once(':').ok_or("a move without a score")?;
            let m = legal_move(&board, text)?;
            let score = score.parse().map_err(|_| "bad score")?;
            board.play(m);
            record.moves.push((m, score));
        }
        Ok(record)
    }
}

fn legal_move(board: &Board, text: &str) -> Result<Move, String> {
    parse_uci(&generate_legal(board), text).ok_or_else(|| format!("illegal move {text}"))
}
