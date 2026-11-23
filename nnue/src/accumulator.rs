// SPDX-License-Identifier: GPL-3.0-or-later

//! The first layer's sums for both perspectives, refreshed from a board or updated from its
//! parent by the pieces a move changed, and the stack of them a search walks. A mirrored net's
//! perspective is rebuilt from the board when its own king crosses the middle.

use cadence_core::features::{feature_index, mirrored_feature_index, mirrors};
use cadence_core::position::Board;
use cadence_core::{Colour, DirtyPieces, MAX_PLY, Piece, PieceType, Square};

use crate::net::{HIDDEN, Inputs, Network};

/// Indexed by the perspective's colour, `H` lanes a perspective.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(64))]
pub struct Accumulator<const H: usize = HIDDEN> {
    pub sums: [[i16; H]; 2],
}

const COLOURS: [Colour; 2] = [Colour::White, Colour::Black];

impl<const H: usize> Network<H> {
    /// Every piece on the board added to the biases, the path an update must always agree with.
    #[must_use]
    pub fn refresh(&self, board: &Board) -> Accumulator<H> {
        let mut acc = Accumulator {
            sums: [self.bias; 2],
        };
        for persp in COLOURS {
            acc.sums[persp.index()] = self.rebuilt(board, persp);
        }
        acc
    }

    /// Every subtraction before any addition: a DFRC castle can empty and fill one square, and
    /// a lane must never hold more than the board's pieces. `board` is the position the move
    /// left, which names each king's square and is what a crossing perspective is rebuilt from.
    #[must_use]
    pub fn update(
        &self,
        parent: &Accumulator<H>,
        dirty: &DirtyPieces,
        board: &Board,
    ) -> Accumulator<H> {
        let mut acc = *parent;
        for persp in COLOURS {
            if self.crossed(dirty, persp) {
                acc.sums[persp.index()] = self.rebuilt(board, persp);
            } else {
                self.apply(&mut acc.sums[persp.index()], persp, dirty, board);
            }
        }
        acc
    }

    /// `update` written into `child`, whatever it held: each perspective's lanes are copied from
    /// the parent once and changed where they land. A search's stack takes this path, which saves
    /// the copy a returned accumulator costs on every move.
    pub fn update_into(
        &self,
        parent: &Accumulator<H>,
        dirty: &DirtyPieces,
        board: &Board,
        child: &mut Accumulator<H>,
    ) {
        for persp in COLOURS {
            let sums = &mut child.sums[persp.index()];
            if self.crossed(dirty, persp) {
                *sums = self.rebuilt(board, persp);
            } else {
                *sums = parent.sums[persp.index()];
                self.apply(sums, persp, dirty, board);
            }
        }
    }

    /// One perspective's sums moved from the parent's to the child's by the pieces a move changed.
    fn apply(&self, sums: &mut [i16; H], persp: Colour, dirty: &DirtyPieces, board: &Board) {
        let king = board.king_square(persp);
        for d in dirty.as_slice() {
            if let Some(from) = d.from.get() {
                self.sub(sums, self.index(persp, king, d.piece, from));
            }
        }
        for d in dirty.as_slice() {
            if let Some(to) = d.to.get() {
                self.add(sums, self.index(persp, king, d.piece, to));
            }
        }
    }

    /// One perspective's sums from every piece on the board.
    fn rebuilt(&self, board: &Board, persp: Colour) -> [i16; H] {
        let king = board.king_square(persp);
        let mut sums = self.bias;
        for sq in board.occupied() {
            if let Some(piece) = board.piece_at(sq) {
                self.add(&mut sums, self.index(persp, king, piece, sq));
            }
        }
        sums
    }

    /// Whether the move took this perspective's own king from one half of a mirrored net's board
    /// to the other, which changes the index of every piece it reads.
    fn crossed(&self, dirty: &DirtyPieces, persp: Colour) -> bool {
        self.inputs() == Inputs::Mirrored
            && dirty.as_slice().iter().any(|d| {
                d.piece == Piece::new(persp, PieceType::King)
                    && matches!(
                        (d.from.get(), d.to.get()),
                        (Some(from), Some(to)) if mirrors(from) != mirrors(to)
                    )
            })
    }

    /// The first-layer column a perspective reads a piece by, under the layout the header names.
    fn index(&self, persp: Colour, king: Square, piece: Piece, sq: Square) -> usize {
        match self.inputs() {
            Inputs::Plain => feature_index(persp, piece, sq),
            Inputs::Mirrored => mirrored_feature_index(persp, king, piece, sq),
        }
    }

    fn add(&self, sums: &mut [i16; H], index: usize) {
        for (s, &w) in sums.iter_mut().zip(&self.weights[index]) {
            *s += w;
        }
    }

    fn sub(&self, sums: &mut [i16; H], index: usize) {
        for (s, &w) in sums.iter_mut().zip(&self.weights[index]) {
            *s -= w;
        }
    }
}

/// One accumulator a ply, allocated once and never resized, its cursor kept in step with the
/// board's by whoever makes and unmakes the moves.
pub struct AccumulatorStack<const H: usize = HIDDEN> {
    stack: Box<[Accumulator<H>; MAX_PLY + 1]>,
    top: usize,
}

impl<const H: usize> AccumulatorStack<H> {
    #[must_use]
    pub fn new(net: &Network<H>, board: &Board) -> AccumulatorStack<H> {
        let root = net.refresh(board);
        let stack: Box<[Accumulator<H>; MAX_PLY + 1]> = vec![root; MAX_PLY + 1]
            .into_boxed_slice()
            .try_into()
            .unwrap_or_else(|_| unreachable!("a vector of MAX_PLY + 1 is MAX_PLY + 1 long"));
        AccumulatorStack { stack, top: 0 }
    }

    #[must_use]
    pub fn current(&self) -> &Accumulator<H> {
        &self.stack[self.top]
    }

    /// # Panics
    ///
    /// Past `MAX_PLY` plies, which the search's own bound keeps it from reaching.
    pub fn push(&mut self, net: &Network<H>, dirty: &DirtyPieces, board: &Board) {
        let (parents, children) = self.stack.split_at_mut(self.top + 1);
        net.update_into(&parents[self.top], dirty, board, &mut children[0]);
        self.top += 1;
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
    pub fn reset(&mut self, net: &Network<H>, board: &Board) {
        self.top = 0;
        self.stack[0] = net.refresh(board);
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.top
    }
}
