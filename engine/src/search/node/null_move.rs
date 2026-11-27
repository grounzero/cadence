// SPDX-License-Identifier: GPL-3.0-or-later

//! The null move: the turn is passed and the position searched reduced, at a null window only.

#[cfg(debug_assertions)]
use cadence_core::Move;

use crate::position::Position;
use crate::score::{self, ABORTED, Score};
use crate::search::depth::null_reduction;
use crate::search::pruning::has_non_pawn_material;
use crate::search::{Expected, Search};

impl<S: crate::search::Settings> Search<'_, S> {
    pub(super) fn null_move(
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
}
