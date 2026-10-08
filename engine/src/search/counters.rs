// SPDX-License-Identifier: GPL-3.0-or-later

//! Nothing here calls into another part of the search, which is what lets every other part call it.

use std::sync::atomic::Ordering;

use cadence_core::position::Board;
use cadence_core::{Colour, Move};

use super::{Limits, NODE_PUBLISH_INTERVAL, Search};
use crate::eval;
use crate::history::{self, History};
use crate::score::{self, Score};
use crate::tt::Bound;

impl Search<'_> {
    #[must_use]
    pub fn nodes(&self) -> u64 {
        self.nodes
    }

    #[must_use]
    pub fn null_attempts(&self) -> u64 {
        self.null_attempts
    }

    #[must_use]
    pub fn null_cutoffs(&self) -> u64 {
        self.null_cutoffs
    }

    #[must_use]
    pub fn null_refused_by_material(&self) -> u64 {
        self.null_refused_material
    }

    /// Late moves first searched at reduced depth.
    #[must_use]
    pub fn lmr_reductions(&self) -> u64 {
        self.lmr_reductions
    }

    /// Reduced searches that beat alpha and were re-run at full depth.
    #[must_use]
    pub fn lmr_researches(&self) -> u64 {
        self.lmr_researches
    }

    #[must_use]
    pub fn history_reduced_less(&self) -> u64 {
        self.history_reduced_less
    }

    #[must_use]
    pub fn history_reduced_more(&self) -> u64 {
        self.history_reduced_more
    }

    /// Nodes the margin admitted; its siblings count the quiet moves skipped there and the skips
    /// refused for giving check.
    #[must_use]
    pub fn futility_nodes(&self) -> u64 {
        self.futility_nodes
    }

    #[must_use]
    pub fn futility_skipped(&self) -> u64 {
        self.futility_skipped
    }

    #[must_use]
    pub fn futility_kept_check(&self) -> u64 {
        self.futility_kept_check
    }

    #[must_use]
    pub fn lmp_nodes(&self) -> u64 {
        self.lmp_nodes
    }

    /// Observations folded into the correction table, and nodes that read a non-zero correction
    /// back.
    #[must_use]
    pub fn corrhist_updates(&self) -> u64 {
        self.corrhist_updates
    }

    #[must_use]
    pub fn corrhist_applied(&self) -> u64 {
        self.corrhist_applied
    }

    #[must_use]
    pub fn lmp_skipped(&self) -> u64 {
        self.lmp_skipped
    }

    #[must_use]
    pub fn lmp_kept_check(&self) -> u64 {
        self.lmp_kept_check
    }

    #[must_use]
    pub fn reverse_futility_cutoffs(&self) -> u64 {
        self.reverse_futility_cutoffs
    }

    /// Nodes the margin would have returned but for the full window.
    #[must_use]
    pub fn reverse_futility_refused_by_window(&self) -> u64 {
        self.reverse_futility_refused_window
    }

    /// Nodes that ran the capture probe, captures it searched at reduced depth, and those that cut
    /// the node.
    #[must_use]
    pub fn probcut_attempts(&self) -> u64 {
        self.probcut_attempts
    }

    #[must_use]
    pub fn probcut_searches(&self) -> u64 {
        self.probcut_searches
    }

    #[must_use]
    pub fn probcut_cutoffs(&self) -> u64 {
        self.probcut_cutoffs
    }

    /// Probes refused for the full window.
    #[must_use]
    pub fn probcut_refused_by_window(&self) -> u64 {
        self.probcut_refused_window
    }

    /// Evasion lists the quiescence search prepared, and those whose head the sort changed.
    #[must_use]
    pub fn evasion_lists(&self) -> u64 {
        self.evasion_lists
    }

    #[must_use]
    pub fn evasion_lists_reordered(&self) -> u64 {
        self.evasion_lists_reordered
    }

    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Elapsed milliseconds at the end of each completed iteration.
    #[must_use]
    pub fn iterations_ms(&self) -> &[u64] {
        &self.iterations
    }

    #[must_use]
    pub fn iteration_roots(&self) -> &[(Move, Score)] {
        &self.roots
    }

    /// Completed iterations in a row ending on the last one's move, counting it. Derived from
    /// [`Search::iteration_roots`], so the two cannot disagree.
    #[must_use]
    pub fn stable_iterations(&self) -> usize {
        let Some(&(last, _)) = self.roots.last() else {
            return 0;
        };
        self.roots
            .iter()
            .rev()
            .take_while(|&&(m, _)| m == last)
            .count()
    }

    /// Zero before any iteration completes.
    #[must_use]
    pub fn completed_depth(&self) -> u32 {
        self.completed_depth
    }

    /// From the side to move's point of view.
    #[must_use]
    pub fn score(&self) -> Score {
        self.score
    }

    #[must_use]
    pub fn pv(&self) -> &[Move] {
        &self.pv
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    #[must_use]
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    #[inline]
    pub(super) fn visit(&mut self, ply: usize) {
        self.nodes += 1;
        self.seldepth = self.seldepth.max(ply);
        if self.nodes & (NODE_PUBLISH_INTERVAL - 1) == 0 {
            self.publish_nodes();
        }
    }

    /// Read before the node folds anything in, so no correction has seen the score it will be
    /// scored against.
    pub(super) fn corrected_eval(
        &mut self,
        board: &Board,
        in_check: bool,
        pawn_key: u64,
        side: Colour,
    ) -> Option<Score> {
        if in_check {
            crate::corrhist_shadow::read(self.shadow_ply, pawn_key, side, None, 0);
            return None;
        }
        let correction = self.corrhist.correction(pawn_key, side);
        self.corrhist_applied += u64::from(correction != 0);
        let raw = eval::evaluate(board);
        crate::corrhist_shadow::read(self.shadow_ply, pawn_key, side, Some(raw), correction);
        Some(raw + correction)
    }

    /// Disqualified: no static reading, a mate score, a noisy or absent best move, or a bound
    /// pointing against the difference.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn remember_correction(
        &mut self,
        pawn_key: u64,
        side: Colour,
        ply: usize,
        best: Score,
        best_move: Move,
        bound: Bound,
        depth: u32,
    ) {
        crate::corrhist_shadow::observe(ply, pawn_key, side, best, best_move, bound, depth);
        let Some(eval) = self.evals[ply] else {
            return;
        };
        if score::is_mate(best) || best_move == Move::NULL || best_move.is_noisy() {
            return;
        }
        let usable = match bound {
            Bound::Exact => true,
            Bound::Lower => best > eval,
            Bound::Upper => best < eval,
        };
        if usable {
            self.corrhist.update(pawn_key, side, best - eval, depth);
            self.corrhist_updates += 1;
        }
    }

    /// Credits `cut` and debits every quiet move tried before it.
    pub(super) fn remember_history(&mut self, us: Colour, tried: &[Move], cut: Move, depth: u32) {
        if cut.is_noisy() {
            return;
        }
        let bonus = history::bonus(depth);
        self.history.update(us, cut, bonus);
        for &beaten in tried.iter().filter(|q| !q.is_noisy()) {
            self.history.update(us, beaten, -bonus);
        }
    }

    pub(super) fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// The group's where one is watching: the siblings' counts from their slots, this worker's
    /// live.
    #[inline]
    pub(super) fn reported_nodes(&self) -> u64 {
        let Some((nodes, worker_index)) = self.shared_nodes else {
            return self.nodes;
        };
        nodes.iter().enumerate().fold(0, |total, (index, nodes)| {
            total.saturating_add(if index == worker_index {
                self.nodes
            } else {
                nodes.load(Ordering::Relaxed)
            })
        })
    }

    /// Only the owner writes the slot, so `Relaxed` suffices. Shared cache lines measured under 1
    /// percent of throughput up to 18 threads, about 3 percent at 64.
    #[inline]
    pub(super) fn publish_nodes(&self) {
        if let Some((nodes, worker_index)) = self.shared_nodes {
            nodes[worker_index].store(self.nodes, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use crate::search::Search;
    use crate::tt::Table;

    /// The one externally visible thing `Threads` above one changes; asserted directly because a
    /// search shows it only when helpers get scheduled.
    #[test]
    fn reported_nodes_sums_the_group_and_reads_its_own_count_live() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let slots: Vec<AtomicU64> = (0..4).map(|_| AtomicU64::new(0)).collect();
        slots[1].store(100, Ordering::Relaxed);
        slots[2].store(20, Ordering::Relaxed);
        slots[3].store(3, Ordering::Relaxed);

        let mut search = Search::new(&stop, &tt);
        search.nodes = 7;
        assert_eq!(
            search.reported_nodes(),
            7,
            "a search outside a group answers for itself"
        );

        search.set_parallel(0, &slots);
        assert_eq!(
            search.reported_nodes(),
            130,
            "own count live plus every sibling's slot"
        );

        // Stale until it publishes, which is why the live count comes from the field.
        assert_eq!(slots[0].load(Ordering::Relaxed), 0);
        search.publish_nodes();
        assert_eq!(slots[0].load(Ordering::Relaxed), 7);
        assert_eq!(search.reported_nodes(), 130, "publishing changes no total");
    }
}
