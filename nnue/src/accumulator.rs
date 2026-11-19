// SPDX-License-Identifier: GPL-3.0-or-later

//! The first layer's sums for both perspectives, refreshed from a board or updated from its
//! parent by the pieces a move changed, and the stack of them a search walks.

use cadence_core::features::feature_index;
use cadence_core::position::Board;
use cadence_core::{Colour, DirtyPieces, MAX_PLY, Piece, Square};

use crate::net::{HIDDEN, Network};

/// Indexed by the perspective's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(64))]
pub struct Accumulator {
    pub sums: [[i16; HIDDEN]; 2],
}

const COLOURS: [Colour; 2] = [Colour::White, Colour::Black];

impl Network {
    /// Every piece on the board added to the biases, the path an update must always agree with.
    #[must_use]
    pub fn refresh(&self, board: &Board) -> Accumulator {
        let mut acc = Accumulator {
            sums: [self.bias; 2],
        };
        for sq in board.occupied() {
            if let Some(piece) = board.piece_at(sq) {
                for persp in COLOURS {
                    self.add(&mut acc.sums[persp.index()], persp, piece, sq);
                }
            }
        }
        acc
    }

    /// Every subtraction before any addition: a DFRC castle can empty and fill one square, and
    /// a lane must never hold more than the board's pieces.
    #[must_use]
    pub fn update(&self, parent: &Accumulator, dirty: &DirtyPieces) -> Accumulator {
        let mut acc = *parent;
        for persp in COLOURS {
            let sums = &mut acc.sums[persp.index()];
            for d in dirty.as_slice() {
                if let Some(from) = d.from.get() {
                    self.sub(sums, persp, d.piece, from);
                }
            }
            for d in dirty.as_slice() {
                if let Some(to) = d.to.get() {
                    self.add(sums, persp, d.piece, to);
                }
            }
        }
        acc
    }

    fn add(&self, sums: &mut [i16; HIDDEN], persp: Colour, piece: Piece, sq: Square) {
        let column = &self.weights[feature_index(persp, piece, sq)];
        for (s, &w) in sums.iter_mut().zip(column) {
            *s += w;
        }
    }

    fn sub(&self, sums: &mut [i16; HIDDEN], persp: Colour, piece: Piece, sq: Square) {
        let column = &self.weights[feature_index(persp, piece, sq)];
        for (s, &w) in sums.iter_mut().zip(column) {
            *s -= w;
        }
    }
}

/// One accumulator a ply, allocated once and never resized, its cursor kept in step with the
/// board's by whoever makes and unmakes the moves.
pub struct AccumulatorStack {
    stack: Box<[Accumulator; MAX_PLY + 1]>,
    top: usize,
}

impl AccumulatorStack {
    #[must_use]
    pub fn new(net: &Network, board: &Board) -> AccumulatorStack {
        let root = net.refresh(board);
        let stack: Box<[Accumulator; MAX_PLY + 1]> = vec![root; MAX_PLY + 1]
            .into_boxed_slice()
            .try_into()
            .unwrap_or_else(|_| unreachable!("a vector of MAX_PLY + 1 is MAX_PLY + 1 long"));
        AccumulatorStack { stack, top: 0 }
    }

    #[must_use]
    pub fn current(&self) -> &Accumulator {
        &self.stack[self.top]
    }

    /// # Panics
    ///
    /// Past `MAX_PLY` plies, which the search's own bound keeps it from reaching.
    pub fn push(&mut self, net: &Network, dirty: &DirtyPieces) {
        let child = net.update(&self.stack[self.top], dirty);
        self.top += 1;
        self.stack[self.top] = child;
    }

    /// A null move changes no piece, so the child is the parent.
    ///
    /// # Panics
    ///
    /// Past `MAX_PLY` plies.
    pub fn push_unchanged(&mut self) {
        self.stack[self.top + 1] = self.stack[self.top];
        self.top += 1;
    }

    /// # Panics
    ///
    /// Below the root, which a make and unmake in step never reach.
    pub fn pop(&mut self) {
        self.top -= 1;
    }

    /// The board a `position` command or a played game move leaves becomes the new root.
    pub fn reset(&mut self, net: &Network, board: &Board) {
        self.top = 0;
        self.stack[0] = net.refresh(board);
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.top
    }
}
