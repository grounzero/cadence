// SPDX-License-Identifier: GPL-3.0-or-later

//! The board, and whatever must move in step with it: today the board alone, and the accumulator
//! stack once there is a net.

use core::ops::Deref;

use cadence_core::position::Board;
use cadence_core::{DirtyPieces, Move};

/// No `DerefMut`: the methods below are the only way to move the position, which keeps a second
/// cursor beside it honest.
pub struct Position {
    board: Board,
}

impl Position {
    #[must_use]
    pub fn new(board: Board) -> Position {
        Position { board }
    }

    #[must_use]
    pub fn board(&self) -> &Board {
        &self.board
    }

    #[must_use]
    pub fn into_board(self) -> Board {
        self.board
    }

    /// The returned changes are for the accumulator, once there is one.
    #[inline]
    pub fn make_move(&mut self, m: Move) -> DirtyPieces {
        self.board.make_move(m)
    }

    #[inline]
    pub fn unmake_move(&mut self, m: Move) {
        self.board.unmake_move(m);
    }

    #[inline]
    pub fn make_null_move(&mut self) -> DirtyPieces {
        self.board.make_null_move()
    }

    /// Keeps `m` in the key history rather than on the search stack: the `position ... moves ...`
    /// path, not one the search takes.
    #[inline]
    pub fn play(&mut self, m: Move) {
        self.board.play(m);
    }

    #[inline]
    pub fn unmake_null_move(&mut self) {
        self.board.unmake_null_move();
    }
}

impl core::fmt::Debug for Position {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.board.fmt(f)
    }
}

impl Deref for Position {
    type Target = Board;

    #[inline]
    fn deref(&self) -> &Board {
        &self.board
    }
}
