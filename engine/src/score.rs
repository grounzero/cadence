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

/// A tablebase win at ply `p` scores `TB_WIN - p`, so every found mate outranks every tablebase
/// win.
pub const TB_WIN: Score = MATE_IN_MAX_PLY - 1;

/// At or above this in magnitude a score is decided, by a mate or a table; below is an evaluation.
pub const TB_WIN_IN_MAX_PLY: Score = TB_WIN - MAX_PLY_SCORE;
const _: () = assert!(MAX_EVAL < TB_WIN_IN_MAX_PLY);

/// A drawn position's score, at every draw rule.
pub const DRAW: Score = 0;

/// What an aborted search returns, and what is reported before any line finishes. Every caller
/// discards an aborted value, so it never judges the position and need not follow `DRAW`.
pub const ABORTED: Score = 0;

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

/// # Panics
///
/// If `ply` exceeds `MAX_PLY`.
#[inline]
#[must_use]
pub const fn tb_win_in(ply: usize) -> Score {
    TB_WIN - ply_score(ply)
}

/// # Panics
///
/// If `ply` exceeds `MAX_PLY`.
#[inline]
#[must_use]
pub const fn tb_loss_in(ply: usize) -> Score {
    -TB_WIN + ply_score(ply)
}

#[inline]
#[must_use]
pub const fn is_mate(score: Score) -> bool {
    score >= MATE_IN_MAX_PLY || score <= -MATE_IN_MAX_PLY
}

/// A mate or a tablebase result: off the evaluation's scale, so no margin or learner may read it as
/// one.
#[inline]
#[must_use]
pub const fn is_decisive(score: Score) -> bool {
    score >= TB_WIN_IN_MAX_PLY || score <= -TB_WIN_IN_MAX_PLY
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

/// Elsewhere a mate or a tablebase result counts from the root, so one result has a different
/// number at every ply; the table stores it relative to the node.
///
/// # Panics
///
/// If `ply` exceeds `MAX_PLY`.
#[inline]
#[must_use]
#[expect(clippy::cast_possible_truncation, reason = "asserted in range below")]
pub const fn to_tt(score: Score, ply: usize) -> i16 {
    let stored = if score >= TB_WIN_IN_MAX_PLY {
        score + ply_score(ply)
    } else if score <= -TB_WIN_IN_MAX_PLY {
        score - ply_score(ply)
    } else {
        score
    };
    // Adding `ply` back to a decided score cannot leave the range, and an evaluation is bounded by
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
    if score >= TB_WIN_IN_MAX_PLY {
        score - ply_score(ply)
    } else if score <= -TB_WIN_IN_MAX_PLY {
        score + ply_score(ply)
    } else {
        score
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// `from_tt` inverts `to_tt` at every ply, and every stored value fits in an `i16`.
    #[kani::proof]
    fn the_table_scale_round_trips() {
        let score: Score = kani::any();
        kani::assume((-INFINITE..=INFINITE).contains(&score));
        let ply: usize = kani::any();
        kani::assume(ply <= MAX_PLY);
        assert_eq!(from_tt(to_tt(score, ply), ply), score);
    }

    /// Mate and tablebase scores are decisive at every ply, and no evaluation inside `MAX_EVAL` is.
    #[kani::proof]
    fn the_scales_do_not_overlap() {
        let ply: usize = kani::any();
        kani::assume(ply <= MAX_PLY);
        assert!(is_mate(mate_in(ply)) && is_mate(mated_in(ply)));
        assert!(is_decisive(tb_win_in(ply)) && !is_mate(tb_win_in(ply)));
        assert!(is_decisive(tb_loss_in(ply)) && !is_mate(tb_loss_in(ply)));
        let eval: Score = kani::any();
        kani::assume(eval > -MAX_EVAL && eval < MAX_EVAL);
        assert!(!is_decisive(eval));
    }
}
