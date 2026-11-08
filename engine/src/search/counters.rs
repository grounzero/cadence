// SPDX-License-Identifier: GPL-3.0-or-later

//! Nothing here calls into another part of the search, which is what lets every other part call it.

use std::sync::atomic::Ordering;

use cadence_core::position::Board;
use cadence_core::{Colour, Move};

use super::node::Searched;
use super::{Limits, NODE_PUBLISH_INTERVAL, Search};
use crate::conthist::{self, ContinuationHistory, OFFSETS};
use crate::eval;
use crate::history::{self, History};
use crate::score::{self, Score};
use crate::tt::Bound;

/// A count only tests read, kept by the `counters` feature and compiled to nothing without it.
/// Nothing on a decision path reads one, so a search is the same with the feature as without.
#[derive(Clone, Copy)]
pub(super) struct Counter {
    #[cfg(feature = "counters")]
    n: u64,
}

impl Counter {
    pub(super) const ZERO: Counter = Counter {
        #[cfg(feature = "counters")]
        n: 0,
    };

    #[inline(always)]
    pub(super) fn add_one(&mut self) {
        self.add(1);
    }

    #[inline(always)]
    pub(super) fn add_if(&mut self, hit: bool) {
        self.add(u64::from(hit));
    }

    #[cfg(feature = "counters")]
    #[inline(always)]
    pub(super) fn add(&mut self, by: u64) {
        self.n += by;
    }

    #[cfg(not(feature = "counters"))]
    #[inline(always)]
    #[expect(
        clippy::unused_self,
        reason = "a build without the feature keeps nothing to count"
    )]
    pub(super) fn add(&mut self, _by: u64) {}

    #[cfg(feature = "counters")]
    #[must_use]
    pub(super) fn get(self) -> u64 {
        self.n
    }
}

/// Every count the search keeps for its gates, each read on no decision path, so a
/// depth-limited search stays a function of the code.
pub(super) struct Stats {
    pub(super) corrhist_updates: Counter,
    pub(super) corrhist_applied: Counter,
    pub(super) null_attempts: Counter,
    pub(super) null_cutoffs: Counter,
    /// Lets the zugzwang gate see the refusal decide, not merely no null move.
    pub(super) null_refused_material: Counter,
    pub(super) lmr_reductions: Counter,
    pub(super) lmr_researches: Counter,
    pub(super) lmr_pv_lessened: Counter,
    pub(super) lmr_cut_reduced_more: Counter,
    pub(super) lmr_losing_captures: Counter,
    /// Two, so a gate can tell a table that only credits from one that also debits.
    pub(super) history_reduced_less: Counter,
    pub(super) history_reduced_more: Counter,
    pub(super) history_debits: Counter,
    /// As the butterfly's, and the reads that found no frame, so a gate sees the root and the null
    /// move skipped.
    pub(super) continuation_credits: Counter,
    pub(super) continuation_debits: Counter,
    pub(super) continuation_absent: Counter,
    /// The check-exemption count lets a gate see the exemption decide.
    pub(super) futility_nodes: Counter,
    pub(super) futility_skipped: Counter,
    pub(super) futility_kept_check: Counter,
    /// What this rule adds over the margin, which is asked first.
    pub(super) lmp_nodes: Counter,
    pub(super) lmp_skipped: Counter,
    pub(super) lmp_kept_check: Counter,
    /// The refusal count lets a gate see the full-window refusal decide.
    pub(super) reverse_futility_cutoffs: Counter,
    pub(super) reverse_futility_refused_window: Counter,
    pub(super) probcut_attempts: Counter,
    pub(super) probcut_searches: Counter,
    pub(super) probcut_cutoffs: Counter,
    pub(super) probcut_refused_window: Counter,
    pub(super) iir_nodes: Counter,
    /// The first shows the in-check horizon is reached, without which the second proves nothing.
    pub(super) evasion_lists: Counter,
    pub(super) evasion_lists_reordered: Counter,
    /// With the picker's own count, one for each place an exchange is read, so a gate can account
    /// for every read.
    pub(super) probcut_exchanges: Counter,
    pub(super) quiescence_exchanges: Counter,
    pub(super) evasion_exchanges: Counter,
}

impl Stats {
    pub(super) const ZERO: Stats = Stats {
        corrhist_updates: Counter::ZERO,
        corrhist_applied: Counter::ZERO,
        null_attempts: Counter::ZERO,
        null_cutoffs: Counter::ZERO,
        null_refused_material: Counter::ZERO,
        lmr_reductions: Counter::ZERO,
        lmr_researches: Counter::ZERO,
        lmr_pv_lessened: Counter::ZERO,
        lmr_cut_reduced_more: Counter::ZERO,
        lmr_losing_captures: Counter::ZERO,
        history_reduced_less: Counter::ZERO,
        history_reduced_more: Counter::ZERO,
        history_debits: Counter::ZERO,
        continuation_credits: Counter::ZERO,
        continuation_debits: Counter::ZERO,
        continuation_absent: Counter::ZERO,
        futility_nodes: Counter::ZERO,
        futility_skipped: Counter::ZERO,
        futility_kept_check: Counter::ZERO,
        lmp_nodes: Counter::ZERO,
        lmp_skipped: Counter::ZERO,
        lmp_kept_check: Counter::ZERO,
        reverse_futility_cutoffs: Counter::ZERO,
        reverse_futility_refused_window: Counter::ZERO,
        probcut_attempts: Counter::ZERO,
        probcut_searches: Counter::ZERO,
        probcut_cutoffs: Counter::ZERO,
        probcut_refused_window: Counter::ZERO,
        iir_nodes: Counter::ZERO,
        evasion_lists: Counter::ZERO,
        evasion_lists_reordered: Counter::ZERO,
        probcut_exchanges: Counter::ZERO,
        quiescence_exchanges: Counter::ZERO,
        evasion_exchanges: Counter::ZERO,
    };
}

impl Search<'_> {
    #[must_use]
    pub fn nodes(&self) -> u64 {
        self.nodes
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn null_attempts(&self) -> u64 {
        self.stats.null_attempts.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn null_cutoffs(&self) -> u64 {
        self.stats.null_cutoffs.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn null_refused_by_material(&self) -> u64 {
        self.stats.null_refused_material.get()
    }

    /// Late moves first searched at reduced depth.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn lmr_reductions(&self) -> u64 {
        self.stats.lmr_reductions.get()
    }

    /// Reduced searches that beat alpha and were re-run at full depth.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn lmr_researches(&self) -> u64 {
        self.stats.lmr_researches.get()
    }

    /// Late moves at PV nodes whose reduction the PV term lowered.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn lmr_pv_lessened(&self) -> u64 {
        self.stats.lmr_pv_lessened.get()
    }

    /// Late moves at expected cut nodes whose reduction the cut-node term raised.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn lmr_cut_reduced_more(&self) -> u64 {
        self.stats.lmr_cut_reduced_more.get()
    }

    /// Losing captures given a reduction before the PV term.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn lmr_losing_captures(&self) -> u64 {
        self.stats.lmr_losing_captures.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn history_reduced_less(&self) -> u64 {
        self.stats.history_reduced_less.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn history_reduced_more(&self) -> u64 {
        self.stats.history_reduced_more.get()
    }

    /// Quiet moves the cutoffs debited.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn history_debits(&self) -> u64 {
        self.stats.history_debits.get()
    }

    /// Nodes the margin admitted; its siblings count the quiet moves skipped there and the skips
    /// refused for giving check.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn futility_nodes(&self) -> u64 {
        self.stats.futility_nodes.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn futility_skipped(&self) -> u64 {
        self.stats.futility_skipped.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn futility_kept_check(&self) -> u64 {
        self.stats.futility_kept_check.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn lmp_nodes(&self) -> u64 {
        self.stats.lmp_nodes.get()
    }

    /// Observations folded into the correction table, and nodes that read a non-zero correction
    /// back.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn corrhist_updates(&self) -> u64 {
        self.stats.corrhist_updates.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn corrhist_applied(&self) -> u64 {
        self.stats.corrhist_applied.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn lmp_skipped(&self) -> u64 {
        self.stats.lmp_skipped.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn lmp_kept_check(&self) -> u64 {
        self.stats.lmp_kept_check.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn reverse_futility_cutoffs(&self) -> u64 {
        self.stats.reverse_futility_cutoffs.get()
    }

    /// Nodes the margin would have returned but for the full window.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn reverse_futility_refused_by_window(&self) -> u64 {
        self.stats.reverse_futility_refused_window.get()
    }

    /// Nodes that ran the capture probe, captures it searched at reduced depth, and those that cut
    /// the node.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn probcut_attempts(&self) -> u64 {
        self.stats.probcut_attempts.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn probcut_searches(&self) -> u64 {
        self.stats.probcut_searches.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn probcut_cutoffs(&self) -> u64 {
        self.stats.probcut_cutoffs.get()
    }

    /// Probes refused for the full window.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn probcut_refused_by_window(&self) -> u64 {
        self.stats.probcut_refused_window.get()
    }

    /// Nodes searched shallower because their table probe named no move.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn iir_nodes(&self) -> u64 {
        self.stats.iir_nodes.get()
    }

    /// Evasion lists the quiescence search prepared, and those whose head the sort changed.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn evasion_lists(&self) -> u64 {
        self.stats.evasion_lists.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn evasion_lists_reordered(&self) -> u64 {
        self.stats.evasion_lists_reordered.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn probcut_exchanges(&self) -> u64 {
        self.stats.probcut_exchanges.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn quiescence_exchanges(&self) -> u64 {
        self.stats.quiescence_exchanges.get()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn evasion_exchanges(&self) -> u64 {
        self.stats.evasion_exchanges.get()
    }

    /// Since the `Search` was built, not since the run: the cache is kept across runs.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn pawn_cache_probes(&self) -> u64 {
        self.pawns.probes()
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn pawn_cache_hits(&self) -> u64 {
        self.pawns.hits()
    }

    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    #[must_use]
    pub fn continuation(&self) -> &ContinuationHistory {
        &self.conthist
    }

    /// Continuation entries the cutoffs credited.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn continuation_credits(&self) -> u64 {
        self.stats.continuation_credits.get()
    }

    /// Continuation entries the cutoffs debited.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn continuation_debits(&self) -> u64 {
        self.stats.continuation_debits.get()
    }

    /// Offsets a node found no frame for, above the root or below a null move.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn continuation_absent(&self) -> u64 {
        self.stats.continuation_absent.get()
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

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn tunables(&self) -> &crate::tune::Tunables {
        &self.tunables
    }

    #[cfg(feature = "counters")]
    #[must_use]
    pub fn root_filter(&self) -> &[Move] {
        &self.root_filter
    }

    /// Whether a node below the root probes the tables.
    #[cfg(feature = "counters")]
    #[must_use]
    pub fn probes(&self) -> bool {
        self.probing.is_some()
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
            return None;
        }
        let correction = self.corrhist.correction(pawn_key, side);
        self.stats.corrhist_applied.add_if(correction != 0);
        Some(eval::evaluate_cached(board, &mut self.pawns) + correction)
    }

    /// Disqualified: no static reading, a decided score, a noisy or absent best move, or a bound
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
        let Some(eval) = self.evals[ply] else {
            return;
        };
        if score::is_decisive(best) || best_move == Move::NULL || best_move.is_noisy() {
            return;
        }
        let usable = match bound {
            Bound::Exact => true,
            Bound::Lower => best > eval,
            Bound::Upper => best < eval,
        };
        if usable {
            self.corrhist.update(pawn_key, side, best - eval, depth);
            self.stats.corrhist_updates.add_one();
        }
    }

    /// Credits the cutter, `moves[cut]`, and debits every quiet move tried before it, in the
    /// butterfly and in each continuation row the node's frames name. The board is the node's, its
    /// moves unmade.
    pub(super) fn remember_history(
        &mut self,
        board: &Board,
        ply: usize,
        moves: &[Move],
        cut: usize,
        searched: &Searched,
        depth: u32,
    ) {
        let (passed, cut) = (&moves[..cut], moves[cut]);
        if cut.is_noisy() {
            return;
        }
        let us = board.side_to_move();
        let contexts = OFFSETS.map(|offset| conthist::context(&self.frames, ply, offset));
        let bonus = history::bonus(depth);
        self.history.update(us, cut, bonus);
        let cut_to = conthist::piece_to(board, cut);
        for &context in contexts.iter().flatten() {
            self.conthist.update(context, cut_to, bonus);
            self.stats.continuation_credits.add_one();
        }
        for (i, &beaten) in passed.iter().enumerate() {
            if beaten.is_noisy() || !searched.contains(i) {
                continue;
            }
            self.stats.history_debits.add_one();
            self.history.update(us, beaten, -bonus);
            let beaten_to = conthist::piece_to(board, beaten);
            for &context in contexts.iter().flatten() {
                self.conthist.update(context, beaten_to, -bonus);
                self.stats.continuation_debits.add_one();
            }
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

    use cadence_core::position::Board;
    use cadence_core::{START_FEN, generate_legal, parse_uci};

    use crate::conthist;
    use crate::history;
    use crate::search::Search;
    use crate::search::node::Searched;
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

    /// A cutoff after a quiet move the node searched and one it gave up, read in the entries
    /// themselves: a search's own counters filter before they count, so they cannot see this.
    #[test]
    fn a_cutoff_debits_the_quiet_move_searched_and_not_the_one_given_up() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = Search::new(&stop, &tt);
        let board = Board::from_fen(START_FEN).expect("the start position");
        let legal = generate_legal(&board);
        let quiet = |uci| parse_uci(&legal, uci).expect("a legal move");
        let (tried, given_up, cutter) = (quiet("g1f3"), quiet("b1c3"), quiet("e2e4"));
        let mut searched = Searched::default();
        searched.set(0);
        // Any two rows, as the moves one and two plies above the node.
        let contexts = [200, 100];
        search.frames[..2].copy_from_slice(&contexts.map(Some));
        search.remember_history(&board, 2, &[tried, given_up, cutter], 2, &searched, 6);

        let bonus = history::bonus(6);
        let side = board.side_to_move();
        for (m, expected) in [(tried, -bonus), (given_up, 0), (cutter, bonus)] {
            assert_eq!(
                search.history.get(side, m),
                expected,
                "the butterfly, {m:?}"
            );
            for context in contexts {
                let entry = search.conthist.row(context)[conthist::piece_to(&board, m)];
                assert_eq!(i32::from(entry), expected, "row {context}, {m:?}");
            }
        }
    }
}
