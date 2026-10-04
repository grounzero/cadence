// SPDX-License-Identifier: GPL-3.0-or-later

//! A skipped move is never looked at again; reductions, which a re-search can undo, are in `depth`.

use cadence_core::position::Board;
use cadence_core::{Move, PieceType};

use super::Search;
use super::depth::REDUCTION_INDEX;
use crate::score::{self, Score};
use crate::tune::{MILLI, Tunable, Tunables};

/// The zugzwang guard: passing is what a side in zugzwang wants and may not have, so the null
/// move's evidence is inverted exactly there.
#[must_use]
pub fn has_non_pawn_material(board: &Board) -> bool {
    let us = board.side_to_move();
    (board.by_colour(us) & !(board.by_type(PieceType::Pawn) | board.by_type(PieceType::King))).any()
}

/// `false` where either reading is missing: an unknown is not known to be improving.
#[must_use]
pub fn improving(evals: &[Option<Score>], ply: usize) -> bool {
    let now = evals.get(ply).copied().flatten();
    let then = ply
        .checked_sub(2)
        .and_then(|p| evals.get(p))
        .copied()
        .flatten();
    match (now, then) {
        (Some(now), Some(then)) => now > then,
        _ => false,
    }
}

/// Above eight the reduction has almost nothing left to delete: 24,000 such moves against 13.3
/// million inside, over the bench. From depth four up the reduction already searches 94% of the
/// moves this rule gives up.
const LMP_DEPTH: u32 = 8;

/// Thousandths of a move per ply squared. The weak end is the large one: at zero every quiet move
/// behind the third is given up, at two the rule reaches almost nothing.
pub(crate) const LMP_MULTIPLIER: i32 = MILLI / 2;

/// Total: the products saturate, and a multiplier of zero gives [`REDUCTION_INDEX`] rather than a
/// division by zero.
#[must_use]
pub fn lmp_count(tunables: &Tunables, depth: u32) -> usize {
    let multiplier = u64::try_from(tunables.get(Tunable::LmpMultiplier)).unwrap_or(0);
    let scaled = u64::from(depth)
        .saturating_mul(u64::from(depth))
        .saturating_mul(multiplier);
    let counted = scaled / u64::from(MILLI.unsigned_abs());
    REDUCTION_INDEX.saturating_add(usize::try_from(counted).unwrap_or(usize::MAX))
}

/// Refused in check, where a wrong skip loses a mate defence and, unlike a reduction, is never
/// re-searched.
#[must_use]
pub fn lmp_index(tunables: &Tunables, in_check: bool, depth: u32, moves: usize) -> Option<usize> {
    let count = lmp_count(tunables, depth);
    (!in_check && depth <= LMP_DEPTH && moves > count).then_some(count)
}

/// The check exemption is the caller's.
#[must_use]
pub fn lmp_skips(from: Option<usize>, m: Move, killers: [Move; 2], index: usize) -> bool {
    from.is_some_and(|count| index >= count) && !m.is_noisy() && m != killers[0] && m != killers[1]
}

/// Past it the rule is off rather than weaker, so only [`futility_skips`]'s exemptions have to be
/// right.
const FUTILITY_DEPTH: u32 = 3;

/// Centipawns per ply. Linear because the material a search can win grows with its moves, not their
/// square.
const FUTILITY_MARGIN: Score = 150;

#[must_use]
pub fn futility_margin(depth: u32) -> Score {
    // Saturating, so the function is total and a gate can pin it; [`futile_node`]'s saturating add
    // keeps the pair from overflowing at any depth.
    FUTILITY_MARGIN.saturating_mul(Score::try_from(depth).unwrap_or(Score::MAX))
}

/// Alpha on the mate scale refuses it; in check `eval` is `None`, so it cannot fire.
#[must_use]
pub fn futile_node(eval: Option<Score>, depth: u32, alpha: Score) -> bool {
    let Some(eval) = eval else {
        return false;
    };
    depth <= FUTILITY_DEPTH
        && !score::is_mate(alpha)
        && eval.saturating_add(futility_margin(depth)) <= alpha
}

/// The check exemption is the caller's.
#[must_use]
pub fn futility_skips(futile: bool, m: Move, index: usize) -> bool {
    futile && index > 0 && !m.is_noisy()
}

/// Centipawns per ply. The rule has no depth limit, so this is its only bound and is chosen to
/// bound as well as to size.
pub(crate) const REVERSE_FUTILITY_MARGIN: Score = 150;

#[must_use]
pub fn reverse_futility_margin(tunables: &Tunables, depth: u32) -> Score {
    // Saturating, for [`futility_margin`]'s reason.
    let per_ply = tunables.get(Tunable::ReverseFutilityMargin);
    per_ply.saturating_mul(Score::try_from(depth).unwrap_or(Score::MAX))
}

/// Returns `beta`, not the evaluation: that is an unverified guess, and as a fail-soft score it
/// loosened every bound above it.
#[must_use]
pub fn reverse_futile(
    tunables: &Tunables,
    eval: Option<Score>,
    depth: u32,
    beta: Score,
) -> Option<Score> {
    let bound = eval?.saturating_sub(reverse_futility_margin(tunables, depth));
    (!score::is_mate(beta) && bound >= beta).then_some(beta)
}

/// Chosen by a shadow measurement, not tuned, with the reduction and margin: the candidate whose
/// cuts save most while disagreeing with the node's own search no more often than the null move's.
const PROBCUT_DEPTH: u32 = 5;

/// The capture's own ply included. Chosen with `PROBCUT_DEPTH` and `PROBCUT_MARGIN` as one reading.
pub const PROBCUT_REDUCTION: u32 = 4;

/// Read against the tree the other margins leave, so a change to reverse futility, futility or late
/// move pruning reopens it.
const PROBCUT_MARGIN: Score = 100;

/// In check `eval` is `None`, so the rule never runs there.
#[must_use]
pub fn probcut_bound(eval: Option<Score>, depth: u32, alpha: Score, beta: Score) -> Option<Score> {
    eval?;
    let raised = beta.saturating_add(PROBCUT_MARGIN);
    (depth >= PROBCUT_DEPTH
        && beta == alpha + 1
        && !score::is_mate(beta)
        && !score::is_mate(raised))
    .then_some(raised)
}

impl Search<'_> {
    /// `gives_check` last: it is the only expensive question, and only a move about to be skipped
    /// has to answer it.
    pub(super) fn futile(&mut self, board: &Board, futile: bool, m: Move, index: usize) -> bool {
        if !futility_skips(futile, m, index) {
            return false;
        }
        if board.gives_check(m) {
            self.futility_kept_check += 1;
            return false;
        }
        self.futility_skipped += 1;
        true
    }

    /// `gives_check` last, as in [`Search::futile`].
    pub(super) fn given_up(
        &mut self,
        board: &Board,
        from: Option<usize>,
        m: Move,
        killers: [Move; 2],
        index: usize,
    ) -> bool {
        if !lmp_skips(from, m, killers, index) {
            return false;
        }
        if board.gives_check(m) {
            self.lmp_kept_check += 1;
            return false;
        }
        self.lmp_skipped += 1;
        true
    }

    pub(super) fn reverse_futility(
        &mut self,
        board: &Board,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Option<Score> {
        let bound = reverse_futile(&self.tunables, self.evals[ply], depth, beta)?;
        if board.halfmove_clock() >= 100 {
            return None;
        }
        if beta != alpha + 1 {
            self.reverse_futility_refused_window += 1;
            return None;
        }
        self.reverse_futility_cutoffs += 1;
        Some(bound)
    }
}
