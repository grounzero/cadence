// SPDX-License-Identifier: GPL-3.0-or-later

//! No adjudication and no ply cap: the fifty-move rule bounds a game, and a game with no result is
//! worse than a long one.

use std::io;
use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Colour, Move, generate_legal};

use crate::position::Position;
use crate::score::Score;
use crate::search::{Limits, Search};
use crate::tt::Table;

/// Every variant is a rule of chess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Mate,
    Stalemate,
    Repetition,
    FiftyMoves,
    InsufficientMaterial,
}

impl Ending {
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Ending::Mate => "mate",
            Ending::Stalemate => "stalemate",
            Ending::Repetition => "repetition",
            Ending::FiftyMoves => "fifty",
            Ending::InsufficientMaterial => "material",
        }
    }

    #[must_use]
    pub fn from_word(word: &str) -> Option<Ending> {
        [
            Ending::Mate,
            Ending::Stalemate,
            Ending::Repetition,
            Ending::FiftyMoves,
            Ending::InsufficientMaterial,
        ]
        .into_iter()
        .find(|e| e.word() == word)
    }
}

/// From White's point of view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    White,
    Black,
    Draw,
}

impl Outcome {
    /// What the tuner reads.
    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Outcome::White => "1-0",
            Outcome::Black => "0-1",
            Outcome::Draw => "1/2-1/2",
        }
    }

    #[must_use]
    pub fn from_text(text: &str) -> Option<Outcome> {
        [Outcome::White, Outcome::Black, Outcome::Draw]
            .into_iter()
            .find(|o| o.text() == text)
    }
}

/// Mate is read first: a mate on the hundredth quiet ply is still a mate.
#[must_use]
pub fn ending(board: &Board) -> Option<(Ending, Outcome)> {
    if generate_legal(board).is_empty() {
        if !board.in_check() {
            return Some((Ending::Stalemate, Outcome::Draw));
        }
        let winner = match board.side_to_move() {
            Colour::White => Outcome::Black,
            Colour::Black => Outcome::White,
        };
        return Some((Ending::Mate, winner));
    }
    if board.is_repetition() {
        return Some((Ending::Repetition, Outcome::Draw));
    }
    if board.halfmove_clock() >= 100 {
        return Some((Ending::FiftyMoves, Outcome::Draw));
    }
    if board.is_insufficient_material() {
        return Some((Ending::InsufficientMaterial, Outcome::Draw));
    }
    None
}

/// Scores from the side to move's point of view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Played {
    pub moves: Vec<(Move, Score)>,
    pub ending: Ending,
    pub outcome: Outcome,
}

/// From an empty table; what screens a start.
pub fn probe(board: &mut Position, nodes: u64, tt: &Table) -> Score {
    tt.clear();
    let stop = AtomicBool::new(false);
    let mut search = Search::new(&stop, tt);
    search.set_limits(node_limit(nodes));
    search.run(board, &mut io::sink());
    search.score()
}

/// The same board and node count give the same game, whatever `tt` held before.
///
/// # Panics
///
/// If the search returns an illegal move.
pub fn play(board: &mut Position, nodes: u64, tt: &Table) -> Played {
    tt.clear();
    let stop = AtomicBool::new(false);
    let mut search = Search::new(&stop, tt);
    search.set_limits(node_limit(nodes));
    let mut moves = Vec::new();
    loop {
        if let Some((ending, outcome)) = ending(board) {
            return Played {
                moves,
                ending,
                outcome,
            };
        }
        let best = search.run(board, &mut io::sink());
        assert!(
            generate_legal(board).contains(best),
            "the search returned {best:?}, which is not legal"
        );
        moves.push((best, search.score()));
        board.play(best);
    }
}

fn node_limit(nodes: u64) -> Limits {
    Limits {
        nodes: Some(nodes),
        ..Limits::default()
    }
}
