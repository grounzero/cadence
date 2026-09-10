// SPDX-License-Identifier: GPL-3.0-or-later

//! What a finished search will tell you about itself. Every item here reads a field and calls
//! nothing, which is what lets them sit in a child module at all.

use std::sync::atomic::Ordering;

use cadence_core::Move;

use super::{Limits, Search};
use crate::history::History;
use crate::score::Score;

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

    /// How often a node lost a ply because its table probe named no move. There is one counter
    /// and not three because what the rule saves is the node count itself, so a second figure
    /// would be the first one restated.
    #[must_use]
    pub fn iir_nodes(&self) -> u64 {
        self.iir_nodes
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
}
