// SPDX-License-Identifier: GPL-3.0-or-later

//! A re-search can undo every move's reduction here; the rules that cannot be undone are in
//! `pruning`, and [`iir_reduction`], which shortens the node itself, is the one exception.

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

/// The shallowest node searched shallower for the move its table did not name.
const IIR_DEPTH: u32 = 4;

/// What every search shortens that node by, unless a gate sets it otherwise.
pub(crate) const IIR_REDUCTION: u32 = 0;

/// `plies` where the node qualifies; a PV node and a node in check keep their depth. The node
/// stores its result at the shortened depth, so no re-search of its own undoes the reduction.
#[must_use]
pub fn iir_reduction(depth: u32, pv: bool, in_check: bool, has_tt_move: bool, plies: u32) -> u32 {
    if pv || in_check || has_tt_move || depth < IIR_DEPTH {
        return 0;
    }
    plies
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

/// The final reduction at a PV node, taken after the history term. It never creates a reduction,
/// so every exemption in [`reduction`] holds.
#[must_use]
pub fn pv_reduction(r: u32, pv: bool) -> u32 {
    r.saturating_sub(u32::from(pv))
}
