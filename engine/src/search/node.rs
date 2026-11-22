// SPDX-License-Identifier: GPL-3.0-or-later

//! `null_move`, `probcut` and `late_move` call `negamax` back, which is why they live here.

use cadence_core::position::Board;
use cadence_core::{
    Colour, MAX_MOVES, MAX_PLY, Move, MoveList, generate_legal, generate_noisy, has_legal_move,
};

#[cfg(debug_assertions)]
use super::Call;
use super::depth::{
    cut_node_reduction, extension, history_reduction, iir_reduction, losing_capture, move_history,
    null_reduction, pv_reduction, reduced_depth, reduction,
};
use super::pruning::{
    PROBCUT_REDUCTION, futile_node, has_non_pawn_material, lmp_index, probcut_bound,
};
use super::{Expected, Search, Via, bound_for, order_first, remember_killer};
use crate::conthist::{self, OFFSETS};
use crate::picker::{self, Picker, Tables};
use crate::position::Position;
use crate::score::{self, ABORTED, DRAW, INFINITE, Score, mated_in};
use crate::see;
use crate::syzygy::Wdl;
use crate::tt::Bound;

/// The key does not carry the fifty-move clock, so from here a cutoff could rest on a line the draw
/// has since cut short; only the entry's move is kept.
const CUTOFF_CLOCK: u8 = 90;

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
            Ok(found) => found,
            Err(score) => return score,
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

    /// The move comes back whatever the depth says, so the two halves return separately, and the
    /// hit's flag beside them.
    fn probe(
        &self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> (Move, Option<Score>, bool) {
        let Some(hit) = self.tt.probe(key) else {
            return (Move::NULL, None, false);
        };
        // A result whose depth reaches the draw may rest on a line the clock now cuts short.
        let clock = board.halfmove_clock();
        if u32::from(hit.depth) < depth
            || clock >= CUTOFF_CLOCK
            || u32::from(clock) + u32::from(hit.depth) >= 100
        {
            return (hit.mv, None, hit.pv);
        }
        let score = score::from_tt(hit.score, ply);
        let cutoff = match hit.bound {
            Bound::Exact => true,
            Bound::Lower => score >= beta,
            Bound::Upper => score <= alpha,
        };
        (hit.mv, cutoff.then_some(score), hit.pv)
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

    /// The one store, and where a node failing low takes its parent's flag.
    fn store(&mut self, key: u64, ply: usize, depth: u32, best: Score, mv: Move, bound: Bound) {
        // An aborted node's value is `ABORTED` and not a bound, and the draw score besides.
        debug_assert!(!self.run.aborted, "a store after the stop");
        let stored_depth = depth.min(u32::from(u8::MAX)) as u8;
        let score = score::to_tt(best, ply);
        // Nothing here beat alpha, so the move into it may have been the good one.
        let pv = self.tt_pv[ply] || (bound == Bound::Upper && ply > 0 && self.tt_pv[ply - 1]);
        self.tt.store(key, mv, score, stored_depth, bound, pv);
    }

    /// The mate bounds, which narrow the window in place, then the table and the tablebases: `Err`
    /// is the node's value, and otherwise the table's move and the floor and ceiling it keeps.
    fn lookup(
        &mut self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: &mut Score,
        beta: &mut Score,
    ) -> Result<(Move, (Score, Score)), Score> {
        (*alpha, *beta) = mate_window(ply, *alpha, *beta)?;
        let (alpha, beta) = (*alpha, *beta);
        let (tt_move, cutoff, hit_pv) = self.probe(board, key, depth, ply, alpha, beta);
        if let Some(score) = cutoff {
            return Err(score);
        }
        // Before any child is searched, since a child that fails low reads it, and before the
        // tablebases, whose stored entry reads it.
        self.tt_pv[ply] = beta > alpha + 1 || hit_pv;
        // After the table, so a hit saves the probe, and before the static readings, which an
        // ending the tables decide makes moot.
        let tb = self.tablebase(board, key, depth, ply, alpha, beta, tt_move)?;
        Ok((tt_move, tb))
    }

    /// `Err` is the node's value. Otherwise the floor and the ceiling a PV node keeps, unbounded at
    /// every other node and wherever no table answers.
    #[allow(
        clippy::too_many_arguments,
        reason = "passing these separately measured +0.21% faster than a shared node type (pull request 233 timing, 2026-11-08)"
    )]
    fn tablebase(
        &mut self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
        tt_move: Move,
    ) -> Result<(Score, Score), Score> {
        let open = (-INFINITE, INFINITE);
        let Some(tables) = self.probing else {
            return Ok(open);
        };
        // Only after a capture or a pawn move, where the answer is exact as to the fifty-move rule:
        // a later quiet move's position is reached through the entry this one stores.
        if board.halfmove_clock() != 0 || !tables.covers(board) {
            return Ok(open);
        }
        let Some(wdl) = tables.probe_wdl(board) else {
            return Ok(open);
        };
        let (score, bound) = match wdl {
            Wdl::Win => (score::tb_win_in(ply), Bound::Lower),
            Wdl::Loss => (score::tb_loss_in(ply), Bound::Upper),
            Wdl::Draw => (DRAW, Bound::Exact),
        };
        let cuts = match bound {
            Bound::Exact => true,
            Bound::Lower => score >= beta,
            Bound::Upper => score <= alpha,
        };
        if cuts {
            // The table's move stays, so a later visit that does not cut still tries it first.
            self.store(key, ply, depth, score, tt_move, bound);
            return Err(score);
        }
        // A mate inside the table's territory is still searched for, and found, under the bound.
        Ok(match bound {
            _ if beta == alpha + 1 => open,
            Bound::Lower => (score, INFINITE),
            _ => (-INFINITE, score),
        })
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

    /// A reduced search that beats alpha is re-run at full depth before it is believed.
    #[allow(
        clippy::too_many_arguments,
        reason = "passing these separately measured +0.21% faster than a shared node type (pull request 233 timing, 2026-11-08)"
    )]
    fn late_move(
        &mut self,
        board: &mut Position,
        child: u32,
        base: u32,
        history: i32,
        ply: usize,
        alpha: Score,
        pv: bool,
        kind: Expected,
    ) -> Score {
        // The history term first, then the PV and cut-node terms on what each leaves, each counted
        // where it acted.
        let shifted = history_reduction(base, history);
        self.stats.history_reduced_less.add_if(shifted < base);
        self.stats.history_reduced_more.add_if(shifted > base);
        let lessened = pv_reduction(shifted, pv);
        self.stats.lmr_pv_lessened.add_if(lessened < shifted);
        let r = cut_node_reduction(lessened, kind == Expected::Cut);
        self.stats.lmr_cut_reduced_more.add_if(r > lessened);
        if r == 0 {
            let full = self.derive(kind, Via::Null);
            return -self.negamax(board, child, ply + 1, -alpha - 1, -alpha, full);
        }
        self.stats.lmr_reductions.add_one();
        let reduced = reduced_depth(child, r);
        let cut = self.derive(kind, Via::Reduced);
        let score = -self.negamax(board, reduced, ply + 1, -alpha - 1, -alpha, cut);
        if score > alpha && !self.run.aborted {
            self.stats.lmr_researches.add_one();
            let full = self.derive(kind, Via::Null);
            return -self.negamax(board, child, ply + 1, -alpha - 1, -alpha, full);
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
        let admitted = self.heuristics.evals[ply].is_some_and(|eval| eval >= beta)
            && beta == alpha + 1
            && !score::is_decisive(beta)
            && board.plies_from_null() != 0
            && board.halfmove_clock() < 100;
        if !admitted {
            return None;
        }
        if !has_non_pawn_material(board) {
            self.stats.null_refused_material.add_one();
            return None;
        }
        self.stats.null_attempts.add_one();
        let reduced = depth.saturating_sub(1 + null_reduction(depth));
        // No move, so the child reads none one ply up.
        self.heuristics.frames[ply] = None;
        #[cfg(debug_assertions)]
        {
            self.moved[ply] = Move::NULL;
        }
        let _ = board.make_null_move();
        // The pass hands the move over, and the null move cuts only where that child fails low.
        let score = -self.negamax(board, reduced, ply + 1, -beta, -beta + 1, Expected::All);
        board.unmake_null_move();
        if self.run.aborted {
            return Some(ABORTED);
        }
        if score >= beta {
            self.stats.null_cutoffs.add_one();
            return Some(if score::is_decisive(score) {
                beta
            } else {
                score
            });
        }
        None
    }

    /// Each capture is screened by the quiescence search before the reduced search; a cutoff is
    /// never on the decided scale.
    fn probcut(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
        kind: Expected,
    ) -> Option<Score> {
        let eval = self.heuristics.evals[ply];
        let Some(raised) = probcut_bound(&self.tunables, eval, depth, alpha, beta) else {
            // Asked as though the window were null, so the counter sees only nodes the window
            // alone refused.
            if beta != alpha + 1
                && probcut_bound(&self.tunables, eval, depth, beta - 1, beta).is_some()
            {
                self.stats.probcut_refused_window.add_one();
            }
            return None;
        };
        if board.halfmove_clock() >= 100 {
            return None;
        }
        let eval = eval?;
        self.stats.probcut_attempts.add_one();
        let mut noisy = generate_noisy(board);
        picker::sort_noisy(board, &mut noisy);
        let child = depth.saturating_sub(PROBCUT_REDUCTION);
        for m in noisy.iter() {
            self.stats.probcut_exchanges.add_one();
            if !see::see_ge(board, m, raised - eval) {
                continue;
            }
            self.make(board, ply, m);
            let mut score = -self.quiesce(board, ply + 1, -raised, -raised + 1);
            if score >= raised {
                self.stats.probcut_searches.add_one();
                let flipped = self.derive(kind, Via::Null);
                score = -self.negamax(board, child, ply + 1, -raised, -raised + 1, flipped);
            }
            board.unmake_move(m);
            if self.run.aborted {
                return Some(ABORTED);
            }
            if score >= raised {
                self.stats.probcut_cutoffs.add_one();
                return Some(if score::is_decisive(score) {
                    raised
                } else {
                    score
                });
            }
        }
        None
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

/// No score at `ply` beats mating at the next ply or falls below being mated there, so `Err` is a
/// bound that already answers the window, and otherwise the window is narrowed to that range.
fn mate_window(ply: usize, alpha: Score, beta: Score) -> Result<(Score, Score), Score> {
    let (worst, best) = (mated_in(ply), score::mate_in(ply + 1));
    if worst >= beta {
        return Err(worst);
    }
    if best <= alpha {
        return Err(best);
    }
    let (low, high) = (alpha.max(worst), beta.min(best));
    // Not where an open window would be left one point wide, since a node's kind reads the width.
    Ok(if high > low + 1 || beta <= alpha + 1 {
        (low, high)
    } else {
        (alpha, beta)
    })
}

#[cfg(test)]
mod tests;
