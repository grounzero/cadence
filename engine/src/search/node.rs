// SPDX-License-Identifier: GPL-3.0-or-later

//! `null_move`, `probcut` and `late_move` call `negamax` back, which is why they live here.

use cadence_core::position::Board;
use cadence_core::{Colour, MAX_PLY, Move, MoveList, generate_legal, generate_noisy};

use super::depth::{extension, history_reduction, null_reduction, reduction};
use super::pruning::{
    PROBCUT_REDUCTION, futile_node, has_non_pawn_material, lmp_index, probcut_bound,
};
use super::{Search, bound_for, order_first, remember_killer};
use crate::eval;
use crate::picker;
use crate::position::Position;
use crate::score::{self, DRAW, INFINITE, Score, mated_in};
use crate::see;
use crate::tt::Bound;

impl Search<'_> {
    #[must_use]
    pub fn node(&mut self, board: &mut Position, depth: u32, ply: usize) -> Score {
        self.node_window(board, depth, ply, -INFINITE, INFINITE)
    }

    #[must_use]
    pub fn node_window(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Score {
        // So the extension cap reads this depth, not what the last iteration left.
        self.root_depth = depth;
        self.negamax(board, depth, ply, alpha, beta)
    }

    /// Fail-soft. After an abort the value means nothing, and every caller discards it.
    pub(super) fn negamax(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        mut alpha: Score,
        beta: Score,
    ) -> Score {
        // The quiescence search counts this node.
        if depth == 0 {
            return self.quiesce(board, ply, alpha, beta);
        }
        self.visit(ply);
        self.table.clear(ply);
        // At every node, so that `nodes N` stops at N and not at N plus a subtree.
        if self.out_of_time() {
            return DRAW;
        }

        if board.is_repetition() {
            return DRAW;
        }

        // Above the sort, where the first read past the arrays would be; release has only a bounds
        // check.
        if ply >= MAX_PLY {
            return eval::evaluate(board);
        }

        // Before generation: that saving is most of what the table is for.
        let key = board.key();
        let (tt_move, cutoff) = self.probe(board, key, depth, ply, alpha, beta);
        if let Some(score) = cutoff {
            return score;
        }

        // Written even if unread here: a rule at `p + 2` compares against it.
        let in_check = board.in_check();
        let pawn_key = board.pawn_key();
        let us_eval = board.side_to_move();
        self.evals[ply] = self.corrected_eval(board, in_check, pawn_key, us_eval);

        // Above the null move: below it the rule would be nothing but its own error case.
        if let Some(bound) = self.reverse_futility(board, depth, ply, alpha, beta) {
            return bound;
        }

        if let Some(score) = self.null_move(board, depth, ply, alpha, beta) {
            return score;
        }

        // Last in the preamble: the static readings run first and the null move's pass after
        // them, and this is the one rule that trusts a reduced search of a real move.
        if let Some(score) = self.probcut(board, depth, ply, alpha, beta) {
            return score;
        }

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
        let killers = self.order(board, &mut legal, tt_move, us, ply);

        // Below the null move, unlike the other margin tests: nothing between writes what it reads,
        // so the placement saves the test at every node the null move cuts.
        let futile = futile_node(self.evals[ply], depth, alpha);
        self.futility_nodes += u64::from(futile);

        // Beside the margin, not above it: they overlap on 43% of one population, and whichever is
        // asked first keeps the moves it takes.
        let give_up = lmp_index(&self.tunables, in_check, depth, legal.len());
        self.lmp_nodes += u64::from(give_up.is_some());

        let original_alpha = alpha;
        let mut best = -INFINITE;
        let mut best_move = Move::NULL;
        for (i, m) in legal.iter().enumerate() {
            if self.futile(board, futile, m, i) {
                continue;
            }
            // Before the move is made, which is the whole saving; above the reduction, which never
            // sees a move given up.
            if self.given_up(board, give_up, m, killers, i) {
                continue;
            }
            board.make_move(m);
            // Free after `make_move`, and also the reduction's check exemption.
            let gives_check = board.in_check();
            let ext = extension(gives_check, ply + 1, self.root_depth);
            let child = depth - 1 + ext;
            // The first move gets the node's own window; every later one is asked the null-window
            // question first.
            let mut score = if i == 0 {
                -self.negamax(board, child, ply + 1, -beta, -alpha)
            } else {
                // The index decides whether to reduce, the history score by how much.
                let base = reduction(in_check, gives_check, m, killers, depth, i);
                self.late_move(board, child, base, self.history.get(us, m), ply, alpha)
            };
            // None at or above beta, which is the bound already returned, and none at a null-window
            // node.
            if i > 0 && !self.aborted && score > alpha && score < beta {
                score = -self.negamax(board, child, ply + 1, -beta, -alpha);
            }
            board.unmake_move(m);
            if self.aborted {
                return DRAW;
            }
            if score > best {
                best = score;
                best_move = m;
                if score > alpha {
                    alpha = score;
                    self.table.update(ply, m);
                    if alpha >= beta {
                        remember_killer(&mut self.killers[ply], m);
                        // The moves it beat include ones the margin and count skipped, so
                        // unsearched moves are debited too: 29.2% of debits over the bench at
                        // 0.4.8.
                        self.remember_history(us, &legal.as_slice()[..i], m, depth);
                        break;
                    }
                }
            }
        }
        // Fail-soft, so the bound follows the value, not the window it was found in. Nothing an
        // aborted search computed is stored: the loop returns before this.
        let bound = bound_for(best, original_alpha, beta);
        self.tt.store(
            key,
            best_move,
            score::to_tt(best, ply),
            depth.min(u32::from(u8::MAX)) as u8,
            bound,
        );
        self.remember_correction(pawn_key, us_eval, ply, best, best_move, bound, depth);
        best
    }

    fn order(
        &self,
        board: &Board,
        legal: &mut MoveList,
        tt_move: Move,
        us: Colour,
        ply: usize,
    ) -> [Move; 2] {
        let ordered = usize::from(order_first(legal, tt_move));
        let killers = self.killers[ply];
        picker::sort_from(board, legal, ordered, killers, self.history.side(us));
        killers
    }

    /// The move comes back whatever the depth says, so the two halves return separately.
    fn probe(
        &self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> (Move, Option<Score>) {
        let Some(hit) = self.tt.probe(key) else {
            return (Move::NULL, None);
        };
        if u32::from(hit.depth) < depth || board.halfmove_clock() >= 100 {
            return (hit.mv, None);
        }
        let score = score::from_tt(hit.score, ply);
        let cutoff = match hit.bound {
            Bound::Exact => true,
            Bound::Lower => score >= beta,
            Bound::Upper => score <= alpha,
        };
        (hit.mv, cutoff.then_some(score))
    }

    /// A reduced search that beats alpha is re-run at full depth before it is believed.
    fn late_move(
        &mut self,
        board: &mut Position,
        child: u32,
        base: u32,
        history: i32,
        ply: usize,
        alpha: Score,
    ) -> Score {
        let r = history_reduction(base, history);
        self.history_reduced_less += u64::from(r < base);
        self.history_reduced_more += u64::from(r > base);
        if r == 0 {
            return -self.negamax(board, child, ply + 1, -alpha - 1, -alpha);
        }
        self.lmr_reductions += 1;
        let reduced = child.saturating_sub(r).max(1);
        let score = -self.negamax(board, reduced, ply + 1, -alpha - 1, -alpha);
        if score > alpha && !self.aborted {
            self.lmr_researches += 1;
            return -self.negamax(board, child, ply + 1, -alpha - 1, -alpha);
        }
        score
    }

    fn null_move(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Option<Score> {
        let admitted = self.evals[ply].is_some_and(|eval| eval >= beta)
            && beta == alpha + 1
            && !score::is_mate(beta)
            && board.plies_from_null() != 0
            && board.halfmove_clock() < 100;
        if !admitted {
            return None;
        }
        if !has_non_pawn_material(board) {
            self.null_refused_material += 1;
            return None;
        }
        self.null_attempts += 1;
        let reduced = depth.saturating_sub(1 + null_reduction(depth));
        let _ = board.make_null_move();
        let score = -self.negamax(board, reduced, ply + 1, -beta, -beta + 1);
        board.unmake_null_move();
        if self.aborted {
            return Some(DRAW);
        }
        if score >= beta {
            self.null_cutoffs += 1;
            return Some(if score::is_mate(score) { beta } else { score });
        }
        None
    }

    /// Each capture is screened by the quiescence search before the reduced search; a cutoff is
    /// never on the mate scale.
    fn probcut(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Option<Score> {
        let eval = self.evals[ply];
        let Some(raised) = probcut_bound(eval, depth, alpha, beta) else {
            // Asked as though the window were null, so the counter sees only nodes the window
            // alone refused.
            if beta != alpha + 1 && probcut_bound(eval, depth, beta - 1, beta).is_some() {
                self.probcut_refused_window += 1;
            }
            return None;
        };
        if board.halfmove_clock() >= 100 {
            return None;
        }
        let eval = eval?;
        self.probcut_attempts += 1;
        let mut noisy = generate_noisy(board);
        picker::sort_noisy(board, &mut noisy);
        let child = depth.saturating_sub(PROBCUT_REDUCTION);
        for m in noisy.iter() {
            if see::see(board, m) < raised - eval {
                continue;
            }
            board.make_move(m);
            let mut score = -self.quiesce(board, ply + 1, -raised, -raised + 1);
            if score >= raised {
                self.probcut_searches += 1;
                score = -self.negamax(board, child, ply + 1, -raised, -raised + 1);
            }
            board.unmake_move(m);
            if self.aborted {
                return Some(DRAW);
            }
            if score >= raised {
                self.probcut_cutoffs += 1;
                return Some(if score::is_mate(score) { raised } else { score });
            }
        }
        None
    }
}
