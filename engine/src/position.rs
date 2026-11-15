// SPDX-License-Identifier: GPL-3.0-or-later

//! The board, and whatever must move in step with it: the accumulator stack when a net scores the
//! search, and nothing else.

use core::ops::Deref;
use std::sync::Arc;

use cadence_core::position::Board;
use cadence_core::{DirtyPieces, Move};
use cadence_nnue::{AccumulatorStack, Network};

use crate::score::{MAX_EVAL, Score};

/// The net and the accumulators its search walks, one a ply.
struct Inference {
    net: Arc<Network>,
    stack: AccumulatorStack,
}

/// No `DerefMut`: the methods below are the only way to move the position, which keeps the
/// accumulator's cursor in step with the board's.
pub struct Position {
    board: Board,
    inference: Option<Inference>,
}

impl Position {
    /// Scored by the handcrafted evaluation.
    #[must_use]
    pub fn new(board: Board) -> Position {
        Position {
            board,
            inference: None,
        }
    }

    /// Scored by `net` from here on, its accumulator refreshed from the board.
    #[must_use]
    pub fn with_net(board: Board, net: Arc<Network>) -> Position {
        let stack = AccumulatorStack::new(&net, &board);
        Position {
            board,
            inference: Some(Inference { net, stack }),
        }
    }

    /// The same board and net with accumulators of its own, for a helper thread's search.
    #[must_use]
    pub fn fork(&self) -> Position {
        match &self.inference {
            Some(i) => Position::with_net(self.board.duplicate(), Arc::clone(&i.net)),
            None => Position::new(self.board.duplicate()),
        }
    }

    #[must_use]
    pub fn board(&self) -> &Board {
        &self.board
    }

    #[must_use]
    pub fn into_board(self) -> Board {
        self.board
    }

    /// `None` where the handcrafted evaluation scores the position; held inside the evaluation's
    /// range, below every mate score.
    #[must_use]
    pub fn net_eval(&self) -> Option<Score> {
        self.inference.as_ref().map(|i| {
            i.net
                .evaluate(i.stack.current(), self.board.side_to_move())
                .clamp(-MAX_EVAL, MAX_EVAL)
        })
    }

    /// Plies the accumulators stand above their root, which every search must leave at zero.
    #[must_use]
    pub fn net_ply(&self) -> Option<usize> {
        self.inference.as_ref().map(|i| i.stack.depth())
    }

    #[inline]
    pub fn make_move(&mut self, m: Move) -> DirtyPieces {
        let dirty = self.board.make_move(m);
        if let Some(i) = &mut self.inference {
            i.stack.push(&i.net, &dirty);
        }
        dirty
    }

    #[inline]
    pub fn unmake_move(&mut self, m: Move) {
        self.board.unmake_move(m);
        if let Some(i) = &mut self.inference {
            i.stack.pop();
        }
    }

    #[inline]
    pub fn make_null_move(&mut self) -> DirtyPieces {
        let dirty = self.board.make_null_move();
        if let Some(i) = &mut self.inference {
            i.stack.push_unchanged();
        }
        dirty
    }

    /// Keeps `m` in the key history rather than on the search stack: the `position ... moves ...`
    /// path, not one the search takes, so the accumulator starts again from the new root.
    #[inline]
    pub fn play(&mut self, m: Move) {
        self.board.play(m);
        if let Some(i) = &mut self.inference {
            i.stack.reset(&i.net, &self.board);
        }
    }

    #[inline]
    pub fn unmake_null_move(&mut self) {
        self.board.unmake_null_move();
        if let Some(i) = &mut self.inference {
            i.stack.pop();
        }
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
