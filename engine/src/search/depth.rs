// SPDX-License-Identifier: GPL-3.0-or-later

//! A re-search can undo everything here; the rules that cannot be undone are in `pruning`.

use cadence_core::Move;

use crate::history;

/// The cap bounds the pathological line, not the ordinary tree: at bench depths an extended line
/// ends on depth, not ply.
const EXTEND_WITHIN: usize = 2;

/// `check` is the child's `in_check`, read after the move is made.
#[must_use]
pub fn extension(check: bool, ply: usize, root_depth: u32) -> u32 {
    u32::from(check && ply < EXTEND_WITHIN * root_depth as usize)
}

/// The reduced search runs at `depth - 1 - null_reduction(depth)`, floored at zero, where the
/// quiescence search takes over.
#[must_use]
pub fn null_reduction(depth: u32) -> u32 {
    3 + depth / 3
}

/// Also the floor for giving a move up: the rule that cannot re-search claims no more than the one
/// that can.
pub const REDUCTION_INDEX: usize = 3;

/// The caller holds the exemptions.
#[must_use]
pub fn lmr_reduction(depth: u32, index: usize) -> u32 {
    if depth < 3 || index < REDUCTION_INDEX {
        return 0;
    }
    1 + depth.ilog2() * index.ilog2() / 4
}

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

/// Adjusts a reduction and never creates one, so every exemption in [`reduction`] holds.
#[must_use]
pub fn history_reduction(base: u32, history: i32) -> u32 {
    if base == 0 {
        return 0;
    }
    base.saturating_add_signed(-history::shift(history))
}
