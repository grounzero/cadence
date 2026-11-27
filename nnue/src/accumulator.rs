// SPDX-License-Identifier: GPL-3.0-or-later

//! The first layer's sums for both perspectives, refreshed from a board or updated from its
//! parent by the pieces a move changed, and the stack of them a search walks. A perspective indexed
//! by its own king is rebuilt from the board when that king crosses the middle or changes bucket.

use cadence_core::features::{
    bucketed_feature_index, feature_index, king_bucket, mirrored_feature_index, mirrors,
};
use cadence_core::position::Board;
use cadence_core::{Colour, DirtyPieces, MAX_DIRTY, MAX_PLY, Piece, PieceType, Square};

use crate::net::{HIDDEN, Inputs, Network};

/// Indexed by the perspective's colour, `H` lanes a perspective.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(64))]
pub struct Accumulator<const H: usize = HIDDEN> {
    pub sums: [[i16; H]; 2],
}

const COLOURS: [Colour; 2] = [Colour::White, Colour::Black];

/// How one input layout reads the board: the first-layer row of a piece, and whether a move of
/// a perspective's own king changes the row of every piece it reads.
trait Layout {
    fn index(&self, persp: Colour, king: Square, piece: Piece, sq: Square) -> usize;
    fn crossed(&self, persp: Colour, from: Square, to: Square) -> bool;
}

struct Plain;
struct Mirrored;
struct Bucketed;

impl Layout for Plain {
    fn index(&self, persp: Colour, _: Square, piece: Piece, sq: Square) -> usize {
        feature_index(persp, piece, sq)
    }

    fn crossed(&self, _: Colour, _: Square, _: Square) -> bool {
        false
    }
}

impl Layout for Mirrored {
    fn index(&self, persp: Colour, king: Square, piece: Piece, sq: Square) -> usize {
        mirrored_feature_index(persp, king, piece, sq)
    }

    fn crossed(&self, _: Colour, from: Square, to: Square) -> bool {
        mirrors(from) != mirrors(to)
    }
}

impl Layout for Bucketed {
    fn index(&self, persp: Colour, king: Square, piece: Piece, sq: Square) -> usize {
        bucketed_feature_index(persp, king, piece, sq)
    }

    fn crossed(&self, persp: Colour, from: Square, to: Square) -> bool {
        mirrors(from) != mirrors(to) || king_bucket(persp, from) != king_bucket(persp, to)
    }
}

/// The lanes an update changes in one pass: eight 256-bit registers, and a whole number of them
/// at every width a net is built at.
const BLOCK: usize = 128;

impl<const H: usize> Network<H> {
    /// Every piece on the board added to the biases, the path an update must always agree with.
    #[must_use]
    pub fn refresh(&self, board: &Board) -> Accumulator<H> {
        let mut acc = Accumulator {
            sums: [self.bias; 2],
        };
        for persp in COLOURS {
            acc.sums[persp.index()] = match self.inputs() {
                Inputs::Plain => self.rebuilt(board, persp, &Plain),
                Inputs::Mirrored => self.rebuilt(board, persp, &Mirrored),
                Inputs::Bucketed => self.rebuilt(board, persp, &Bucketed),
            };
        }
        acc
    }

    /// The child of `parent` by the pieces a move changed, as `update_into` writes it. `board` is
    /// the position the move left, which names each king's square.
    #[must_use]
    pub fn update(
        &self,
        parent: &Accumulator<H>,
        dirty: &DirtyPieces,
        board: &Board,
    ) -> Accumulator<H> {
        let mut child = *parent;
        self.update_into(parent, dirty, board, &mut child);
        child
    }

    /// `update` written into `child`, whatever it held, so a search's stack is never copied twice.
    /// The layout is matched once and each arm runs its own copy of the loop, so no piece pays
    /// for a layout the net does not have.
    pub fn update_into(
        &self,
        parent: &Accumulator<H>,
        dirty: &DirtyPieces,
        board: &Board,
        child: &mut Accumulator<H>,
    ) {
        match self.inputs() {
            Inputs::Plain => self.updated(parent, dirty, board, child, &Plain),
            Inputs::Mirrored => self.updated(parent, dirty, board, child, &Mirrored),
            Inputs::Bucketed => self.updated(parent, dirty, board, child, &Bucketed),
        }
    }

    /// `update_into` under one layout. A perspective whose own king `crossed` is rebuilt from
    /// `board`; any other is moved by every subtraction before any addition, as a DFRC castle can
    /// empty and fill one square.
    fn updated<L: Layout>(
        &self,
        parent: &Accumulator<H>,
        dirty: &DirtyPieces,
        board: &Board,
        child: &mut Accumulator<H>,
        layout: &L,
    ) {
        for persp in COLOURS {
            let sums = &mut child.sums[persp.index()];
            let own = Piece::new(persp, PieceType::King);
            let crossed = dirty.as_slice().iter().any(|d| {
                d.piece == own
                    && matches!(
                        (d.from.get(), d.to.get()),
                        (Some(from), Some(to)) if layout.crossed(persp, from, to)
                    )
            });
            if crossed {
                *sums = self.rebuilt(board, persp, layout);
                continue;
            }
            let king = board.king_square(persp);
            let (mut removed, mut added) = ([0; MAX_DIRTY], [0; MAX_DIRTY]);
            let (mut r, mut a) = (0, 0);
            for d in dirty.as_slice() {
                if let Some(from) = d.from.get() {
                    removed[r] = layout.index(persp, king, d.piece, from);
                    r += 1;
                }
                if let Some(to) = d.to.get() {
                    added[a] = layout.index(persp, king, d.piece, to);
                    a += 1;
                }
            }
            self.moved(
                &parent.sums[persp.index()],
                sums,
                &removed[..r],
                &added[..a],
            );
        }
    }

    /// One perspective's lanes `BLOCK` at a time, each read from the parent, taken down by every
    /// removed row before any added row and written once, so a lane stays within the board.
    /// Never inlined, so that CI can find it in a release binary and check that it is vector code.
    #[inline(never)]
    fn moved(&self, parent: &[i16; H], child: &mut [i16; H], removed: &[usize], added: &[usize]) {
        const { assert!(H.is_multiple_of(BLOCK), "a width of whole blocks") };
        let (from, _) = parent.as_chunks::<BLOCK>();
        let (to, _) = child.as_chunks_mut::<BLOCK>();
        for (at, (from, to)) in from.iter().zip(to).enumerate() {
            let mut block = *from;
            for &row in removed {
                let (rows, _) = self.weights[row].as_chunks::<BLOCK>();
                for (s, &w) in block.iter_mut().zip(&rows[at]) {
                    *s -= w;
                }
            }
            for &row in added {
                let (rows, _) = self.weights[row].as_chunks::<BLOCK>();
                for (s, &w) in block.iter_mut().zip(&rows[at]) {
                    *s += w;
                }
            }
            *to = block;
        }
    }

    /// One perspective's sums from every piece on the board.
    fn rebuilt<L: Layout>(&self, board: &Board, persp: Colour, layout: &L) -> [i16; H] {
        let king = board.king_square(persp);
        let mut sums = self.bias;
        for sq in board.occupied() {
            if let Some(piece) = board.piece_at(sq) {
                self.add(&mut sums, layout.index(persp, king, piece, sq));
            }
        }
        sums
    }

    fn add(&self, sums: &mut [i16; H], index: usize) {
        for (s, &w) in sums.iter_mut().zip(&self.weights[index]) {
            *s += w;
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
