// SPDX-License-Identifier: GPL-3.0-or-later

//! Every node's table and tablebase reads, made before any move is generated, and the one store.

use cadence_core::Move;
use cadence_core::position::Board;

use crate::score::{self, DRAW, INFINITE, Score, mated_in};
use crate::search::Search;
use crate::syzygy::Wdl;
use crate::tt::Bound;

/// The key does not carry the fifty-move clock, so from here a cutoff could rest on a line the draw
/// has since cut short; only the entry's move is kept.
const CUTOFF_CLOCK: u8 = 90;

impl<const SOFT_STAND_PAT: bool> Search<'_, SOFT_STAND_PAT> {
    /// The move comes back whatever the depth says, so the two halves return separately, and the
    /// hit's flag and static evaluation beside them.
    fn probe(
        &self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> (Move, Option<Score>, bool, Option<Score>) {
        let Some(hit) = self.tt.probe(key) else {
            return (Move::NULL, None, false, None);
        };
        let eval = hit.eval.map(Score::from);
        // A result whose depth reaches the draw may rest on a line the clock now cuts short.
        let clock = board.halfmove_clock();
        if u32::from(hit.depth) < depth
            || clock >= CUTOFF_CLOCK
            || u32::from(clock) + u32::from(hit.depth) >= 100
        {
            return (hit.mv, None, hit.pv, eval);
        }
        let score = score::from_tt(hit.score, ply);
        let cutoff = match hit.bound {
            Bound::Exact => true,
            Bound::Lower => score >= beta,
            Bound::Upper => score <= alpha,
        };
        (hit.mv, cutoff.then_some(score), hit.pv, eval)
    }
}

impl<const SOFT_STAND_PAT: bool> Search<'_, SOFT_STAND_PAT> {
    /// The one store, and where a node failing low takes its parent's flag; `eval` is the node's
    /// static evaluation before any correction, where it read one.
    #[allow(
        clippy::too_many_arguments,
        reason = "the node's own values, passed as the store before it took them"
    )]
    pub(super) fn store(
        &mut self,
        key: u64,
        ply: usize,
        depth: u32,
        best: Score,
        mv: Move,
        bound: Bound,
        eval: Option<Score>,
    ) {
        // An aborted node's value is `ABORTED` and not a bound, and the draw score besides.
        debug_assert!(!self.run.aborted, "a store after the stop");
        let stored_depth = depth.min(u32::from(u8::MAX)) as u8;
        let score = score::to_tt(best, ply);
        // Nothing here beat alpha, so the move into it may have been the good one.
        let pv = self.tt_pv[ply] || (bound == Bound::Upper && ply > 0 && self.tt_pv[ply - 1]);
        self.tt
            .store_with_eval(key, mv, score, stored_depth, bound, pv, eval);
    }
}

/// What the table and tablebase reads say about a node before its moves are generated.
pub(super) enum Lookup {
    /// The node's value, which the search returns at once.
    Cutoff(Score),
    /// What the node's search starts from.
    Search(Found),
}

/// The table's move to try first, the floor and ceiling the node keeps, and the static evaluation
/// the table holds for it.
pub(super) struct Found {
    pub(super) tt_move: Move,
    pub(super) bounds: (Score, Score),
    pub(super) eval: Option<Score>,
}

impl<const SOFT_STAND_PAT: bool> Search<'_, SOFT_STAND_PAT> {
    /// A move that repeats and the mate bounds, which narrow the window in place, then the table
    /// and the tablebases. The node's value when one of them answers it, and otherwise what its
    /// search starts from.
    pub(super) fn lookup(
        &mut self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: &mut Score,
        beta: &mut Score,
    ) -> Lookup {
        // The node is worth at least the draw, and takes no cutoff from a score stored under an
        // earlier history.
        let repeats = ply > 0 && *alpha < DRAW && board.has_upcoming_repetition();
        if repeats {
            if DRAW >= *beta {
                return Lookup::Cutoff(DRAW);
            }
            // Not where an open window would be left one point wide, since a node's kind reads it.
            if *beta > DRAW + 1 {
                *alpha = DRAW;
            }
        }
        (*alpha, *beta) = match mate_window(ply, *alpha, *beta) {
            Ok(window) => window,
            Err(score) => return Lookup::Cutoff(score),
        };
        let (alpha, beta) = (*alpha, *beta);
        let (tt_move, cutoff, hit_pv, eval) = self.probe(board, key, depth, ply, alpha, beta);
        if let Some(score) = cutoff.filter(|_| !repeats) {
            return Lookup::Cutoff(score);
        }
        // Before any child is searched, since a child that fails low reads it, and before the
        // tablebases, whose stored entry reads it.
        self.tt_pv[ply] = beta > alpha + 1 || hit_pv;
        // After the table, so a hit saves the probe, and before the static readings, which an
        // ending the tables decide makes moot.
        match self.tablebase(board, key, depth, ply, alpha, beta, tt_move) {
            Ok(bounds) => Lookup::Search(Found {
                tt_move,
                bounds,
                eval,
            }),
            Err(score) => Lookup::Cutoff(score),
        }
    }
}

impl<const SOFT_STAND_PAT: bool> Search<'_, SOFT_STAND_PAT> {
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
            self.store(key, ply, depth, score, tt_move, bound, None);
            return Err(score);
        }
        // A mate inside the table's territory is still searched for, and found, under the bound.
        Ok(match bound {
            _ if beta == alpha + 1 => open,
            Bound::Lower => (score, INFINITE),
            _ => (-INFINITE, score),
        })
    }
}

/// No score at `ply` beats mating at the next ply or falls below being mated there, so `Err` is a
/// bound that already answers the window, and otherwise the window is narrowed to that range.
pub(super) fn mate_window(ply: usize, alpha: Score, beta: Score) -> Result<(Score, Score), Score> {
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
