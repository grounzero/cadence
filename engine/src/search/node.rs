// SPDX-License-Identifier: GPL-3.0-or-later

//! `null_move`, `probcut` and `late_move` call `negamax` back, which is why they live here.

use cadence_core::position::Board;
use cadence_core::{
    Colour, MAX_MOVES, MAX_PLY, Move, MoveList, generate_legal, generate_noisy, has_legal_move,
};

#[cfg(debug_assertions)]
use super::Call;
use super::depth::{
    asks_singular, child_depth, counted, cut_node_reduction, extension, granted, history_reduction,
    iir_reduction, losing_capture, move_history, null_reduction, pv_reduction, reduced_depth,
    reduction, singular_admission, verification_depth,
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
use crate::tt::{Bound, Hit};

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
        // So the extension caps read this depth, not what the last iteration left, and no frame,
        // flag, count or exclusion is read from an earlier run.
        self.root_depth = depth;
        self.heuristics.frames.fill(None);
        self.tt_pv.fill(false);
        self.extensions.fill(0);
        self.excluded.fill(Move::NULL);
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
        beta: Score,
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
        let (tt_move, tb, hit) = match self.lookup(board, key, depth, ply, alpha, beta) {
            Ok(found) => found,
            Err(score) => return score,
        };

        // Written even if unread here: a rule at `p + 2` compares against it.
        let in_check = board.in_check();
        let pawn_key = board.pawn_key();
        let us_eval = board.side_to_move();
        self.heuristics.evals[ply] = self.corrected_eval(board, ply, in_check, pawn_key, us_eval);

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
        let depth = self.iir_depth(depth, ply, pv, in_check, tt_move);

        let mut legal = match generate(board, in_check, ply) {
            Ok(legal) => legal,
            Err(score) => return score,
        };
        // After generation, so the list holds the table's move, and before the sort, whose row at
        // this ply the verification's own picker takes first.
        let singular = match self.singular_verdict(board, &legal, hit, depth, ply, kind, beta) {
            Ok(singular) => singular,
            Err(score) => return score,
        };
        // Before any move is made: the history row read here and per move below is the mover's.
        let us = board.side_to_move();
        let (killers, mut picker) = self.order(board, &mut legal, tt_move, us, ply);

        // Below the null move, unlike the other margin tests: nothing between writes what it reads,
        // so the placement saves the test at every node the null move cuts.
        let futile = futile_node(self.heuristics.evals[ply], depth, alpha);
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
            if self.skipped(board, ply, futile, give_up, m, killers, i, quiet_index) {
                continue;
            }
            searched.set(i);
            let losing = losing_capture(m, i, picker.lost());
            self.make(board, ply, m);
            // Free after `make_move`, and also the reduction's check exemption.
            let gives_check = board.in_check();
            let ext = self.extend(gives_check, singular && m == tt_move, ply);
            let child = child_depth(depth, ext);
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
        self.store(key, ply, depth, best, best_move, bound);
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

    /// A check or a singular table move takes one ply, never two, held to the line's headroom where
    /// the set asks. The child's count is written here, after the move is made, for its searches.
    pub(super) fn extend(&mut self, check: bool, singular: bool, ply: usize) -> i32 {
        let taken = self.extensions[ply];
        let asked = extension(check || singular, ply + 1, self.root_depth).cast_signed();
        let ext = if asks_singular(&self.tunables) {
            granted(asked, taken, self.root_depth)
        } else {
            asked
        };
        self.stats
            .singular_extensions
            .add_if(singular && !check && ext > 0);
        self.extensions[ply + 1] = counted(taken, ext);
        ext
    }

    /// `Ok` says whether the table's move is singular: this node searched again without it falls
    /// short of the singular beta. `Err` is the node's value, after a stop or where the search
    /// without it reaches a singular beta at or above beta, which multicut returns.
    #[allow(
        clippy::too_many_arguments,
        reason = "the node's own values, which each helper of the node takes separately"
    )]
    fn singular_verdict(
        &mut self,
        board: &mut Position,
        legal: &MoveList,
        hit: Option<Hit>,
        depth: u32,
        ply: usize,
        kind: Expected,
        beta: Score,
    ) -> Result<bool, Score> {
        let listed = hit.filter(|h| legal.contains(h.mv));
        let admitted = singular_admission(listed, depth, ply, &self.tunables);
        let (Some(hit), Some(s_beta)) = (listed, admitted) else {
            return Ok(false);
        };
        // An open window's verification is expected to fail low where the move is singular.
        let kind = if kind == Expected::Pv {
            Expected::All
        } else {
            kind
        };
        let v = self.singular(board, depth, ply, hit.mv, s_beta, kind);
        if self.run.aborted {
            return Err(ABORTED);
        }
        if v >= s_beta && s_beta >= beta {
            self.stats.multicut_cutoffs.add_one();
            return Err(s_beta);
        }
        Ok(v < s_beta)
    }

    /// This node again at the verification depth with `excluded` left out, on the null window
    /// below the singular beta. The slot is cleared before an abort is read.
    pub(super) fn singular(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        excluded: Move,
        s_beta: Score,
        kind: Expected,
    ) -> Score {
        self.stats.singular_tests.add_one();
        self.stats.singular_nested.add_if(self.verifying > 0);
        let before = self.run.nodes;
        self.verifying += 1;
        self.excluded[ply] = excluded;
        let at = verification_depth(depth);
        let v = self.negamax(board, at, ply, s_beta - 1, s_beta, kind);
        self.excluded[ply] = Move::NULL;
        self.verifying -= 1;
        if self.verifying == 0 {
            self.stats.singular_nodes.add(self.run.nodes - before);
        }
        if self.run.aborted {
            return ABORTED;
        }
        v
    }

    /// A method so the counter is written where the rule runs. The excluded node has no table move,
    /// and its depth is already halved.
    fn iir_depth(
        &mut self,
        depth: u32,
        ply: usize,
        pv: bool,
        in_check: bool,
        tt_move: Move,
    ) -> u32 {
        if !self.excluded[ply].is_null() {
            return depth;
        }
        let r = iir_reduction(depth, pv, in_check, tt_move != Move::NULL, self.iir_plies);
        self.stats.iir_nodes.add_if(r > 0);
        depth - r
    }

    /// The move comes back whatever the depth says, so the two halves return separately, and the
    /// hit's flag and the hit beside them.
    fn probe(
        &self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> (Move, Option<Score>, bool, Option<Hit>) {
        let Some(hit) = self.tt.probe(key) else {
            return (Move::NULL, None, false, None);
        };
        if u32::from(hit.depth) < depth || board.halfmove_clock() >= 100 {
            return (hit.mv, None, hit.pv, Some(hit));
        }
        let score = score::from_tt(hit.score, ply);
        let cutoff = match hit.bound {
            Bound::Exact => true,
            Bound::Lower => score >= beta,
            Bound::Upper => score <= alpha,
        };
        (hit.mv, cutoff.then_some(score), hit.pv, Some(hit))
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
            let extensions = self.extensions.get(ply).copied().unwrap_or(0);
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
                    extensions,
                    verifying: self.verifying,
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

    /// The one store, and where a node failing low takes its parent's flag. The excluded node's
    /// value answers another question, and once stored nothing would tell the two apart.
    fn store(&mut self, key: u64, ply: usize, depth: u32, best: Score, mv: Move, bound: Bound) {
        if !self.excluded[ply].is_null() {
            return;
        }
        // An aborted node's value is `ABORTED` and not a bound, and the draw score besides.
        debug_assert!(!self.run.aborted, "a store after the stop");
        let stored_depth = depth.min(u32::from(u8::MAX)) as u8;
        let score = score::to_tt(best, ply);
        // Nothing here beat alpha, so the move into it may have been the good one.
        let pv = self.tt_pv[ply] || (bound == Bound::Upper && ply > 0 && self.tt_pv[ply - 1]);
        self.tt.store(key, mv, score, stored_depth, bound, pv);
    }

    /// The transposition table, then the tablebases: `Err` is the node's value, and otherwise the
    /// table's move, the floor and ceiling the node keeps, and the entry the move came from.
    fn lookup(
        &mut self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Result<Found, Score> {
        // The excluded node is its parent's position without one move, which neither the table nor
        // the tablebases can answer for; its flag stays as its own search wrote it.
        if !self.excluded[ply].is_null() {
            return Ok((Move::NULL, (-INFINITE, INFINITE), None));
        }
        let (tt_move, cutoff, hit_pv, hit) = self.probe(board, key, depth, ply, alpha, beta);
        if let Some(score) = cutoff {
            return Err(score);
        }
        // Before any child is searched, since a child that fails low reads it, and before the
        // tablebases, whose stored entry reads it.
        self.tt_pv[ply] = beta > alpha + 1 || hit_pv;
        // After the table, so a hit saves the probe, and before the static readings, which an
        // ending the tables decide makes moot.
        let tb = self.tablebase(board, key, depth, ply, alpha, beta, tt_move)?;
        Ok((tt_move, tb, hit))
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
        // At the excluded node a pass would say no move is needed, where the question is whether
        // another suffices.
        let admitted = self.excluded[ply].is_null()
            && self.heuristics.evals[ply].is_some_and(|eval| eval >= beta)
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
        // No move, so the child reads none one ply up, and nothing is extended.
        self.heuristics.frames[ply] = None;
        self.extensions[ply + 1] = self.extensions[ply];
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
        // The excluded node's capture could be the move it leaves out.
        if !self.excluded[ply].is_null() {
            return None;
        }
        let eval = self.heuristics.evals[ply];
        let Some(raised) = probcut_bound(eval, depth, alpha, beta) else {
            // Asked as though the window were null, so the counter sees only nodes the window
            // alone refused.
            if beta != alpha + 1 && probcut_bound(eval, depth, beta - 1, beta).is_some() {
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
            // A capture searched shallower is extended by nothing.
            self.extensions[ply + 1] = self.extensions[ply];
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

/// What the lookup hands the node: the table's move, the floor and ceiling it keeps, and the entry.
type Found = (Move, (Score, Score), Option<Hit>);

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

/// The legal moves, or the node's score where none is searched. The draw on the hundredth half-move
/// is asked after the mate check, since mate there is mate.
fn generate(board: &Board, in_check: bool, ply: usize) -> Result<MoveList, Score> {
    let legal = generate_legal(board);
    if legal.is_empty() {
        return Err(if in_check { mated_in(ply) } else { DRAW });
    }
    if board.halfmove_clock() >= 100 {
        return Err(DRAW);
    }
    Ok(legal)
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
mod tests {
    use std::sync::atomic::AtomicBool;

    use cadence_core::position::Board;
    use cadence_core::{MAX_PLY, Move, generate_legal, parse_uci};

    use crate::conthist;
    use crate::eval;
    use crate::position::Position;
    use crate::score::{self, ABORTED, Score};
    use crate::search::{
        Call, Expected, Limits, SINGULAR_MARGIN, Search, Via, singular_beta, verification_depth,
    };
    use crate::tt::{Bound, Table};
    use crate::tune::{PARAMS, Tunable, Tunables};

    /// What a net's search reads, the set under which the gates below ask their question.
    const DEFAULT: Tunables = Tunables::DEFAULT;

    const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

    /// Far above either position's value, so no preamble rule cuts the node and every child is
    /// searched as the parent asked.
    const HIGH: Score = 2_000;

    fn searcher<'a>(stop: &'a AtomicBool, tt: &'a Table) -> Search<'a> {
        let mut search = Search::new(stop, tt);
        search.trace_calls();
        search
    }

    /// Every call recorded at one ply, in order: at the ply below a call site, its children alone.
    fn at(search: &Search<'_>, ply: usize) -> Vec<Call> {
        search
            .calls()
            .iter()
            .filter(|c| c.ply == ply)
            .copied()
            .collect()
    }

    /// Below the reduction's depth, so every child is a full-depth null window and takes the
    /// opposite of a cut parent, and a cut under an all parent.
    #[test]
    fn a_null_window_node_hands_its_children_the_other_kind() {
        let stop = AtomicBool::new(false);
        for (parent, child) in [
            (Expected::Cut, Expected::All),
            (Expected::All, Expected::Cut),
        ] {
            let tt = Table::new(1).expect("a one mebibyte table");
            let mut search = searcher(&stop, &tt);
            let mut board = Position::new(Board::from_fen(MIDDLEGAME).expect("the middlegame"));
            let _ = search.node_expected(&mut board, 2, 0, HIGH, HIGH + 1, parent);
            assert_eq!(
                at(&search, 0).iter().map(|c| c.kind).collect::<Vec<_>>(),
                [parent]
            );
            let children = at(&search, 1);
            assert!(children.len() > 1, "under {parent:?}: too few children");
            for (i, c) in children.iter().enumerate() {
                let via = if i == 0 { Via::First } else { Via::Null };
                assert_eq!(c.asked, Some((parent, via)), "under {parent:?}, child {i}");
                assert_eq!(c.kind, child, "under {parent:?}, child {i}");
            }
        }
    }

    /// One late move searched alone from the middlegame at depth four, and the calls one ply below.
    fn late_move_alone(base: u32, alpha: Score, parent: Expected) -> Vec<Call> {
        let stop = AtomicBool::new(false);
        let root = Board::from_fen(MIDDLEGAME).expect("the middlegame");
        let m = parse_uci(&generate_legal(&root), "h2h3").expect("a legal move");
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let mut board = Position::new(root);
        search.root_depth = 4;
        search.make(&mut board, 0, m);
        let pv = parent == Expected::Pv;
        let _ = search.late_move(&mut board, 4, base, 0, 0, alpha, pv, parent);
        at(&search, 1)
    }

    /// The reduced search's child is expected to cut whatever the parent, and the re-search a
    /// reduced move earns runs at full depth and flips the parent. A high alpha is never beaten, so
    /// nothing is searched again; a low one always is.
    #[test]
    fn a_reduced_late_moves_child_is_a_cut_node_and_its_re_search_flips_the_parent() {
        use Expected::{All, Cut, Pv};
        // Two plies off four, one more at a cut parent and one taken back at a PV parent.
        let cases = [
            (HIGH, Cut, vec![(Via::Reduced, Cut, 1)]),
            (HIGH, All, vec![(Via::Reduced, Cut, 2)]),
            (HIGH, Pv, vec![(Via::Reduced, Cut, 3)]),
            (
                -HIGH,
                Cut,
                vec![(Via::Reduced, Cut, 1), (Via::Null, All, 4)],
            ),
            (
                -HIGH,
                All,
                vec![(Via::Reduced, Cut, 2), (Via::Null, Cut, 4)],
            ),
        ];
        for (alpha, parent, expected) in cases {
            let read: Vec<(Via, Expected, u32)> = late_move_alone(2, alpha, parent)
                .iter()
                .map(|c| {
                    let (from, via) = c.asked.expect("a derived kind");
                    assert_eq!(from, parent, "alpha {alpha}, {parent:?}");
                    (via, c.kind, c.depth)
                })
                .collect();
            assert_eq!(read, expected, "alpha {alpha}, {parent:?}");
        }
    }

    /// With no reduction the move is searched once, at full depth, flipping the parent.
    #[test]
    fn an_unreduced_late_move_is_searched_once_at_full_depth() {
        use Expected::{All, Cut, Pv};
        for (alpha, parent, child) in [
            (HIGH, Cut, All),
            (HIGH, All, Cut),
            (HIGH, Pv, Cut),
            (-HIGH, Cut, All),
            (-HIGH, All, Cut),
        ] {
            let read: Vec<_> = late_move_alone(0, alpha, parent)
                .iter()
                .map(|c| (c.asked, c.kind, c.depth))
                .collect();
            assert_eq!(
                read,
                [(Some((parent, Via::Null)), child, 4)],
                "alpha {alpha}, {parent:?}"
            );
        }
    }

    #[test]
    fn the_null_moves_child_is_an_all_node() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let root = Board::from_fen(MIDDLEGAME).expect("the middlegame");
        let m = parse_uci(&generate_legal(&root), "h2h3").expect("a legal move");
        let mut board = Position::new(root);
        search.root_depth = DEPTH;
        search.make(&mut board, 0, m);
        search.heuristics.evals[1] = Some(BETA + 200);
        let _ = search.null_move(&mut board, DEPTH, 1, BETA - 1, BETA);
        assert!(
            search.stats.null_attempts.get() > 0,
            "the null move was refused"
        );
        let read: Vec<_> = at(&search, 2).iter().map(|c| (c.asked, c.kind)).collect();
        assert_eq!(read, [(None, Expected::All)]);
    }

    /// A queen left to a pawn: the capture clears any raised beta, so the reduced search runs.
    #[test]
    fn probcuts_child_flips_the_parent() {
        const HANGING: &str = "4k3/8/8/3q4/4P3/8/8/4K3 w - - 0 1";
        let stop = AtomicBool::new(false);
        for (parent, child) in [
            (Expected::Cut, Expected::All),
            (Expected::All, Expected::Cut),
        ] {
            let tt = Table::new(1).expect("a one mebibyte table");
            let mut search = searcher(&stop, &tt);
            let root = Board::from_fen(HANGING).expect("the hanging queen");
            let static_eval = eval::evaluate(&root);
            let mut board = Position::new(root);
            search.root_depth = 6;
            search.heuristics.evals[0] = Some(static_eval);
            let beta = static_eval;
            let cut = search.probcut(&mut board, 6, 0, beta - 1, beta, parent);
            assert!(cut.is_some(), "the capture did not cut");
            assert_eq!(search.stats.probcut_searches.get(), 1, "under {parent:?}");
            let read: Vec<_> = at(&search, 1).iter().map(|c| (c.asked, c.kind)).collect();
            assert_eq!(
                read,
                [(Some((parent, Via::Null)), child)],
                "under {parent:?}"
            );
        }
    }

    /// Only a fail low under a flagged parent adds the flag; a node's own flag is stored whatever
    /// its bound, and a node with no parent reads none.
    #[test]
    fn a_node_failing_low_takes_its_parents_flag() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = Search::new(&stop, &tt);
        let cases = [
            (2, true, false, Bound::Upper, true),
            (2, true, false, Bound::Lower, false),
            (2, true, false, Bound::Exact, false),
            (2, false, false, Bound::Upper, false),
            (2, false, true, Bound::Lower, true),
            (0, true, false, Bound::Upper, false),
        ];
        for (i, &(ply, parent, own, bound, stored)) in cases.iter().enumerate() {
            let key = 0x9E37_79B9_7F4A_7C15_u64.wrapping_mul(i as u64 + 1);
            search.tt_pv = [parent; MAX_PLY];
            search.tt_pv[ply] = own;
            search.store(key, ply, 5, 0, Move::NULL, bound);
            let hit = tt.probe(key).expect("the entry was stored");
            assert_eq!(
                hit.pv, stored,
                "ply {ply}, parent {parent}, own {own}, {bound:?}"
            );
        }
    }

    /// Deep enough, and beta far enough above the child's evaluation, that the nodes below the null
    /// move search quiet moves to a cutoff and so write the rows they read.
    const DEPTH: u32 = 9;
    const BETA: Score = 300;

    /// A null move at ply 1 over the frame an earlier node left there, its static evaluation planted
    /// above beta: the child reads no move one ply up and the root's move two plies up.
    #[test]
    fn a_null_move_hides_the_frame_below_it_and_not_the_one_above() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = Search::new(&stop, &tt);
        let root = Board::from_fen(MIDDLEGAME).expect("the middlegame");
        let m = parse_uci(&generate_legal(&root), "h2h3").expect("a legal move");
        let mut board = Position::new(root);
        search.root_depth = DEPTH;
        search.make(&mut board, 0, m);
        let root_frame = search.heuristics.frames[0].expect("the root move's frame");
        // Black's king onto a1, which no move below the null move can be.
        let stale = 11 * 64;
        search.heuristics.frames[1] = Some(stale);
        search.heuristics.evals[1] = Some(BETA + 200);
        let _ = search.null_move(&mut board, DEPTH, 1, BETA - 1, BETA);

        // Nodes below it try their own, but only after this one was admitted.
        assert!(
            search.stats.null_attempts.get() > 0,
            "the null move was refused"
        );
        assert_eq!(
            conthist::context(&search.heuristics.frames, 2, 1),
            None,
            "one ply up"
        );
        assert_eq!(
            conthist::context(&search.heuristics.frames, 2, 2),
            Some(root_frame),
            "two plies up"
        );
        let written = |context| {
            search
                .heuristics
                .conthist
                .row(context)
                .iter()
                .any(|&v| v != 0)
        };
        assert!(
            !written(stale),
            "a node below the null move read its ply's old frame"
        );
        assert!(
            written(root_frame),
            "no node below the null move read the root's move"
        );
    }

    /// A queen loose against a castled king, with checks to give once white has moved.
    const CHECKS: &str = "6k1/8/8/8/3q4/8/5PPP/6K1 w - - 0 1";

    /// The checking children of a node at ply one under a planted count, at a root depth of four.
    /// Under a high window every move is searched, and a check is neither reduced nor skipped.
    fn checking_children(taken: u32) -> Vec<Call> {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        search.tunables = DEFAULT;
        let (mut board, m) = position_and_move(CHECKS, "h2h3");
        search.root_depth = 4;
        search.make(&mut board, 0, m);
        let checks: Vec<Move> = generate_legal(&board)
            .iter()
            .filter(|&c| board.gives_check(c))
            .collect();
        assert!(checks.len() >= 3, "too few checks: {checks:?}");
        search.extensions[1] = taken;
        let _ = search.negamax(&mut board, 3, 1, HIGH, HIGH + 1, Expected::Cut);
        let children: Vec<Call> = at(&search, 2)
            .into_iter()
            .filter(|c| checks.contains(&c.moved))
            .collect();
        assert_eq!(children.len(), checks.len(), "a check was not searched");
        children
    }

    /// One ply short of the cap a check is extended and its count reaches the root depth; at the
    /// cap it is searched one ply down and the count stays.
    #[test]
    fn a_line_at_its_cap_takes_no_check_extension() {
        for c in checking_children(3) {
            assert_eq!((c.depth, c.extensions), (3, 4), "below the cap: {c:?}");
        }
        for c in checking_children(4) {
            assert_eq!((c.depth, c.extensions), (2, 4), "at the cap: {c:?}");
        }
    }

    /// Over a stale count at its ply, the null move's child takes its parent's.
    #[test]
    fn the_null_moves_child_takes_its_parents_count() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let (mut board, m) = position_and_move(MIDDLEGAME, "h2h3");
        search.root_depth = DEPTH;
        search.make(&mut board, 0, m);
        search.extensions[1] = 3;
        search.extensions[2] = 7;
        search.heuristics.evals[1] = Some(BETA + 200);
        let _ = search.null_move(&mut board, DEPTH, 1, BETA - 1, BETA);
        assert!(
            search.stats.null_attempts.get() > 0,
            "the null move was refused"
        );
        let read: Vec<_> = at(&search, 2).iter().map(|c| c.extensions).collect();
        assert_eq!(read, [3]);
    }

    /// Over a stale count at its ply, the capture probe's child takes its parent's.
    #[test]
    fn probcuts_child_takes_its_parents_count() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let root = Board::from_fen(QUEEN_HANGS).expect("the hanging queen");
        let static_eval = eval::evaluate(&root);
        let mut board = Position::new(root);
        search.root_depth = 6;
        search.heuristics.evals[0] = Some(static_eval);
        search.extensions[0] = 2;
        search.extensions[1] = 7;
        let beta = static_eval;
        let cut = search.probcut(&mut board, 6, 0, beta - 1, beta, Expected::Cut);
        assert!(cut.is_some(), "the capture did not cut");
        let read: Vec<_> = at(&search, 1).iter().map(|c| c.extensions).collect();
        assert_eq!(read, [2]);
    }

    /// A count left at every ply by an earlier run: the gates' entry starts the line at none, so no
    /// call holds more than its ply, which is all the handcrafted search's uncapped count allows.
    #[test]
    fn the_gates_entry_starts_every_line_at_no_extension() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let mut board = Position::new(Board::from_fen(CHECKS).expect("the loose queen"));
        search.extensions = [7; MAX_PLY + 1];
        let _ = search.node(&mut board, 3, 0);
        let calls = search.calls();
        assert!(calls.len() > 1, "nothing searched below the entry");
        for c in calls {
            let ply = u32::try_from(c.ply).expect("a small ply");
            assert!(c.extensions <= ply, "{c:?}");
        }
    }

    /// A back-rank mate in one, a queen hanging to a pawn, the same giving check, and the queen
    /// hanging to two pawns.
    const BACK_RANK: &str = "6k1/5ppp/8/8/8/8/8/R5K1 w - - 0 1";
    const QUEEN_HANGS: &str = "4k3/8/8/3q4/4P3/8/8/4K3 w - - 0 1";
    const QUEEN_HANGS_CHECKING: &str = "8/8/2k5/3q4/4P3/8/8/4K3 w - - 0 1";
    const QUEEN_HANGS_TWICE: &str = "4k3/8/8/3q4/2P1P3/8/8/4K3 w - - 0 1";

    fn position_and_move(fen: &str, uci: &str) -> (Position, Move) {
        let root = Board::from_fen(fen).expect("a position");
        let m = parse_uci(&generate_legal(&root), uci).expect("a legal move");
        (Position::new(root), m)
    }

    /// The mate left out: no call below the verification makes it and its value is undecided,
    /// where the same node searched plainly at the verification's depth finds it.
    #[test]
    fn the_verification_never_searches_the_move_it_leaves_out() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let (mut board, mate) = position_and_move(BACK_RANK, "a1a8");
        search.root_depth = 9;
        let v = search.singular(&mut board, 9, 1, mate, 1_000, Expected::Cut);
        assert!(!score::is_decisive(v), "{v}");
        let below = at(&search, 2);
        assert!(below.len() > 3, "too few moves searched");
        assert!(
            below.iter().all(|c| c.moved != mate),
            "the mate was searched"
        );

        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let (mut board, _) = position_and_move(BACK_RANK, "a1a8");
        search.root_depth = 9;
        let at_depth = verification_depth(9);
        let v = search.negamax(&mut board, at_depth, 1, 999, 1_000, Expected::Cut);
        assert!(score::is_decisive(v), "the control found no mate: {v}");
        assert!(at(&search, 2).iter().any(|c| c.moved == mate));
    }

    /// An exact entry at the verification's depth, which would answer it at once: the verification
    /// neither reads it nor overwrites it.
    #[test]
    fn the_excluded_node_neither_probes_nor_stores() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let (mut board, mate) = position_and_move(BACK_RANK, "a1a8");
        let key = board.key();
        let depth = u8::try_from(verification_depth(9)).expect("a small depth");
        tt.store(key, mate, 7_777, depth, Bound::Exact, false);
        let planted = tt.probe(key).expect("the planted entry");
        search.root_depth = 9;
        let v = search.singular(&mut board, 9, 1, mate, 1_000, Expected::Cut);
        assert_ne!(v, 7_777, "the planted entry answered the verification");
        assert_eq!(
            tt.probe(key),
            Some(planted),
            "the verification overwrote the entry"
        );
    }

    type Counted = fn(&Search) -> u64;

    fn null_attempts(s: &Search) -> u64 {
        s.stats.null_attempts.get()
    }

    fn iir_nodes(s: &Search) -> u64 {
        s.stats.iir_nodes.get()
    }

    fn probcut_attempts(s: &Search) -> u64 {
        s.stats.probcut_attempts.get()
    }

    /// The middlegame after h2h3, black to move at ply one, searched with its own evaluation offset
    /// to make the window: as a verification at `depth`, or plainly at the verification's depth.
    fn verified_or_plain(depth: u32, offset: Score, verified: bool, read: Counted) -> u64 {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let (mut board, m) = position_and_move(MIDDLEGAME, "h2h3");
        search.root_depth = depth;
        search.make(&mut board, 0, m);
        let eval = eval::evaluate(&board);
        let s_beta = eval + offset;
        if verified {
            let left_out = generate_legal(&board).as_slice()[0];
            search.heuristics.evals[1] = Some(eval);
            let _ = search.singular(&mut board, depth, 1, left_out, s_beta, Expected::Cut);
        } else {
            let at = verification_depth(depth);
            let _ = search.negamax(&mut board, at, 1, s_beta - 1, s_beta, Expected::Cut);
        }
        read(&search)
    }

    /// At depths where no node below the excluded one can take them, its own null move, capture
    /// probe and iterative reduction are each counted at zero, where the plain node takes each.
    #[test]
    fn the_excluded_node_takes_no_null_move_probcut_or_iterative_reduction() {
        let cases: [(&str, u32, Score, Counted); 3] = [
            ("null move", 3, -50, null_attempts),
            ("iterative reduction", 9, 100, iir_nodes),
            ("capture probe", 11, 100, probcut_attempts),
        ];
        for (rule, depth, offset, read) in cases {
            assert!(
                verified_or_plain(depth, offset, false, read) > 0,
                "{rule}: the control"
            );
            assert_eq!(verified_or_plain(depth, offset, true, read), 0, "{rule}");
        }
    }

    /// An entry that would admit a node at the excluded node's depth: no verification runs inside
    /// the one called.
    #[test]
    fn the_excluded_node_asks_no_question_of_its_own() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        search.tunables = DEFAULT;
        let (mut board, m) = position_and_move(MIDDLEGAME, "h2h3");
        search.root_depth = 17;
        search.make(&mut board, 0, m);
        let eval = eval::evaluate(&board);
        let legal = generate_legal(&board);
        let (left_out, named) = (legal.as_slice()[0], legal.as_slice()[1]);
        let stored = i16::try_from(eval).expect("an evaluation");
        tt.store(board.key(), named, stored, 17, Bound::Lower, false);
        search.heuristics.evals[1] = Some(eval);
        let _ = search.singular(&mut board, 17, 1, left_out, eval + 100, Expected::Cut);
        assert_eq!(search.stats.singular_tests.get(), 1);
    }

    /// The flag and the evaluation the node wrote survive its verification, and its slot is empty
    /// after; the gates' entry clears an exclusion an earlier run left.
    #[test]
    fn the_nodes_state_survives_its_verification() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        let (mut board, mate) = position_and_move(BACK_RANK, "a1a8");
        search.root_depth = 9;
        search.tt_pv[1] = true;
        search.heuristics.evals[1] = Some(123);
        let _ = search.singular(&mut board, 9, 1, mate, 1_000, Expected::Cut);
        assert!(search.tt_pv[1], "the flag was clobbered");
        assert_eq!(search.heuristics.evals[1], Some(123));
        assert!(search.excluded.iter().all(|m| m.is_null()));

        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        search.excluded = [mate; MAX_PLY];
        let _ = search.node(&mut board, 2, 0);
        assert!(
            at(&search, 1).iter().any(|c| c.moved == mate),
            "a planted exclusion hid the move"
        );
    }

    /// A check or a singular table move takes one ply and the two together take one; at the cap
    /// neither takes any, and only a singular move without a check is counted.
    #[test]
    fn a_singular_move_takes_one_ply_however_it_is_granted() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = Search::new(&stop, &tt);
        search.tunables = DEFAULT;
        search.root_depth = 8;
        for (check, singular, want) in [
            (false, false, 0),
            (true, false, 1),
            (false, true, 1),
            (true, true, 1),
        ] {
            search.extensions[3] = 2;
            assert_eq!(
                search.extend(check, singular, 3),
                want,
                "{check} {singular}"
            );
            assert_eq!(search.extensions[4], 2 + want.unsigned_abs());
            search.extensions[3] = 8;
            assert_eq!(
                search.extend(check, singular, 3),
                0,
                "at the cap: {check} {singular}"
            );
        }
        assert_eq!(search.stats.singular_extensions.get(), 1);
    }

    /// Under the handcrafted set an admitted entry is asked nothing and a check at the cap is still
    /// extended, as `0.6.0` searched.
    #[test]
    fn the_handcrafted_set_neither_asks_nor_caps() {
        let lower = (Bound::Lower, 500);
        let found = below_a_table_move(QUEEN_HANGS, lower, 11, Tunables::HCE, HIGH_WINDOW);
        assert_eq!(found.tests, 0);
        assert_eq!(depths_of(&found.calls, found.table_move), [10]);
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = Search::new(&stop, &tt);
        search.root_depth = 8;
        search.extensions[3] = 8;
        assert_eq!(search.extend(true, false, 3), 1);
    }

    /// The set a net's search reads, with the shallowest depth asked moved to `depth`.
    fn asked_from(depth: i32) -> Tunables {
        let mut tunables = DEFAULT;
        let param = &PARAMS[Tunable::SingularDepth as usize];
        param.set(&mut tunables, depth);
        tunables
    }

    /// What a node at ply one showed with the capture e4d5 as its table's move: the calls one ply
    /// below it, the move, the verifications and cuts, its value, and whether its entry changed.
    struct Below {
        calls: Vec<Call>,
        table_move: Move,
        tests: u64,
        value: Score,
        multicuts: u64,
        stored: bool,
    }

    /// `tunables` decides the depth asked from, and `window` the node's.
    fn below_a_table_move(
        fen: &str,
        (bound, stored_score): (Bound, i16),
        depth: u32,
        tunables: Tunables,
        window: (Score, Score),
    ) -> Below {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        search.tunables = tunables;
        let (mut board, capture) = position_and_move(fen, "e4d5");
        let key = board.key();
        let stored = u8::try_from(depth - 1).expect("a small depth");
        tt.store(key, capture, stored_score, stored, bound, false);
        let planted = tt.probe(key);
        search.root_depth = depth;
        let kind = if window.1 > window.0 + 1 {
            Expected::Pv
        } else {
            Expected::Cut
        };
        let value = search.negamax(&mut board, depth, 1, window.0, window.1, kind);
        Below {
            calls: at(&search, 2),
            table_move: capture,
            tests: search.stats.singular_tests.get(),
            value,
            multicuts: search.stats.multicut_cutoffs.get(),
            stored: tt.probe(key) != planted,
        }
    }

    fn depths_of(calls: &[Call], m: Move) -> Vec<u32> {
        calls
            .iter()
            .filter(|c| c.moved == m && c.verifying == 0)
            .map(|c| c.depth)
            .collect()
    }

    /// Admitted, the capture is the one move that holds, and its child keeps the node's depth
    /// while no other move is extended; an upper bound admits nothing and the capture is searched a
    /// ply down. The verification is emptied by futility at depth eight and searches at eleven.
    #[test]
    fn a_single_holding_move_is_searched_a_ply_deeper() {
        for depth in [8, 11] {
            let lower = (Bound::Lower, 500);
            let found = below_a_table_move(QUEEN_HANGS, lower, depth, DEFAULT, HIGH_WINDOW);
            assert_eq!(found.tests, 1, "depth {depth}");
            assert_eq!(depths_of(&found.calls, found.table_move), [depth]);
            let outer: Vec<&Call> = found
                .calls
                .iter()
                .filter(|c| c.verifying == 0 && c.moved != found.table_move)
                .collect();
            assert!(outer.len() > 3, "depth {depth}: too few other moves");
            assert!(outer.iter().all(|c| c.depth < depth), "{outer:?}");
            let verified = found.calls.iter().filter(|c| c.verifying == 1).count();
            assert_eq!(
                verified > 0,
                depth == 11,
                "depth {depth}: {verified} moves verified"
            );

            let upper = (Bound::Upper, 500);
            let found = below_a_table_move(QUEEN_HANGS, upper, depth, DEFAULT, HIGH_WINDOW);
            assert_eq!(found.tests, 0);
            assert_eq!(depths_of(&found.calls, found.table_move), [depth - 1]);
        }
    }

    /// Far above the node's value, so no move cuts and every one is searched.
    const HIGH_WINDOW: (Score, Score) = (HIGH, HIGH + 1);

    /// The capture gives check as well as being singular, and takes one ply, not two.
    #[test]
    fn a_singular_table_move_that_checks_is_extended_once() {
        let lower = (Bound::Lower, 500);
        let found = below_a_table_move(QUEEN_HANGS_CHECKING, lower, 8, DEFAULT, HIGH_WINDOW);
        assert_eq!(found.tests, 1);
        assert_eq!(depths_of(&found.calls, found.table_move), [8]);
    }

    /// Either pawn takes the queen, so the search without the table's capture still holds its
    /// score: asked, the capture is not singular and is searched a ply down.
    #[test]
    fn two_moves_that_hold_extend_neither() {
        let lower = (Bound::Lower, 100);
        let found = below_a_table_move(QUEEN_HANGS_TWICE, lower, 4, asked_from(4), HIGH_WINDOW);
        assert_eq!((found.tests, found.multicuts), (1, 0));
        assert_eq!(depths_of(&found.calls, found.table_move), [3]);
    }

    /// The same, under a beta at the singular beta: the other capture beats it, so two moves do and
    /// the node returns the singular beta, searching nothing outside the verification and storing
    /// nothing. Shallower than the capture probe, which would cut first.
    #[test]
    fn two_moves_beating_beta_return_the_singular_beta_unstored() {
        let s_beta = singular_beta(100, 4, SINGULAR_MARGIN);
        let lower = (Bound::Lower, 100);
        let window = (s_beta - 1, s_beta);
        let found = below_a_table_move(QUEEN_HANGS_TWICE, lower, 4, asked_from(4), window);
        assert_eq!((found.tests, found.multicuts), (1, 1));
        assert_eq!(found.value, s_beta);
        assert!(
            found.calls.iter().all(|c| c.verifying == 1),
            "a move was searched outside the verification: {:?}",
            found.calls
        );
        assert!(
            found.calls.iter().all(|c| c.moved != found.table_move),
            "the table's move was searched"
        );
        assert!(!found.stored, "the node stored over the entry");
    }

    /// A stop at the verification's first move, deep enough that it searches one: the node returns
    /// the aborted value, stores nothing over the entry that admitted it, and leaves no exclusion.
    #[test]
    fn a_stop_inside_the_verification_aborts_the_node() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        search.tunables = DEFAULT;
        let (mut board, capture) = position_and_move(QUEEN_HANGS, "e4d5");
        let key = board.key();
        tt.store(key, capture, 500, 8, Bound::Lower, false);
        let planted = tt.probe(key).expect("the planted entry");
        search.set_limits(Limits {
            nodes: Some(3),
            ..Limits::default()
        });
        search.root_depth = 9;
        let v = search.negamax(&mut board, 9, 1, HIGH, HIGH + 1, Expected::Cut);
        assert_eq!(
            (v, search.run.aborted, search.stats.singular_tests.get()),
            (ABORTED, true, 1)
        );
        let last = search.calls().last().expect("a call");
        assert_eq!(
            (last.ply, last.verifying),
            (2, 1),
            "the stop fell outside it"
        );
        assert!(search.excluded.iter().all(|m| m.is_null()));
        assert_eq!(tt.probe(key), Some(planted));

        // Asked alone across the same stop, the verdict is the stop and never singular.
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = searcher(&stop, &tt);
        search.tunables = DEFAULT;
        tt.store(key, capture, 500, 8, Bound::Lower, false);
        search.set_limits(Limits {
            nodes: Some(2),
            ..Limits::default()
        });
        search.root_depth = 9;
        let legal = generate_legal(&board);
        let hit = tt.probe(key);
        let verdict = search.singular_verdict(&mut board, &legal, hit, 9, 1, Expected::Cut, HIGH);
        assert!(search.run.aborted, "the stop was not reached");
        assert_eq!(
            verdict,
            Err(ABORTED),
            "an aborted verification read as a verdict"
        );
    }
}
