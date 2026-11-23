// SPDX-License-Identifier: GPL-3.0-or-later

//! A capture whose reduced search beats a raised beta cuts the node before its moves are generated.

use cadence_core::generate_noisy;

use crate::picker;
use crate::position::Position;
use crate::score::{self, ABORTED, Score};
use crate::search::pruning::{PROBCUT_REDUCTION, probcut_bound};
use crate::search::{Expected, Search, Via};
use crate::see;

impl Search<'_> {
    /// Each capture is screened by the quiescence search before the reduced search; a cutoff is
    /// never on the decided scale.
    pub(super) fn probcut(
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
