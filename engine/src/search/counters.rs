// SPDX-License-Identifier: GPL-3.0-or-later

//! What a search counts and remembers as it runs, and what it tells you about itself afterwards.
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
    /// Nodes searched so far.
    #[must_use]
    pub fn nodes(&self) -> u64 {
        self.nodes
    }

    /// How many null moves the last search tried.
    #[must_use]
    pub fn null_attempts(&self) -> u64 {
        self.null_attempts
    }

    /// How many of those produced a cutoff.
    #[must_use]
    pub fn null_cutoffs(&self) -> u64 {
        self.null_cutoffs
    }

    /// How often every other condition admitted a null move and the side to move had nothing
    /// but pawns beside the king.
    #[must_use]
    pub fn null_refused_by_material(&self) -> u64 {
        self.null_refused_material
    }

    /// How many late moves the last search first searched at reduced depth.
    #[must_use]
    pub fn lmr_reductions(&self) -> u64 {
        self.lmr_reductions
    }

    /// How many of those reduced searches beat alpha and were re-run at full depth.
    #[must_use]
    pub fn lmr_researches(&self) -> u64 {
        self.lmr_researches
    }

    /// How often a history score shortened a reduction the index had decided on, and how often
    /// it lengthened one.
    #[must_use]
    pub fn history_reduced_less(&self) -> u64 {
        self.history_reduced_less
    }

    #[must_use]
    pub fn history_reduced_more(&self) -> u64 {
        self.history_reduced_more
    }

    /// How many nodes the margin admitted, how many quiet moves it skipped there, and how many
    /// it would have skipped and did not because the move gives check.
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

    /// How many nodes the margin returned without searching, and how many it would have
    /// returned and did not because the node had the full window. How often a node admitted
    /// this rule, how many quiet moves it gave up there, and how often a move that would have
    /// been given up was kept for giving check.
    #[must_use]
    pub fn lmp_nodes(&self) -> u64 {
        self.lmp_nodes
    }

    /// How many observations this search folded into the correction table,
    /// and at how many nodes it read a non-zero correction back.
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

    #[must_use]
    pub fn reverse_futility_refused_by_window(&self) -> u64 {
        self.reverse_futility_refused_window
    }

    /// How many nodes ran the capture probe, how many captures it searched at reduced depth, and
    /// how many of those cut the node.
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

    /// How often the probe would have run and did not because the node had the full window.
    #[must_use]
    pub fn probcut_refused_by_window(&self) -> u64 {
        self.probcut_refused_window
    }

    /// How many check evasion lists the quiescence search prepared, and how many of those the
    /// sort moved a new move to the head of. The first says the in-check horizon was reached at
    /// all, which is what stops the second being vacuous.
    #[must_use]
    pub fn evasion_lists(&self) -> u64 {
        self.evasion_lists
    }

    #[must_use]
    pub fn evasion_lists_reordered(&self) -> u64 {
        self.evasion_lists_reordered
    }

    /// The table the last search left behind, for a gate that wants to see what the cutoffs
    /// wrote and what the ordering would do with it.
    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The depth of the last completed iteration; zero before any. Elapsed milliseconds at the
    /// end of each completed iteration, in order.
    #[must_use]
    pub fn iterations_ms(&self) -> &[u64] {
        &self.iterations
    }

    /// The root move and score of each completed iteration, in order, and empty where none
    /// completed. Kept under every limit, so a `go depth` and a `bench` position record one
    /// entry per iteration while reading no clock.
    #[must_use]
    pub fn iteration_roots(&self) -> &[(Move, Score)] {
        &self.roots
    }

    /// How many completed iterations in a row ended on the move the last one ended on, counting
    /// that one, and zero where none completed. Derived from [`Search::iteration_roots`] rather
    /// than counted beside it, so the two cannot disagree.
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

    #[must_use]
    pub fn completed_depth(&self) -> u32 {
        self.completed_depth
    }

    /// The root score of the last completed iteration, from the side to move's point of view.
    #[must_use]
    pub fn score(&self) -> Score {
        self.score
    }

    /// The principal variation of the last completed iteration.
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

    /// Count a node, at the ply it sits at, and publish the count where a group is watching.
    /// The deepest ply is what `seldepth` reports and nothing here reads it, so a search that
    /// keeps it visits the same nodes in the same order as one that does not.
    #[inline]
    pub(super) fn visit(&mut self, ply: usize) {
        self.nodes += 1;
        self.seldepth = self.seldepth.max(ply);
        if self.nodes & (NODE_PUBLISH_INTERVAL - 1) == 0 {
            self.publish_nodes();
        }
    }

    /// The static evaluation this node's rules read, corrected by what the evaluation has been
    /// wrong by on this pawn structure. The correction is read before the node folds anything
    /// in, so nothing it offers has seen the score it will be scored against.
    pub(super) fn corrected_eval(
        &mut self,
        board: &Board,
        in_check: bool,
        pawn_key: u64,
        side: Colour,
    ) -> Option<Score> {
        if in_check {
            return None;
        }
        let correction = self.corrhist.correction(pawn_key, side);
        self.corrhist_applied += u64::from(correction != 0);
        Some(eval::evaluate(board) + correction)
    }

    /// Fold this node's disagreement between the static evaluation and the score it returned
    /// into the correction table. Four things disqualify a node: no static reading, a mate
    /// score, a best move that is noisy or absent, and a bound pointing the other way from the
    /// difference.
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

    /// Record what this node's cutoff says about its quiet moves: credit `cut`, and debit every
    /// quiet move tried ahead of it at this node.
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

    /// This search's nodes, or the group's where one is watching. A worker reads its own count
    /// live and its siblings' from the slots they publish into, so a reported figure is about
    /// the whole search rather than about one thread.
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

    /// Store this worker's count in the slot it owns. Nothing else writes that slot, so no
    /// ordering beyond `Relaxed` is needed; the slots do share cache lines, which was measured
    /// at under 1 percent of node throughput up to 18 threads and about 3 percent at 64.
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

    /// `reported_nodes` answers for the group and not for the worker that asks. This is the one
    /// externally visible thing `Threads` above one changes, and it is asserted here rather than
    /// through a search because a search only shows it when the helpers get scheduled.
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

        // The worker's own slot is stale until it publishes, which is why the live count is read
        // from the field and never from the slot.
        assert_eq!(slots[0].load(Ordering::Relaxed), 0);
        search.publish_nodes();
        assert_eq!(slots[0].load(Ordering::Relaxed), 7);
        assert_eq!(search.reported_nodes(), 130, "publishing changes no total");
    }
}
