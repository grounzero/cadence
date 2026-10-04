// SPDX-License-Identifier: GPL-3.0-or-later

//! One node of the main search and everything that searches back into it. `null_move`,
//! `probcut` and `late_move` each call `negamax` again, which is why they live here and not
//! beside their rules.

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
    /// One interior node of the main search, at the `ply` and `depth` given, searched with the
    /// full window.
    #[must_use]
    pub fn node(&mut self, board: &mut Position, depth: u32, ply: usize) -> Score {
        self.node_window(board, depth, ply, -INFINITE, INFINITE)
    }

    /// The same node, searched with the window given instead of the full one. The seam a null
    /// window is reached through.
    #[must_use]
    pub fn node_window(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Score {
        // As though `depth` were the iteration's, so that the extension's ply cap means here
        // what it means in a search rather than reading whatever the last iteration left
        // behind.
        self.root_depth = depth;
        self.negamax(board, depth, ply, alpha, beta)
    }

    /// Negamax with alpha-beta, fail-soft. The value returned after an abort is meaningless and
    /// is discarded by every caller.
    pub(super) fn negamax(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        mut alpha: Score,
        beta: Score,
    ) -> Score {
        // The horizon: the quiescence search takes over, and counts the node.
        if depth == 0 {
            return self.quiesce(board, ply, alpha, beta);
        }
        self.visit(ply);
        self.table.clear(ply);
        // At every node, so that `nodes N` stops at N and not at N plus a subtree.
        if self.out_of_time() {
            return DRAW;
        }

        // A repeated position is a draw wherever it is met: twofold inside the tree, threefold
        // against the game history.
        if board.is_repetition() {
            return DRAW;
        }

        // The ply bound, above the sort because that is where the first read past the arrays
        // would be. In release a read past their end is a bounds check rather than the
        // assertion a debug build gets.
        if ply >= MAX_PLY {
            return eval::evaluate(board);
        }

        // The table, before the moves are generated: that saving is most of what it is for.
        let key = board.key();
        let (tt_move, cutoff) = self.probe(board, key, depth, ply, alpha, beta);
        if let Some(score) = cutoff {
            return score;
        }

        // The static evaluation, written whether or not anything below reads it at this node:
        // the stack is how a rule at ply `p` compares its own reading against the one at `p -
        // 2`, so a hole here is a wrong answer there, not a saving. A node in check writes
        // `None` rather than a number, because what the evaluation measures is a position
        // nobody is about to win material in, and a check is exactly that claim being
        // contested.
        let in_check = board.in_check();
        let pawn_key = board.pawn_key();
        let us_eval = board.side_to_move();
        self.evals[ply] = self.corrected_eval(board, in_check, pawn_key, us_eval);

        // The margin, above the null move because the node's preamble runs the margin tests
        // there and because below it the rule would be nothing but its own error case: see
        // [`Search::reverse_futility`].
        if let Some(bound) = self.reverse_futility(board, depth, ply, alpha, beta) {
            return bound;
        }

        if let Some(score) = self.null_move(board, depth, ply, alpha, beta) {
            return score;
        }

        // Below the null move, which is ADR-0008's order: this is the one preamble rule that
        // trusts a reduced search of a real move rather than a static reading or a pass.
        if let Some(score) = self.probcut(board, depth, ply, alpha, beta) {
            return score;
        }

        let mut legal = generate_legal(board);
        if legal.is_empty() {
            return if in_check { mated_in(ply) } else { DRAW };
        }
        // After the mate check: a mate delivered on the hundredth half-move is a mate, not a
        // draw.
        if board.halfmove_clock() >= 100 {
            return DRAW;
        }
        // The history row is the side to move's, read here and again per move below, so `us` is
        // taken once at the node rather than after a move has changed it.
        let us = board.side_to_move();
        let killers = self.order(board, &mut legal, tt_move, us, ply);

        // The margin, read once with the node's own alpha. The interior node's preamble runs
        // the margin tests beside the static evaluation and above the null move, and this sits
        // below it instead: nothing between the two writes what the test reads, and this rule
        // returns no score of its own, so the sequence is unaffected and what the placement
        // saves is the test at every node the null move cuts.
        let futile = futile_node(self.evals[ply], depth, alpha);
        self.futility_nodes += u64::from(futile);

        // The count, read once against the list this node actually holds, because the rule is
        // off at a node the count already admits whole. It sits beside the margin and not above
        // it: the two act on one population and overlap over 43% of it, and whichever is asked
        // first keeps the moves it takes.
        let give_up = lmp_index(&self.tunables, in_check, depth, legal.len());
        self.lmp_nodes += u64::from(give_up.is_some());

        let original_alpha = alpha;
        let mut best = -INFINITE;
        let mut best_move = Move::NULL;
        for (i, m) in legal.iter().enumerate() {
            if self.futile(board, futile, m, i) {
                continue;
            }
            // Before the move is made, which is the whole of what the rule buys: a move given
            // up here costs the node its exemption tests and nothing else. It is also above the
            // reduction rather than beside it, and the two are not alternatives at a node where
            // both would fire -- the move is gone and [`reduction`] never sees it.
            if self.given_up(board, give_up, m, killers, i) {
                continue;
            }
            board.make_move(m);
            // The check extension, on the child's own state: `make_move` has just recomputed
            // the checkers, so asking costs nothing that was not already spent. The same read
            // is the reduction's check exemption below, so it is taken once and named.
            let gives_check = board.in_check();
            let ext = extension(gives_check, ply + 1, self.root_depth);
            let child = depth - 1 + ext;
            // The first move gets the window this node was given and every move behind it the
            // narrower question. The module doc has the window, what it buys and what it costs.
            let mut score = if i == 0 {
                -self.negamax(board, child, ply + 1, -beta, -alpha)
            } else {
                // The index decides whether this move is reduced at all and the score by how
                // much; [`history_reduction`] has why those are not the same reading.
                let base = reduction(in_check, gives_check, m, killers, depth, i);
                self.late_move(board, child, base, self.history.get(us, m), ply, alpha)
            };
            // No re-search at or above beta, which is already the bound this node returns, and
            // none at a node whose own window is a null one, where there is no room for a score
            // to ask for one.
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
                        // The moves it beat are the ones before it in the sorted list, and the
                        // margin and the count above both skip moves inside it, so a quiet
                        // cutoff debits moves that failed to cut and moves that were never
                        // searched alike. Measured over the bench positions at 0.4.8 that is
                        // 29.2% of the debits, and separating the two changes what the table
                        // holds, which is a change with its own test.
                        self.remember_history(us, &legal.as_slice()[..i], m, depth);
                        break;
                    }
                }
            }
        }
        // Fail-soft, so the bound follows the value and not the window it was found in. Nothing
        // an aborted search computed is stored: the loop above returns before this line.
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

    /// Put the node's move list in the order it will be searched, and hand back the killers the
    /// caller needs again below. Three stages and one sort.
    pub(super) fn order(
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

    /// The transposition table at an interior node: the move a hit named, and the score to
    /// return where the stored bound answers this node's question outright. The move comes back
    /// whatever the depth says, which is why the two halves come back separately.
    pub(super) fn probe(
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

    /// The null-window search of one move behind a node's first, reduced by as many plies as
    /// [`reduction`] and [`history_reduction`] allow. A reduced search that beats alpha is
    /// re-run at the full child depth before its answer is believed.
    pub(super) fn late_move(
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

    /// Null-move pruning at one node: `Some` is the cutoff, `None` means search the node.
    /// Refused in check, at a full window, on a mate-scale beta, below beta, at a position the
    /// null move itself reached, on a halfmove clock at the limit, and where the side to move
    /// has nothing but pawns beside the king ([`has_non_pawn_material`]).
    pub(super) fn null_move(
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

    /// The capture probe at one node: a capture whose exchange could carry the static evaluation
    /// to the raised beta is screened by the quiescence search and then searched
    /// [`PROBCUT_REDUCTION`] plies shallower, and the first to stand at or above that bound cuts
    /// the node. `Some` is the cutoff, never on the mate scale; `None` means search the node.
    pub(super) fn probcut(
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
