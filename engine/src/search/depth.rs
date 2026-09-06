// SPDX-License-Identifier: GPL-3.0-or-later

//! How much less deeply a move is searched than its siblings. Everything here can be undone by
//! a re-search, which is what separates it from the rules in `pruning`.

use cadence_core::Move;

use crate::history;

/// How many times the root depth a check extension is granted within. The cap is doing the job
/// [`extension`] gives it, which is bounding the pathological line, and it is not shaping the
/// ordinary tree: at the depths the bench reaches, what ends an extended line is depth running
/// out rather than ply.
const EXTEND_WITHIN: usize = 2;

/// How much deeper the child of `m` is searched, in plies: one when the move gave check, none
/// otherwise. `check` is the child's `Board::in_check`, read after the move is made.
#[must_use]
pub fn extension(check: bool, ply: usize, root_depth: u32) -> u32 {
    u32::from(check && ply < EXTEND_WITHIN * root_depth as usize)
}

/// How many plies past the one the move would have cost a null-move verification is shortened
/// by: the reduced search runs at `depth - 1 - null_reduction(depth)`, floored at zero, where
/// the floor hands the question to the quiescence search.
#[must_use]
pub fn null_reduction(depth: u32) -> u32 {
    3 + depth / 3
}

/// The first index a late move's search may be shortened at, and the first a late move may be
/// given up at. **One constant read by two rules, and the tie is the argument for
/// [`lmp_count`]'s floor rather than a convenience.** A move inside this prefix is one the
/// search will not shorten by a single ply on the strength of its rank; giving it up entirely
/// on the same evidence is the larger claim, so the rule that cannot re-search takes its floor
/// from the rule that can.
pub const REDUCTION_INDEX: usize = 3;

/// How many plies a late move's first search is shortened by: zero for the first
/// [`REDUCTION_INDEX`] moves of a node, zero below depth three, and otherwise one plus a
/// quarter of the product of the two integer logarithms. The caller holds the exemptions.
#[must_use]
pub fn lmr_reduction(depth: u32, index: usize) -> u32 {
    if depth < 3 || index < REDUCTION_INDEX {
        return 0;
    }
    1 + depth.ilog2() * index.ilog2() / 4
}

/// How many plies the first search of move `m`, at `index` in its node's sorted list, is
/// shortened by. Zero at a node in check, for a move that gives check, for a noisy move and for
/// a killer; otherwise [`lmr_reduction`].
#[must_use]
pub fn reduction(
    in_check: bool,
    gives_check: bool,
    m: Move,
    killers: [Move; 2],
    depth: u32,
    index: usize,
) -> u32 {
    if in_check || gives_check || m.is_noisy() || m == killers[0] || m == killers[1] {
        return 0;
    }
    lmr_reduction(depth, index)
}

/// How many plies a late move whose base reduction is `base` is actually reduced by, once its
/// history score is read. **It adjusts a reduction and never creates one.** A base of zero
/// comes back zero, so every exemption [`reduction`] holds survives whatever the table says,
/// and so does the depth threshold.
#[must_use]
pub fn history_reduction(base: u32, history: i32) -> u32 {
    if base == 0 {
        return 0;
    }
    base.saturating_add_signed(-history::shift(history))
}
