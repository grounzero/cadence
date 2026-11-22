// SPDX-License-Identifier: GPL-3.0-or-later

//! A later move's null-window search, reduced first and re-run at full depth when it beats alpha.

use crate::position::Position;
use crate::score::Score;
use crate::search::depth::{cut_node_reduction, history_reduction, pv_reduction, reduced_depth};
use crate::search::{Expected, Search, Via};

impl Search<'_> {
    /// A reduced search that beats alpha is re-run at full depth before it is believed.
    #[allow(
        clippy::too_many_arguments,
        reason = "passing these separately measured +0.21% faster than a shared node type (pull request 233 timing, 2026-11-08)"
    )]
    pub(super) fn late_move(
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
}
