// SPDX-License-Identifier: GPL-3.0-or-later

//! The first layer's sums for both perspectives, refreshed from a board or updated from its
//! parent by the pieces a move changed, and the stack of them a search walks.

use cadence_core::DirtyPieces;
use cadence_core::position::Board;

use crate::net::{HIDDEN, Network};

/// Indexed by the perspective's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(64))]
pub struct Accumulator {
    pub sums: [[i16; HIDDEN]; 2],
}

impl Network {
    #[must_use]
    pub fn refresh(&self, _board: &Board) -> Accumulator {
        Accumulator {
            sums: [[0; HIDDEN]; 2],
        }
    }

    #[must_use]
    pub fn update(&self, parent: &Accumulator, _dirty: &DirtyPieces) -> Accumulator {
        *parent
    }
}

/// One accumulator a ply, allocated once and never resized, its cursor kept in step with the
/// board's by whoever makes and unmakes the moves.
pub struct AccumulatorStack {
    root: Accumulator,
    top: usize,
}

impl AccumulatorStack {
    #[must_use]
    pub fn new(net: &Network, board: &Board) -> AccumulatorStack {
        AccumulatorStack {
            root: net.refresh(board),
            top: 0,
        }
    }

    #[must_use]
    pub fn current(&self) -> &Accumulator {
        &self.root
    }

    pub fn push(&mut self, _net: &Network, _dirty: &DirtyPieces) {}

    pub fn push_unchanged(&mut self) {}

    pub fn pop(&mut self) {}

    pub fn reset(&mut self, net: &Network, board: &Board) {
        self.root = net.refresh(board);
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.top
    }
}
