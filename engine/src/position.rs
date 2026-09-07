// SPDX-License-Identifier: GPL-3.0-or-later

//! What the search holds: the board, and beside it whatever else has to move in step with it.
//! Today that is the board alone; the accumulator stack joins it when there is a net to evaluate.

use core::ops::Deref;

use cadence_core::position::Board;
use cadence_core::{DirtyPieces, Move};

/// The board the search makes moves on, wrapped so that mutation has one entry point. Read
/// access derefs to [`Board`], and there is deliberately no `DerefMut`: the four methods below
/// are the only way to move the position, which is what keeps a second cursor beside it honest.
pub struct Position {
    board: Board,
}

impl Position {
    /// Hold `board` at the ply it is already on.
    #[must_use]
    pub fn new(board: Board) -> Position {
        Position { board }
    }

    /// The board, for a consumer that names the type rather than taking it by deref.
    #[must_use]
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// Give the board back, for a caller that is done searching it.
    #[must_use]
    pub fn into_board(self) -> Board {
        self.board
    }

    /// Play `m`, returning the piece changes it produced. The return is what the accumulator
    /// consumes once there is one, and is discarded until then.
    #[inline]
    pub fn make_move(&mut self, m: Move) -> DirtyPieces {
        self.board.make_move(m)
    }

    /// Take `m` back, leaving the position as it was before [`Position::make_move`].
    #[inline]
    pub fn unmake_move(&mut self, m: Move) {
        self.board.unmake_move(m);
    }

    /// Pass the move without touching a piece, which changes the side to move and nothing else.
    #[inline]
    pub fn make_null_move(&mut self) -> DirtyPieces {
        self.board.make_null_move()
    }

    /// Play `m` as a game move, which keeps it in the key history rather than on the search
    /// stack. This is the `position ... moves ...` path and not a path the search takes.
    #[inline]
    pub fn play(&mut self, m: Move) {
        self.board.play(m);
    }

    /// Take the null move back.
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
