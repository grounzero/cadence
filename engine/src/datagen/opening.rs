// SPDX-License-Identifier: GPL-3.0-or-later

//! A double-Fischer start drawn uniformly, uniform random plies on top, and a shallow screen
//! refusing a start already decided. The three constants are chosen, not measured.

use cadence_core::chess960::{ARRAYS, dfrc_fen};
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{Move, generate_legal};

use super::game;
use crate::position::Position;
use crate::score::Score;
use crate::tt::Table;

/// Half for each side.
pub const RANDOM_PLIES: usize = 8;

/// The same as a game move's.
pub const SCREEN_NODES: u64 = 5_000;

/// Drops gross imbalance without trying to keep a start balanced.
pub const WINDOW: Score = 200;

pub struct Opening {
    pub white: u32,
    pub black: u32,
    pub plies: Vec<Move>,
    pub board: Position,
}

/// What screens a start, which a test replaces.
type Probe<'a> = dyn Fn(&mut Position, &Table) -> Option<Score> + 'a;

fn screen(board: &mut Position, tt: &Table) -> Option<Score> {
    game::probe(board, SCREEN_NODES, tt)
}

/// `None` if the random plies end the game or the screen refuses it, as it does a probe that did not
/// finish.
///
/// # Panics
///
/// If the drawn start does not parse.
pub fn draw(rng: &mut Rng, tt: &Table) -> Option<Opening> {
    draw_with(rng, tt, &screen)
}

fn draw_with(rng: &mut Rng, tt: &Table, probe: &Probe<'_>) -> Option<Opening> {
    let white = draw_array(rng);
    let black = draw_array(rng);
    let fen = dfrc_fen(white, black);
    let start = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
    let mut board = Position::new(start);
    let mut plies = Vec::with_capacity(RANDOM_PLIES);
    for _ in 0..RANDOM_PLIES {
        let legal = generate_legal(&board);
        if legal.is_empty() {
            return None;
        }
        let m = legal.as_slice()[rng.below(legal.len())];
        board.play(m);
        plies.push(m);
    }
    if game::ending(&board).is_some() {
        return None;
    }
    let score = probe(&mut board, tt)?;
    (score.abs() <= WINDOW).then_some(Opening {
        white,
        black,
        plies,
        board,
    })
}

/// Also returns how many were refused.
pub fn next(rng: &mut Rng, tt: &Table) -> (Opening, u32) {
    next_with(rng, tt, &screen)
}

fn next_with(rng: &mut Rng, tt: &Table, probe: &Probe<'_>) -> (Opening, u32) {
    let mut refused = 0;
    loop {
        if let Some(opening) = draw_with(rng, tt, probe) {
            return (opening, refused);
        }
        refused += 1;
    }
}

fn draw_array(rng: &mut Rng) -> u32 {
    u32::try_from(rng.below(ARRAYS as usize)).expect("below the array count")
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use cadence_core::rng::Rng;

    use super::{next_with, screen};
    use crate::datagen::HASH_MB;
    use crate::tt::Table;

    /// A probe that does not finish refuses its start rather than reading as balanced.
    #[test]
    fn an_unfinished_probe_refuses_its_start() {
        let tt = Table::new(HASH_MB).expect("a datagen table");
        let calls = Cell::new(0);
        let probe = |board: &mut _, tt: &Table| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                None
            } else {
                screen(board, tt)
            }
        };
        let (_, refused) = next_with(&mut Rng::new(3), &tt, &probe);
        assert_eq!(refused, 1, "the unfinished probe's start was taken");
    }
}
