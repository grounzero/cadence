// SPDX-License-Identifier: GPL-3.0-or-later

//! Scores: centipawns, the mate scale, and the bound between them.
//!
//! One integer type for every score the engine computes or prints. No
//! floating point appears on a decision path, so there is none here either.

use cadence_core::MAX_PLY;

/// A score in centipawns from the side to move's point of view, or a mate
/// score: `MATE - ply` for the side delivering mate at `ply`, negated for
/// the side receiving it.
pub type Score = i32;

/// Larger than any score. The initial window and the sentinel for "no score
/// yet"; never a value the search returns.
pub const INFINITE: Score = 32_001;

/// Mate at ply zero. Mate at ply `n` scores `MATE - n`, so a shorter mate
/// scores higher and the root prefers it.
pub const MATE: Score = 32_000;

/// `MAX_PLY` as a score, for the arithmetic below. The conversion is exact:
/// `MAX_PLY` is 256 and this is checked, so the `as` cannot wrap.
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

/// The lowest score that is a mate: mate at `MAX_PLY`. Anything at or above
/// this in magnitude is a mate score; anything below is an evaluation.
pub const MATE_IN_MAX_PLY: Score = MATE - MAX_PLY_SCORE;

/// Every static evaluation lies strictly inside `(-MAX_EVAL, MAX_EVAL)`, and
/// `MAX_EVAL` lies strictly below `MATE_IN_MAX_PLY`, so no evaluation can be
/// mistaken for a mate and no mate for an evaluation.
pub const MAX_EVAL: Score = 30_000;
const _: () = assert!(MAX_EVAL < MATE_IN_MAX_PLY);

pub const DRAW: Score = 0;

/// The score of the side to move being mated at `ply`.
///
/// # Panics
///
/// If `ply` exceeds `MAX_PLY`, which no search can reach.
#[inline]
#[must_use]
pub const fn mated_in(ply: usize) -> Score {
    -MATE + ply_score(ply)
}

/// The score of the side to move delivering mate at `ply`.
///
/// # Panics
///
/// If `ply` exceeds `MAX_PLY`, which no search can reach.
#[inline]
#[must_use]
pub const fn mate_in(ply: usize) -> Score {
    MATE - ply_score(ply)
}

/// Whether `score` is a mate score, for either side.
#[inline]
#[must_use]
pub const fn is_mate(score: Score) -> bool {
    score >= MATE_IN_MAX_PLY || score <= -MATE_IN_MAX_PLY
}

/// The UCI `score` field: `cp <n>`, or `mate <n>` in moves, positive when
/// the side to move mates, negative when it is mated. Mate in one move is
/// `mate 1`; being mated next move is `mate -1`.
#[must_use]
pub fn uci(score: Score) -> String {
    if is_mate(score) {
        let plies = MATE - score.abs();
        // Plies to moves, rounding up: the side to move's own mating move is
        // ply 1 and is move 1; the opponent being mated at ply 2 is still
        // "mate 1" for the side that delivered it.
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

/// A score on its way into the transposition table.
///
/// Every other score in the engine counts mate from the **root**:
/// `mated_in(ply)` is `-MATE + ply`, so the same forced mate has a
/// different number at every ply it is seen from. A table entry outlives
/// the node that wrote it and is read from other plies and other searches,
/// so what is stored is the distance from **that node**: `ply` is added to
/// a winning mate score and subtracted from a losing one, and
/// [`from_tt`] undoes it against the ply doing the reading.
///
/// Getting this wrong is the classic table bug and it does not look like
/// one. The engine plays legal chess, the score is a mate score, and only
/// the distance is wrong, so it announces mate in four, plays a move, and
/// announces mate in four again.
///
/// An evaluation passes through unchanged: `MAX_EVAL` is below
/// `MATE_IN_MAX_PLY`, so the two scales cannot be confused (see
/// [`MAX_EVAL`]).
///
/// # Panics
///
/// If `ply` exceeds `MAX_PLY`, which no search can reach.
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
    // A mate score at `ply` is at most `MATE - ply` in magnitude, so
    // adding `ply` back cannot leave the range; an evaluation is bounded
    // by MAX_EVAL. Neither can reach i16's ends.
    debug_assert!(stored >= i16::MIN as Score && stored <= i16::MAX as Score);
    stored as i16
}

/// A score on its way out of the transposition table: the inverse of
/// [`to_tt`] at the ply doing the reading.
///
/// # Panics
///
/// If `ply` exceeds `MAX_PLY`, which no search can reach.
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
