// SPDX-License-Identifier: GPL-3.0-or-later

//! No adjudication and no ply cap: the fifty-move rule bounds a game, and a game with no result is
//! worse than a long one.

use std::io;
use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Colour, Move, generate_legal};

use crate::position::Position;
use crate::score::Score;
use crate::search::{Learned, Limits, Play, Search, Settings, sealed};
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

/// Scores from the side to move's point of view, `None` where the search did not finish depth 1.
/// Such a search's score is a fallback of 0 or a partial bound, so it labels nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Played {
    pub moves: Vec<(Move, Option<Score>)>,
    pub ending: Ending,
    pub outcome: Outcome,
}

/// From an empty table; what screens a start, `None` if the search did not finish depth 1.
pub fn probe(board: &mut Position, nodes: u64, tt: &Table) -> Option<Score> {
    tt.clear();
    let stop = AtomicBool::new(false);
    let mut search = Search::new(&stop, tt);
    search.set_limits(node_limit(nodes));
    search.run(board, &mut io::sink());
    label(&search)
}

/// The same board and node count give the same game, whatever `tt` or an earlier game left behind.
/// One set of histories is lent to every search of the game, so each move starts from what the
/// moves before it learned, and the first from empty tables.
///
/// # Panics
///
/// If the search returns an illegal move.
pub fn play(board: &mut Position, nodes: u64, tt: &Table) -> Played {
    play_as(board, nodes, tt, false)
}

/// As [`play`], with every search's quiescence stand-pat cut answering with the stand pat where
/// `stand_pat_fail_soft` is set.
///
/// # Panics
///
/// If the search returns an illegal move.
pub fn play_as(board: &mut Position, nodes: u64, tt: &Table, stand_pat_fail_soft: bool) -> Played {
    if stand_pat_fail_soft {
        play_with(board, nodes, tt, SoftStandPat(()))
    } else {
        play_with(board, nodes, tt, Play)
    }
}

/// Datagen's copy of the search, whose quiescence stand-pat cut answers with the stand pat. Private
/// to this module and made from a private field, so play cannot name or make it.
#[derive(Clone, Copy, Debug)]
struct SoftStandPat(());

impl sealed::Sealed for SoftStandPat {}

impl Settings for SoftStandPat {
    const SOFT_STAND_PAT: bool = true;
}

/// The setting is chosen once per game, so neither copy of the search checks it.
fn play_with<S: Settings>(board: &mut Position, nodes: u64, tt: &Table, settings: S) -> Played {
    tt.clear();
    let stop = AtomicBool::new(false);
    let mut search = Search::with_settings(&stop, tt, settings);
    search.set_limits(node_limit(nodes));
    let mut learned = Learned::new();
    search.swap_learned(&mut learned);
    let mut moves = Vec::new();
    let (ending, outcome) = loop {
        if let Some(end) = ending(board) {
            break end;
        }
        let best = search.run(board, &mut io::sink());
        assert!(
            generate_legal(board).contains(best),
            "the search returned {best:?}, which is not legal"
        );
        moves.push((best, label(&search)));
        board.play(best);
    };
    search.swap_learned(&mut learned);
    Played {
        moves,
        ending,
        outcome,
    }
}

fn label<S: Settings>(search: &Search<'_, S>) -> Option<Score> {
    (search.completed_depth() > 0).then_some(search.score())
}

fn node_limit(nodes: u64) -> Limits {
    Limits {
        nodes: Some(nodes),
        ..Limits::default()
    }
}

#[cfg(test)]
mod tests {
    use cadence_core::generate_noisy;

    use super::*;
    use crate::eval;

    /// Only the side to move's king can move and nothing can be taken, so a horizon node is its
    /// stand pat alone.
    const STAND_PAT: &str = "7k/5q2/8/8/1p6/pP6/P7/7K w - - 0 1";

    fn cut<S: Settings>(settings: S, beta: Score) -> Score {
        let stop = AtomicBool::new(false);
        let tt = Table::new(crate::tt::DEFAULT_HASH_MB).expect("a table");
        let mut board = Position::new(Board::from_fen(STAND_PAT).expect("a fen"));
        let mut search = Search::with_settings(&stop, &tt, settings);
        search.node_window(&mut board, 0, 1, beta - 1, beta)
    }

    /// Asked with a window far below its stand pat, play's copy answers with beta and datagen's
    /// with the stand pat.
    #[test]
    fn a_stand_pat_cut_answers_beta_in_play_s_copy_and_the_stand_pat_in_datagen_s() {
        let board = Board::from_fen(STAND_PAT).expect("a fen");
        assert!(
            generate_noisy(&board).is_empty(),
            "the side to move can take"
        );
        let stand_pat = eval::evaluate(&board);
        let beta = stand_pat - 100;
        assert_eq!(cut(Play, beta), beta, "play's copy");
        assert_eq!(cut(SoftStandPat(()), beta), stand_pat, "datagen's copy");
    }
}
