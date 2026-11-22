// SPDX-License-Identifier: GPL-3.0-or-later

//! A node's preamble and move loop. The table reads, the null move, `ProbCut` and the late-move
//! search are child modules, which call `negamax` back.

use cadence_core::position::Board;
use cadence_core::{Colour, MAX_MOVES, MAX_PLY, Move, MoveList, generate_legal, has_legal_move};

#[cfg(debug_assertions)]
use super::Call;
use super::depth::{extension, iir_reduction, losing_capture, move_history, reduction};
use super::pruning::{futile_node, lmp_index};
use super::{Expected, Search, Via, bound_for, order_first, remember_killer};
use crate::conthist::{self, OFFSETS};
use crate::picker::{Picker, Tables};
use crate::position::Position;
use crate::score::{ABORTED, DRAW, INFINITE, Score, mated_in};
use crate::tt::Bound;
use lookup::Lookup;

mod late_move;
mod lookup;
mod null_move;
mod probcut;

/// Once a line from a node can reach the fifty-move draw, which the key does not carry, its entry
/// is stored at depth 0, where no probe cuts with the score, and only its move is used.
fn stored_depth(board: &Board, depth: u32) -> u32 {
    if u32::from(board.halfmove_clock()) + depth >= 100 {
        0
    } else {
        depth
    }
}

impl Search<'_> {
    #[must_use]
    pub fn node(&mut self, board: &mut Position, depth: u32, ply: usize) -> Score {
        self.node_window(board, depth, ply, -INFINITE, INFINITE)
    }

    /// A null window is asked as one of the root's later moves would be.
    #[must_use]
    pub fn node_window(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Score {
        let kind = Expected::Pv.child(if beta > alpha + 1 {
            Via::First
        } else {
            Via::Null
        });
        self.node_expected(board, depth, ply, alpha, beta, kind)
    }

    /// `kind` must agree with the window: PV exactly where it is open.
    #[must_use]
    pub fn node_expected(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
        kind: Expected,
    ) -> Score {
        // So the extension cap reads this depth, not what the last iteration left, and no frame or
        // flag is read from an earlier run.
        self.root_depth = depth;
        self.heuristics.frames.fill(None);
        self.tt_pv.fill(false);
        self.choose_tunables(board);
        self.negamax(board, depth, ply, alpha, beta, kind)
    }

    /// Fail-soft. After an abort the value means nothing, and every caller discards it.
    pub(super) fn negamax(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        mut alpha: Score,
        mut beta: Score,
        kind: Expected,
    ) -> Score {
        // First, so a call that returns early is checked too.
        self.expect(ply, depth, kind, alpha, beta);
        // The quiescence search counts this node.
        if depth == 0 {
            return self.quiesce(board, ply, alpha, beta);
        }
        self.visit(ply);
        self.table.clear(ply);
        // At every node, so that `nodes N` stops at N and not at N plus a subtree.
        if self.out_of_time() {
            return ABORTED;
        }

        if board.is_repetition() || board.is_insufficient_material() {
            return DRAW;
        }

        // Above the sort, where the first read past the arrays would be; release has only a bounds
        // check.
        if ply >= MAX_PLY {
            return self.static_eval(board);
        }

        // Before generation: that saving is most of what the table is for.
        let key = board.key();
        let (tt_move, tb) = match self.lookup(board, key, depth, ply, &mut alpha, &mut beta) {
            Lookup::Search { tt_move, bounds } => (tt_move, bounds),
            Lookup::Cutoff(score) => return score,
        };

        // Written even if unread here: a rule at `p + 2` compares against it.
        let in_check = board.in_check();
        let pawn_key = board.pawn_key();
        let us_eval = board.side_to_move();
        self.heuristics.evals[ply] = self.corrected_eval(board, in_check, pawn_key, us_eval);

        // Above the null move: below it the rule would be nothing but its own error case.
        if let Some(bound) = self.reverse_futility(board, depth, ply, alpha, beta) {
            return unless_stalemate(board, bound);
        }

        if let Some(score) = self.null_move(board, depth, ply, alpha, beta) {
            return unless_stalemate(board, score);
        }

        // Last in the preamble: the static readings run first and the null move's pass after
        // them, and this is the one rule that trusts a reduced search of a real move.
        if let Some(score) = self.probcut(board, depth, ply, alpha, beta, kind) {
            return score;
        }

        // The node's own window, read before the loop raises `alpha`.
        let pv = beta > alpha + 1;
        // Last above the move loop, so the margins, the count and every child read the shortened
        // depth.
        let depth = self.iir_depth(depth, pv, in_check, tt_move);

        let mut legal = generate_legal(board);
        if legal.is_empty() {
            return if in_check { mated_in(ply) } else { DRAW };
        }
        // After the mate check: mate on the hundredth half-move is mate.
        if board.halfmove_clock() >= 100 {
            return DRAW;
        }
        // Before any move is made: the history row read here and per move below is the mover's.
        let us = board.side_to_move();
        let (killers, mut picker) = self.order(board, &mut legal, tt_move, us, ply);

        // Below the null move, unlike the other margin tests: nothing between writes what it reads,
        // so the placement saves the test at every node the null move cuts. An open window refuses
        // it, as it refuses every other margin rule.
        let futile = !pv && futile_node(&self.tunables, self.heuristics.evals[ply], depth, alpha);
        self.stats.futility_nodes.add_if(futile);

        // Beside the margin, not above it: they overlap on 43% of one population, and whichever is
        // asked first keeps the moves it takes.
        let give_up = lmp_index(&self.tunables, in_check, pv, depth, legal.len());
        self.stats.lmp_nodes.add_if(give_up.is_some());

        let original_alpha = alpha;
        alpha = alpha.max(tb.0);
        let mut best = -INFINITE;
        let mut best_move = Move::NULL;
        // Written at one site above `make_move`, so every rule giving a move up is excluded unasked.
        let mut searched = Searched::default();
        while let Some((m, i, quiet_index)) = picker.next(board, &mut legal, &mut self.slots[ply]) {
            // Before the move is made, which is the whole saving; above the reduction, which never
            // sees a move given up.
            if self.futile(board, futile, m, i)
                || self.given_up(board, give_up, m, killers, quiet_index)
            {
                continue;
            }
            searched.set(i);
            let losing = losing_capture(m, i, picker.lost());
            self.make(board, ply, m);
            // Free after `make_move`, and also the reduction's check exemption.
            let gives_check = board.in_check();
            let ext = extension(gives_check, ply + 1, self.root_depth);
            let child = depth - 1 + ext;
            // The first move gets the node's own window; every later one is asked the null-window
            // question first.
            let mut score = if i == 0 {
                self.own_window(board, child, ply, alpha, beta, kind, Via::First)
            } else {
                // The index decides whether to reduce, the history score by how much.
                let base = reduction(in_check, gives_check, m, losing, killers, depth, i);
                self.count_lmr_losing_capture(base, m);
                let history = move_history(m, self.heuristics.history.get(us, m));
                self.late_move(board, child, base, history, ply, alpha, pv, kind)
            };
            // None at or above beta, which is the bound already returned, and none at a null-window
            // node.
            if i > 0 && !self.run.aborted && score > alpha && score < beta {
                score = self.own_window(board, child, ply, alpha, beta, kind, Via::Open);
            }
            board.unmake_move(m);
            if self.run.aborted {
                return ABORTED;
            }
            if score > best {
                best = score;
                best_move = m;
                if score > alpha {
                    alpha = score;
                    // At a null window a raise is a cutoff, so the parent fails low and never copies
                    // this row.
                    if beta > original_alpha + 1 {
                        self.table.update(ply, m);
                    }
                    if alpha >= beta {
                        remember_killer(&mut self.heuristics.killers[ply], m);
                        // A move the margin or the count gave up was never played, so it is
                        // passed over rather than debited.
                        self.remember_history(board, ply, legal.as_slice(), i, &searched, depth);
                        break;
                    }
                }
            }
        }
        // Fail-soft, so the bound follows the value, not the window it was found in. Nothing an
        // aborted search computed is stored: the loop returns before this.
        let (best, bound) = within(best, original_alpha, beta, tb);
        self.store(key, ply, stored_depth(board, depth), best, best_move, bound);
        self.remember_correction(pawn_key, us_eval, ply, best, best_move, bound, depth);
        best
    }

    fn order(
        &mut self,
        board: &Board,
        legal: &mut MoveList,
        tt_move: Move,
        us: Colour,
        ply: usize,
    ) -> ([Move; 2], Picker) {
        let ordered = usize::from(order_first(legal, tt_move));
        let killers = self.heuristics.killers[ply];
        let mut continuation = [None; OFFSETS.len()];
        for (row, &offset) in continuation.iter_mut().zip(OFFSETS.iter()) {
            match conthist::context(&self.heuristics.frames, ply, offset) {
                Some(context) => *row = Some(self.heuristics.conthist.row(context)),
                None => self.stats.continuation_absent.add_one(),
            }
        }
        let tables = Tables {
            butterfly: self.heuristics.history.side(us),
            continuation,
        };
        let picker = Picker::new(
            board,
            legal,
            ordered,
            killers,
            &tables,
            &mut self.slots[ply],
        );
        (killers, picker)
    }

    /// Every descent into `negamax` makes its move here, so a child's frames are its path's.
    pub(super) fn make(&mut self, board: &mut Position, ply: usize, m: Move) {
        self.heuristics.frames[ply] = conthist::frame(board, m);
        #[cfg(debug_assertions)]
        {
            self.moved[ply] = m;
        }
        board.make_move(m);
    }

    /// A method so the counter is written where the rule runs.
    fn iir_depth(&mut self, depth: u32, pv: bool, in_check: bool, tt_move: Move) -> u32 {
        let r = iir_reduction(depth, pv, in_check, tt_move != Move::NULL, self.iir_plies);
        self.stats.iir_nodes.add_if(r > 0);
        depth - r
    }

    /// Checked against the window in a debug build, and recorded there once a gate asks.
    pub(super) fn expect(
        &mut self,
        ply: usize,
        depth: u32,
        kind: Expected,
        alpha: Score,
        beta: Score,
    ) {
        debug_assert_eq!(
            kind == Expected::Pv,
            beta > alpha + 1,
            "ply {ply} depth {depth}: {kind:?}"
        );
        #[cfg(debug_assertions)]
        {
            let asked = self.asked.take();
            // Read, not indexed: a gate calls past the ply bound, and the call returns there.
            let moved = ply
                .checked_sub(1)
                .and_then(|up| self.moved.get(up).copied())
                .unwrap_or(Move::NULL);
            if let Some(calls) = &mut self.calls {
                calls.push(Call {
                    ply,
                    depth,
                    asked,
                    kind,
                    open: beta > alpha + 1,
                    moved,
                });
            }
        }
    }

    /// [`Expected::child`], and in a debug build the call it is passed to records how it was
    /// derived.
    pub(super) fn derive(&mut self, parent: Expected, via: Via) -> Expected {
        #[cfg(debug_assertions)]
        {
            self.asked = Some((parent, via));
        }
        parent.child(via)
    }

    /// The child searched with the node's own window, as the first move is and a later one that
    /// beat alpha is again.
    #[allow(
        clippy::too_many_arguments,
        reason = "passing these separately measured +0.21% faster than a shared node type (pull request 233 timing, 2026-11-08)"
    )]
    fn own_window(
        &mut self,
        board: &mut Position,
        child: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
        kind: Expected,
        via: Via,
    ) -> Score {
        let derived = self.derive(kind, via);
        -self.negamax(board, child, ply + 1, -beta, -alpha, derived)
    }
}

/// Which of a node's moves it searched, one bit per index into its own list.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Searched([u64; MAX_MOVES / 64]);

impl Searched {
    /// Indexes rather than saturates, so an index past any list is a bounds failure, not a lost bit.
    pub(super) fn set(&mut self, index: usize) {
        self.0[index / 64] |= 1 << (index % 64);
    }

    pub(super) fn contains(&self, index: usize) -> bool {
        self.0[index / 64] & (1 << (index % 64)) != 0
    }
}

/// A table's floor or ceiling the moves did not pass is the node's value, held with the bound the
/// probe gave it; fail-soft otherwise, so the bound follows the value.
fn within(best: Score, original_alpha: Score, beta: Score, tb: (Score, Score)) -> (Score, Bound) {
    if best < tb.0 {
        (tb.0, Bound::Lower)
    } else if best > tb.1 {
        (tb.1, Bound::Upper)
    } else {
        (best, bound_for(best, original_alpha, beta))
    }
}

/// A cut at a stalemate overstates it only by returning more than a draw, since a lower bound at or
/// below the draw score still holds there; so the two cuts before generation ask only then.
#[inline]
fn unless_stalemate(board: &Board, value: Score) -> Score {
    if value <= DRAW || has_legal_move(board) {
        value
    } else {
        DRAW
    }
}

#[cfg(test)]
mod tests;
