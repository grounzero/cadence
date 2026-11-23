// SPDX-License-Identifier: GPL-3.0-or-later

//! The board, and whatever must move in step with it: the accumulator stack when a net scores the
//! search, and nothing else.

use core::ops::Deref;
use std::sync::Arc;

use cadence_core::position::Board;
use cadence_core::{DirtyPieces, Move};
use cadence_nnue::{AccumulatorStack, AnyNetwork, HIDDEN, Network, WIDE};

use crate::score::{MAX_EVAL, Score};

/// The net and the accumulators its search walks, one a ply, at the width the net's header names.
enum Inference {
    Narrow(Arc<Network<HIDDEN>>, AccumulatorStack<HIDDEN>),
    Wide(Arc<Network<WIDE>>, AccumulatorStack<WIDE>),
}

/// The same body for either width, so the narrow net's code is exactly what it was before the wide
/// one existed.
macro_rules! each {
    ($inference:expr, |$net:ident, $stack:ident| $body:expr) => {
        match $inference {
            Inference::Narrow($net, $stack) => $body,
            Inference::Wide($net, $stack) => $body,
        }
    };
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
    pub fn with_net(board: Board, net: impl Into<AnyNetwork>) -> Position {
        let inference = match net.into() {
            AnyNetwork::Narrow(net) => {
                let stack = AccumulatorStack::new(&net, &board);
                Inference::Narrow(net, stack)
            }
            AnyNetwork::Wide(net) => {
                let stack = AccumulatorStack::new(&net, &board);
                Inference::Wide(net, stack)
            }
        };
        Position {
            board,
            inference: Some(inference),
        }
    }

    /// The same board and net with accumulators of its own, for a helper thread's search.
    #[must_use]
    pub fn fork(&self) -> Position {
        let board = self.board.duplicate();
        match &self.inference {
            Some(Inference::Narrow(net, _)) => Position::with_net(board, Arc::clone(net)),
            Some(Inference::Wide(net, _)) => Position::with_net(board, Arc::clone(net)),
            None => Position::new(board),
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

    /// `None` where the handcrafted evaluation scores the position; held strictly inside the
    /// evaluation's range, as that evaluation is, below every mate and tablebase score.
    #[must_use]
    pub fn net_eval(&self) -> Option<Score> {
        let stm = self.board.side_to_move();
        self.inference.as_ref().map(|i| {
            each!(i, |net, stack| net.evaluate(stack.current(), stm))
                .clamp(-MAX_EVAL + 1, MAX_EVAL - 1)
        })
    }

    /// Which parameter set the search reads follows this.
    #[must_use]
    pub fn scored_by_net(&self) -> bool {
        self.inference.is_some()
    }

    /// Plies the accumulators stand above their root, which every search must leave at zero.
    #[must_use]
    pub fn net_ply(&self) -> Option<usize> {
        self.inference
            .as_ref()
            .map(|i| each!(i, |_net, stack| stack.depth()))
    }

    #[inline]
    pub fn make_move(&mut self, m: Move) -> DirtyPieces {
        let dirty = self.board.make_move(m);
        if let Some(i) = &mut self.inference {
            each!(i, |net, stack| stack.push(net, &dirty, &self.board));
        }
        dirty
    }

    #[inline]
    pub fn unmake_move(&mut self, m: Move) {
        self.board.unmake_move(m);
        if let Some(i) = &mut self.inference {
            each!(i, |_net, stack| stack.pop());
        }
    }

    #[inline]
    pub fn make_null_move(&mut self) -> DirtyPieces {
        let dirty = self.board.make_null_move();
        if let Some(i) = &mut self.inference {
            each!(i, |_net, stack| stack.push_unchanged());
        }
        dirty
    }

    /// Keeps `m` in the key history rather than on the search stack: the `position ... moves ...`
    /// path, not one the search takes, so the accumulator starts again from the new root.
    #[inline]
    pub fn play(&mut self, m: Move) {
        self.board.play(m);
        if let Some(i) = &mut self.inference {
            let board = &self.board;
            each!(i, |net, stack| stack.reset(net, board));
        }
    }

    #[inline]
    pub fn unmake_null_move(&mut self) {
        self.board.unmake_null_move();
        if let Some(i) = &mut self.inference {
            each!(i, |_net, stack| stack.pop());
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
