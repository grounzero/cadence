// SPDX-License-Identifier: GPL-3.0-or-later

//! Time management: how much of the clock one move may use.
//!
//! A pure function of its arguments, in integer milliseconds, so that it can
//! be tested as arithmetic (`tests/time.rs`) and so that the search consults
//! a clock only when this says there is one. **Under a fixed-depth or node limit there is no budget and the
//! search never reads the time**, which is what makes `bench` a function of
//! the code alone.
//!
//! **The hard budget also reads how far into the game the position is**,
//! which is the one input here that is not a `go` limit. It arrives as an
//! integer derived by [`progress`], so the arithmetic below stays a pure
//! function of its arguments and can still be tested as arithmetic.
//!
//! **Both budgets are capped at half of what is left after the overhead.**
//! That cap is the "never" in "never lose on time": however the fractions
//! are tuned later, no move can spend more than half the clock, so search
//! time alone can never run it out.

use cadence_core::Colour;
use cadence_core::position::Board;

use crate::eval;
use crate::search::Limits;

/// Milliseconds held back from every budget for the cost of getting the
/// move out: the pipe to the GUI, thread scheduling, the GUI's own clock.
pub const MOVE_OVERHEAD_MS: u64 = 20;

/// The far end of the progress scale: a pawn ending, where no minor or
/// major piece is left. Zero is the opening's full complement, so the scale
/// is the evaluation's own read the other way up and the two cannot
/// disagree about where a middlegame is.
pub const PROGRESS_MAX: u32 = eval::PHASE_MAX as u32;

/// How far into the game `board` is, `0..=PROGRESS_MAX`.
///
/// Material and not the move number, because a move number counts shuffling
/// and a book exit alike. It is `eval::phase` subtracted from its own
/// maximum, so a position the evaluation calls an endgame is one this calls
/// late.
#[must_use]
pub fn progress(board: &Board) -> u32 {
    PROGRESS_MAX.saturating_sub(eval::phase(board) as u32)
}

/// The hard budget's multiple of the soft one at the opening's full
/// complement, in twelfths, falling by one twelfth per step of progress.
/// So four times the soft budget at the opening, three at the middle of the
/// scale where the old constant sat, and twice it in a pawn ending.
const HARD_AT_THE_OPENING: u64 = 4 * HARD_SCALE;

/// The denominator the multiple above is counted in. Twelfths, because the
/// scale has twenty-four steps and this walks it from four down to two.
const HARD_SCALE: u64 = 12;

const _: () = assert!(HARD_AT_THE_OPENING - PROGRESS_MAX as u64 == 2 * HARD_SCALE);

/// A time budget for one move, in milliseconds from the start of the search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Do not start another iteration once this much has elapsed.
    pub soft: u64,
    /// Stop searching, mid-iteration if need be, once this much has elapsed.
    pub hard: u64,
}

/// The budget for `us` under `limits` at `progress`, or `None` when nothing
/// in `limits` constrains the time: no `movetime`, and no clock named for
/// either side. `movetime` takes precedence over a clock when both are
/// given, and ignores `progress` because it names the budget exactly.
///
/// **`None` means "unlimited by design", and nothing else.** It is what
/// `bench`, `go depth`, `go nodes` and `go infinite` get, and it is the
/// licence not to read the clock at all.
#[must_use]
pub fn budget(limits: &Limits, us: Colour, progress: u32) -> Option<Budget> {
    if let Some(movetime) = limits.movetime {
        let t = movetime.saturating_sub(MOVE_OVERHEAD_MS);
        return Some(Budget { soft: t, hard: t });
    }
    if !limits.is_clocked() {
        return None;
    }
    // A clock was named and it was not ours. There is no information here
    // about our own time, and the safe reading of that is not "unlimited"
    // but zero: soft and hard of zero, the first iteration returned and no
    // more. It is the rule `Limits::parse` already applies to a *negative*
    // clock, which a GUI sends when a side has overstepped -- that reads as
    // zero rather than as "no clock", so the search still hurries -- and a
    // clock we were never told is no better informed than one that has run
    // out.
    let (time, inc) = limits.clock(us).unwrap_or((0, 0));
    let avail = time.saturating_sub(MOVE_OVERHEAD_MS);
    let cap = avail / 2;
    let share = match limits.movestogo {
        Some(mtg) => avail / u64::from(mtg.max(1)),
        None => avail / 25,
    };
    let soft = (share + inc * 3 / 4).min(cap);
    // How far past its share an iteration already under way may run, and it
    // shrinks as the game does. Clamped rather than trusted, because a
    // multiple driven under `HARD_SCALE` would put the hard budget below the
    // soft one and make `another_iteration_fits` inert.
    let multiple = HARD_AT_THE_OPENING - u64::from(progress.min(PROGRESS_MAX));
    let hard = (soft * multiple / HARD_SCALE).min(cap);
    Some(Budget { soft, hard })
}

/// Whether to start another iteration, given the elapsed milliseconds at the
/// end of each completed one and the budget.
///
/// **EBF is read over two iterations and never one**, and the obvious
/// simplification to a single ratio is the bug this exists to not be.
/// Alpha-beta iteration times alternate: an iteration whose parity suits the
/// ordering the last one left is cheap and the next is dear, so a one-step
/// ratio samples half a period and reads the alternation as the trend --
/// under-predicting at exactly the step where the answer matters, and
/// starting an iteration that cannot finish. Two steps span a full period,
/// and the elapsed times two apart are in the ratio EBF squared. A
/// fixed-depth bench cannot see any of this.
///
/// **The rule is inert where the hard budget equals the soft one**, which is
/// what `movetime` gives: nothing later gets what this move does not spend,
/// so refusing to start an iteration would only give up the chance that the
/// prediction was wrong. The condition is read off the budget rather than
/// off the limits, so it is a property of what there is to save.
///
/// **There is no safety factor on the prediction.** One would be a tunable
/// fraction, which is the thing this rule exists to not be.
///
/// **Every way of not knowing answers "start it":** fewer than three
/// completed iterations, or one two back too fast to have taken a
/// millisecond. The search must always have a move.
///
/// Integer throughout, like the rest of this module: no float reaches a path
/// that decides anything.
#[must_use]
pub fn another_iteration_fits(completed: &[u64], budget: Budget) -> bool {
    if budget.hard <= budget.soft {
        return true;
    }
    let n = completed.len();
    if n < 3 {
        return true;
    }
    let elapsed = completed[n - 1];
    let two_back = completed[n - 3];
    if two_back == 0 {
        return true;
    }
    // Two iterations apart the elapsed times are in the ratio EBF squared.
    // Scaled by a million so the root comes back in thousandths, which is
    // enough resolution to be exact at this scale and keeps every step an
    // integer one.
    let ebf_milli = (elapsed.saturating_mul(1_000_000) / two_back).isqrt();
    let predicted = elapsed.saturating_mul(ebf_milli) / 1_000;
    predicted <= budget.hard
}
