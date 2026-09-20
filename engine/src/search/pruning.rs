// SPDX-License-Identifier: GPL-3.0-or-later

//! The rules that skip a move or a node outright, and the margins they read. A rule here never
//! looks at what it skipped, which is what separates it from a reduction in `depth`.

use cadence_core::position::Board;
use cadence_core::{Move, PieceType};

use super::depth::REDUCTION_INDEX;
use crate::score::{self, Score};
use crate::tune::{MILLI, Tunable, Tunables};

/// Whether the side to move has any piece beside its pawns and king. The null move's zugzwang
/// guard, and the one condition there that is a chess claim rather than a search claim: passing
/// is what a side in zugzwang wants and may not have, so the evidence a null move collects is
/// inverted exactly there.
#[must_use]
pub fn has_non_pawn_material(board: &Board) -> bool {
    let us = board.side_to_move();
    (board.by_colour(us) & !(board.by_type(PieceType::Pawn) | board.by_type(PieceType::King))).any()
}

/// Whether the side to move's position is improving: the static evaluation written at this ply
/// against the one two plies back. `false` wherever either reading is missing, because a rule
/// reading this flag wants "known to be getting better", and an unknown is not that.
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

/// The deepest node at which a quiet move may be given up for its place in the order. **Eight,
/// and what bounds it is where the reduction still has something to delete.** Over the bench
/// positions the moves this rule would give up are ones the reduction searches at reduced depth
/// in 94% of cases from depth four up; at depths nine and above there are 24,000 of them
/// against 13.3 million inside the band, so the rule is off there because there is nothing
/// there rather than because it is unsafe there.
const LMP_DEPTH: u32 = 8;

/// What the count of moves a node searches grows by, in thousandths of a move per ply squared:
/// the square of the remaining depth times this. **The weak end is the large one**: at zero the
/// count is [`REDUCTION_INDEX`] and every quiet move behind the third is given up, while at two
/// the count outruns the move lists and the rule reaches almost nothing.
pub(crate) const LMP_MULTIPLIER: i32 = MILLI / 2;

/// How many moves a node at `depth` searches before the quiet moves behind them are given up.
/// Total for [`futility_margin`]'s reason: the products saturate, and a multiplier of zero is a
/// count of [`REDUCTION_INDEX`] rather than a division by zero.
#[must_use]
pub fn lmp_count(tunables: &Tunables, depth: u32) -> usize {
    let multiplier = u64::try_from(tunables.get(Tunable::LmpMultiplier)).unwrap_or(0);
    let scaled = u64::from(depth)
        .saturating_mul(u64::from(depth))
        .saturating_mul(multiplier);
    let counted = scaled / u64::from(MILLI.unsigned_abs());
    REDUCTION_INDEX.saturating_add(usize::try_from(counted).unwrap_or(usize::MAX))
}

/// The index from which this node gives up its quiet moves, or `None` where the rule cannot
/// fire here at all: past [`LMP_DEPTH`], at a node in check, or at a node with no move the
/// count does not already admit. **In check is refused here and not left to the move.** Every
/// legal move at such a node is an evasion and what a wrong skip loses there is a mate defence,
/// which is the exemption this rule's asymmetry bears on hardest: [`reduction`] refuses the
/// same node and can afford to be wrong, because a reduced search that beats alpha is re-run.
#[must_use]
pub fn lmp_index(tunables: &Tunables, in_check: bool, depth: u32, moves: usize) -> Option<usize> {
    let count = lmp_count(tunables, depth);
    (!in_check && depth <= LMP_DEPTH && moves > count).then_some(count)
}

/// Whether the move at `index` of a node [`lmp_index`] admitted is a candidate to be given up
/// without being searched: never a noisy move, never a killer, and never inside the count. The
/// check exemption is the caller's, because it is the one question here that costs anything,
/// which is [`futility_skips`]'s division of the same work.
#[must_use]
pub fn lmp_skips(from: Option<usize>, m: Move, killers: [Move; 2], index: usize) -> bool {
    from.is_some_and(|count| index >= count) && !m.is_noisy() && m != killers[0] && m != killers[1]
}

/// The deepest node at which a quiet move may be skipped for the margin. **Below the limit the
/// rule is off, not weaker.** A node outside the band searches every move it generates, so the
/// exemptions at [`futility_skips`] are the only thing that has to be right about the nodes
/// inside it.
const FUTILITY_DEPTH: u32 = 3;

/// What the margin grows by per ply of remaining depth, in centipawns. **Linear rather than
/// squared because the evidence is linear**: the material a search can win grows with the moves
/// it has, not with their square.
const FUTILITY_MARGIN: Score = 150;

/// How far below alpha a node's static evaluation may sit and still have its quiet moves
/// searched: [`FUTILITY_MARGIN`] per ply of `depth`.
#[must_use]
pub fn futility_margin(depth: u32) -> Score {
    // Saturating, and total for that reason: no caller passes a depth outside the band, and a
    // function that is only right for the arguments something happens to hand it is one a gate
    // cannot pin. [`futile_node`] adds it to the evaluation with a saturating add for the same
    // reason, so the pair cannot overflow at any depth at all.
    FUTILITY_MARGIN.saturating_mul(Score::try_from(depth).unwrap_or(Score::MAX))
}

/// Whether a node may skip quiet moves for the margin: its static evaluation plus
/// [`futility_margin`] still does not reach `alpha`. Alpha on the mate scale refuses it, and in
/// check `evals[ply]` is `None`, so the rule cannot read anything and cannot fire.
#[must_use]
pub fn futile_node(eval: Option<Score>, depth: u32, alpha: Score) -> bool {
    let Some(eval) = eval else {
        return false;
    };
    depth <= FUTILITY_DEPTH
        && !score::is_mate(alpha)
        && eval.saturating_add(futility_margin(depth)) <= alpha
}

/// Whether the move at `index` of a node [`futile_node`] admitted is a candidate to be skipped
/// without being searched: never the node's first move, and never a noisy one. The check
/// exemption is the caller's, because it is the one question here that costs anything.
#[must_use]
pub fn futility_skips(futile: bool, m: Move, index: usize) -> bool {
    futile && index > 0 && !m.is_noisy()
}

/// What the margin a node is returned on grows by per ply of remaining depth, in centipawns.
/// **It is the only thing bounding this rule**, because there is no depth limit here, so it is
/// chosen where it bounds as well as where it sizes.
pub(crate) const REVERSE_FUTILITY_MARGIN: Score = 150;

/// How far above `beta` a node's static evaluation must stand before the node is returned
/// without being searched: the reverse futility margin `tunables` holds, per ply of `depth`.
#[must_use]
pub fn reverse_futility_margin(tunables: &Tunables, depth: u32) -> Score {
    // Saturating, and total for that reason, like [`futility_margin`].
    let per_ply = tunables.get(Tunable::ReverseFutilityMargin);
    per_ply.saturating_mul(Score::try_from(depth).unwrap_or(Score::MAX))
}

/// The bound a node may be returned at without being searched at all: its static evaluation
/// less [`reverse_futility_margin`], where that still stands at or above `beta`. `None` is a
/// node that has to be searched.
#[must_use]
pub fn reverse_futile(
    tunables: &Tunables,
    eval: Option<Score>,
    depth: u32,
    beta: Score,
) -> Option<Score> {
    let bound = eval?.saturating_sub(reverse_futility_margin(tunables, depth));
    (!score::is_mate(beta) && bound >= beta).then_some(bound)
}
