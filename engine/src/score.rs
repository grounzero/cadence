// SPDX-License-Identifier: GPL-3.0-or-later

//! One integer type for every score the engine computes or prints.

use cadence_core::MAX_PLY;

/// From the side to move's point of view; a mate is `MATE - ply` for the side delivering it,
/// negated for the side receiving it.
pub type Score = i32;

/// Never a value the search returns.
pub const INFINITE: Score = 32_001;

/// A shorter mate scores higher, so the root prefers it.
pub const MATE: Score = 32_000;

/// `MAX_PLY` is 256 and checked, so the `as` cannot wrap.
#[expect(clippy::cast_possible_wrap, reason = "MAX_PLY is 256; asserted below")]
const MAX_PLY_SCORE: Score = MAX_PLY as Score;
const _: () = assert!(MAX_PLY_SCORE as usize == MAX_PLY);

/// `ply` as a score. Every ply the search hands in is at most `MAX_PLY`.
#[inline]
#[expect(clippy::cast_possible_wrap, reason = "ply <= MAX_PLY, asserted")]
const fn ply_score(ply: usize) -> Score {
    assert!(ply <= MAX_PLY);
    ply as Score
}

/// At or above this in magnitude is a mate; below is an evaluation.
pub const MATE_IN_MAX_PLY: Score = MATE - MAX_PLY_SCORE;

/// Evaluations lie strictly inside `(-MAX_EVAL, MAX_EVAL)`, below `MATE_IN_MAX_PLY`, so neither
/// scale can be mistaken for the other.
pub const MAX_EVAL: Score = 30_000;
const _: () = assert!(MAX_EVAL < MATE_IN_MAX_PLY);

pub const DRAW: Score = 0;

/// # Panics
///
/// If `ply` exceeds `MAX_PLY`.
#[inline]
#[must_use]
pub const fn mated_in(ply: usize) -> Score {
    -MATE + ply_score(ply)
}

/// # Panics
///
/// If `ply` exceeds `MAX_PLY`.
#[inline]
#[must_use]
pub const fn mate_in(ply: usize) -> Score {
    MATE - ply_score(ply)
}

#[inline]
#[must_use]
pub const fn is_mate(score: Score) -> bool {
    score >= MATE_IN_MAX_PLY || score <= -MATE_IN_MAX_PLY
}

/// Mate in moves, positive when the side to move mates: `mate 1` is mate next move, `mate -1` being
/// mated next move.
#[must_use]
pub fn uci(score: Score) -> String {
    if is_mate(score) {
        let plies = MATE - score.abs();
        // Rounding up: the mating move is ply 1 and move 1, and ply 2's mate is still mate 1 for
        // the side that delivered it.
        let moves = (plies + 1) / 2;
        if score > 0 {
            format!("mate {moves}")
        } else {
            format!("mate -{moves}")
        }
    } else {
        format!("cp {score}")
    }
}

// ---------------------------------------------------------------------------
// The transposition table's scale
// ---------------------------------------------------------------------------

/// Elsewhere mate counts from the root, so one forced mate has a different number at every ply;
/// the table stores it relative to the node.
///
/// # Panics
///
/// If `ply` exceeds `MAX_PLY`.
#[inline]
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "asserted in range below")]
pub const fn to_tt(score: Score, ply: usize) -> i16 {
    let stored = if score >= MATE_IN_MAX_PLY {
        score + ply_score(ply)
    } else if score <= -MATE_IN_MAX_PLY {
        score - ply_score(ply)
    } else {
        score
    };
    // Adding `ply` back to a mate cannot leave the range, and an evaluation is bounded by
    // `MAX_EVAL`, so neither reaches `i16`'s ends.
    debug_assert!(stored >= i16::MIN as Score && stored <= i16::MAX as Score);
    stored as i16
}

/// The inverse of [`to_tt`] at the reading ply.
///
/// # Panics
///
/// If `ply` exceeds `MAX_PLY`.
#[inline]
#[must_use]
pub const fn from_tt(stored: i16, ply: usize) -> Score {
    let score = stored as Score;
    if score >= MATE_IN_MAX_PLY {
        score - ply_score(ply)
    } else if score <= -MATE_IN_MAX_PLY {
        score + ply_score(ply)
    } else {
        score
    }
}
