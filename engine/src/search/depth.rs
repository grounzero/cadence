// SPDX-License-Identifier: GPL-3.0-or-later

//! A re-search can undo every move's reduction here; the rules that cannot be undone are in
//! `pruning`, and [`iir_reduction`], which shortens the node itself, is the one exception.

use cadence_core::Move;

use crate::history;
use crate::score::{self, Score};
use crate::tt::{Bound, Hit};
use crate::tune::{Tunable, Tunables};

/// The backstop beside the count's cap at the root depth, which refuses a grant here first. At
/// bench depths an extended line ends on depth, not ply.
const EXTEND_WITHIN: usize = 2;

/// `check` is the child's `in_check`, read after the move is made.
#[must_use]
pub fn extension(check: bool, ply: usize, root_depth: u32) -> u32 {
    u32::from(check && ply < EXTEND_WITHIN * root_depth as usize)
}

/// What a line may still be extended by: its extensions of every kind stop at the root depth, so no
/// interior node lies past twice the root depth from the root.
#[must_use]
pub fn headroom(taken: u32, root_depth: u32) -> u32 {
    root_depth.saturating_sub(taken)
}

/// A positive extension held to the line's headroom. A negative one is a reduction of one move and
/// reads none.
#[must_use]
pub fn granted(ext: i32, taken: u32, root_depth: u32) -> i32 {
    ext.min(i32::try_from(headroom(taken, root_depth)).unwrap_or(i32::MAX))
}

/// The child's count, which never falls: a negative extension gives no headroom back.
#[must_use]
pub fn counted(taken: u32, ext: i32) -> u32 {
    taken.saturating_add(ext.max(0).unsigned_abs())
}

/// One ply below the parent, moved by the extension, and never below the quiescence search.
#[must_use]
pub fn child_depth(depth: u32, ext: i32) -> u32 {
    depth.saturating_add_signed(ext).saturating_sub(1)
}

/// The shallowest node that asks whether its table's move is singular, where a net's search starts.
pub const SINGULAR_DEPTH: i32 = 8;

/// A shallowest depth that asks nowhere, which the handcrafted evaluation's search reads so that it
/// searches as `0.6.0` did. The count's cap at the root depth is off with it.
pub const SINGULAR_OFF: i32 = 0;

/// Whether a search reading `tunables` asks the singular question and caps each line's extensions.
#[must_use]
pub fn asks_singular(tunables: &Tunables) -> bool {
    tunables.get(Tunable::SingularDepth) != SINGULAR_OFF
}

/// Centipawns a ply below the stored score at which another move counts as holding it.
pub const SINGULAR_MARGIN: i32 = 2;

/// How far below the node's depth a stored entry may lie and still be evidence.
const SINGULAR_WITHIN: u32 = 3;

/// Saturating, so the function is total at any depth and margin.
#[must_use]
pub fn singular_beta(tt_score: Score, depth: u32, margin: Score) -> Score {
    tt_score.saturating_sub(margin.saturating_mul(Score::try_from(depth).unwrap_or(Score::MAX)))
}

/// Half the depth below the node, rounded down: the verification establishes a bound, not a value.
#[must_use]
pub fn verification_depth(depth: u32) -> u32 {
    depth.saturating_sub(1) / 2
}

/// The singular beta where a node asks whether its table's move is singular, and `None` where it
/// does not. The caller passes only an entry whose move is legal here; a set that asks, asks below
/// the root at its depth or deeper, of an entry naming a move, not an upper bound, within three
/// plies, with neither score on the decided scale.
#[must_use]
pub fn singular_admission(
    hit: Option<Hit>,
    depth: u32,
    ply: usize,
    tunables: &Tunables,
) -> Option<Score> {
    let hit = hit.filter(|_| asks_singular(tunables))?;
    let shallowest = tunables.get(Tunable::SingularDepth).unsigned_abs();
    if ply == 0
        || depth < shallowest
        || hit.mv.is_null()
        || hit.bound == Bound::Upper
        || u32::from(hit.depth) + SINGULAR_WITHIN < depth
    {
        return None;
    }
    let tt_score = score::from_tt(hit.score, ply);
    let s_beta = singular_beta(tt_score, depth, tunables.get(Tunable::SingularMargin));
    if score::is_decisive(tt_score) || score::is_decisive(s_beta) {
        return None;
    }
    Some(s_beta)
}

/// The shallowest node searched shallower for the move its table did not name.
pub(crate) const IIR_DEPTH: u32 = 4;

/// What every search shortens that node by, unless a gate sets it otherwise.
pub(crate) const IIR_REDUCTION: u32 = 2;

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

/// `losing` is [`losing_capture`]'s answer, read before the move was made.
#[must_use]
pub fn reduction(
    in_check: bool,
    gives_check: bool,
    m: Move,
    losing: bool,
    killers: [Move; 2],
    depth: u32,
    index: usize,
) -> u32 {
    if in_check || gives_check || m == killers[0] || m == killers[1] {
        return 0;
    }
    if m.is_noisy() {
        // One ply where a quiet move here would be reduced at all, and only for a losing capture.
        return u32::from(losing).min(lmr_reduction(depth, index));
    }
    lmr_reduction(depth, index)
}

/// A capture whose exchange the picker read as losing, at an index the reduction reaches. The
/// verdict is the picker's, so no exchange is read again here.
#[must_use]
pub fn losing_capture(m: Move, index: usize, lost: bool) -> bool {
    index >= REDUCTION_INDEX && m.is_capture() && lost
}

/// None for a capture, which the quiet history does not score.
#[must_use]
pub fn move_history(m: Move, history: i32) -> i32 {
    if m.is_noisy() { 0 } else { history }
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

/// One ply more at an expected cut node, after every other term. It never creates a reduction, so
/// every exemption in [`reduction`] holds.
#[must_use]
pub fn cut_node_reduction(r: u32, cut: bool) -> u32 {
    r + u32::from(cut && r > 0)
}

/// At least one ply. A reduction is made only from depth 3, where the child has two, so the floor
/// never lifts a reduced search above the move's unreduced depth.
#[must_use]
pub fn reduced_depth(child: u32, r: u32) -> u32 {
    child.saturating_sub(r).max(1)
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// No exempt move is reduced, a noisy one by at most a ply, and no later term creates one.
    #[kani::proof]
    fn the_exemptions_hold_through_every_term() {
        let (in_check, gives_check, losing) = (kani::any(), kani::any(), kani::any());
        let m = Move::from_bits(kani::any());
        let killers = [Move::from_bits(kani::any()), Move::from_bits(kani::any())];
        let (depth, index): (u32, usize) = (kani::any(), kani::any());
        let base = reduction(in_check, gives_check, m, losing, killers, depth, index);
        if in_check || gives_check || m == killers[0] || m == killers[1] {
            assert!(base == 0);
        }
        if m.is_noisy() {
            assert!(base <= u32::from(losing));
        }
        let (history, pv, cut): (i32, bool, bool) = (kani::any(), kani::any(), kani::any());
        let r = cut_node_reduction(pv_reduction(history_reduction(base, history), pv), cut);
        assert!(base > 0 || r == 0);
    }

    /// The chain never overflows, and a reduced search keeps between one ply and the move's depth.
    #[kani::proof]
    fn a_reduced_depth_stays_between_one_ply_and_the_moves() {
        let (depth, index): (u32, usize) = (kani::any(), kani::any());
        let (history, pv, cut): (i32, bool, bool) = (kani::any(), kani::any(), kani::any());
        let base = lmr_reduction(depth, index);
        let r = cut_node_reduction(pv_reduction(history_reduction(base, history), pv), cut);
        let child: u32 = kani::any();
        kani::assume(child >= 1);
        let reduced = reduced_depth(child, r);
        assert!(reduced >= 1 && reduced <= child);
    }
}
