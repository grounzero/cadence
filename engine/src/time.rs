// SPDX-License-Identifier: GPL-3.0-or-later

//! Time management: how much of the clock one move may use.
//!
//! A pure function of the `go` limits and the side to move, in integer
//! milliseconds, so that it can be tested as arithmetic (`tests/time.rs`)
//! and so that the search consults a clock only when this says there is
//! one. Under a fixed-depth or node limit there is no budget, and the
//! search never reads the time -- which is what makes `bench` a function of
//! the code alone.
//!
//! The allocation is the simplest one that never loses on time:
//!
//! - `movetime N` is spent as given, less the overhead.
//! - On a clock, a *soft* budget of a fraction of the time left plus most of
//!   the increment -- one twenty-fifth in sudden death, the even share when
//!   `movestogo` is given -- and a *hard* budget of three times that. The
//!   search does not start an iteration past the soft budget and abandons
//!   one at the hard budget.
//! - Both are capped at **half of what is left after the overhead**. That
//!   cap is the "never" in "never lose on time": however the fractions are
//!   tuned later, no move can spend more than half the clock, so search
//!   time alone can never run it out, and latency up to the overhead per
//!   move is covered by an increment that covers it.
//! - And an iteration is started only if it is predicted to finish inside
//!   the hard budget: [`another_iteration_fits`].
//!
//! Below the overhead there is no time to spend; the budget is zero and the
//! search returns its first iteration, which completes in microseconds. A
//! `go` that named the other side's clock and not ours lands in the same
//! place, and for the same reason: nothing is known about our own time.

use cadence_core::Colour;

use crate::search::Limits;

/// Milliseconds held back from every budget for the cost of getting the
/// move out: the pipe to the GUI, thread scheduling, the GUI's own clock.
pub const MOVE_OVERHEAD_MS: u64 = 20;

/// A time budget for one move, in milliseconds from the start of the search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Do not start another iteration once this much has elapsed.
    pub soft: u64,
    /// Stop searching, mid-iteration if need be, once this much has elapsed.
    pub hard: u64,
}

/// The budget for `us` under `limits`, or `None` when nothing in `limits`
/// constrains the time: no `movetime`, and no clock named for either side.
///
/// `movetime` takes precedence over a clock when both are given.
///
/// **`None` means "unlimited by design", and nothing else.** It is what
/// `bench`, `go depth`, `go nodes` and `go infinite` get, and it is the
/// licence not to read the clock at all. A `go` that named a clock always
/// gets a budget, including one that named the *other* side's clock and not
/// ours: see [`Limits::is_clocked`] for why those are not the same thing.
#[must_use]
pub fn budget(limits: &Limits, us: Colour) -> Option<Budget> {
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
    let hard = (soft * 3).min(cap);
    Some(Budget { soft, hard })
}

/// Whether to start another iteration, given the elapsed milliseconds at the
/// end of each completed one and the budget.
///
/// **What this is for.** Starting an iteration on "the last one finished
/// before the soft budget" and abandoning it at the hard budget leaves a
/// window in which an iteration is always started and never finished: the
/// search spends up to two and a half times the soft budget and returns the
/// move it already had. The window is not a badly chosen fraction. With an
/// effective branching factor of EBF, iteration times are geometric, so the
/// cumulative time after each iteration is EBF times the one before it, and
/// in log-space the window is `ln(EBF / 3) / ln(EBF)` of all moves -- 39% at
/// EBF 6 and 47% at EBF 8. No time control and no tunable fraction appears
/// in that expression. Moving the soft fraction moves which clocks land in
/// the window; it cannot close it, because the window is the hard-to-soft
/// ratio of three measured against a branching factor larger than three.
/// Only a different rule closes it, and this is the rule.
///
/// **The window is closed on the tree as it stands, and this rule is kept
/// deliberately, not by oversight.** Null-move pruning brought the
/// iteration ratios under three, so the expression above says no clock
/// lands in the window and the prediction here almost always answers
/// "start it". That makes the rule nearly inert, and it stays: it re-arms
/// wherever the ladder steepens past three again, on a position or a
/// machine this reading was not taken on, and removing it would be a
/// behavioural change needing its own test. The gate in `tests/time.rs`
/// makes the same move, verifying the closure instead of demonstrating
/// the waste.
///
/// **The prediction is free.** Cumulative iteration times are geometric in
/// EBF, so the elapsed time carries its own estimate of it, and the next
/// iteration is predicted to end at EBF times the elapsed time. It is
/// started only if that lands inside the hard budget.
///
/// **EBF is read over two iterations and not one, which is the one thing
/// here a measurement decided.** Alpha-beta iteration times alternate: an
/// iteration whose parity suits the ordering the last one left is cheap and
/// the next is dear. A one-step ratio therefore samples half a period and
/// reads the alternation as the trend. Measured on the middlegame position
/// `tests/time.rs` gates this with, the costs run 2, 7, 37, 107, 617 ms and
/// the one-step ratios 3.5, 5.3, 2.9, 5.8: at the step that matters the
/// one-step estimate reads 2.9 against a true 5.8, predicts the iteration
/// will fit, and starts it. Two steps span a full period, and the elapsed
/// times two apart are in the ratio EBF squared.
///
/// **Three further things settled here rather than inside a test.**
///
/// - **The rule is inert where the hard budget equals the soft one**, which
///   is what `movetime` gives. There the time is not transferable: nothing
///   later gets what this move does not spend, so an abandoned iteration
///   costs nothing and refusing to start one only gives up the chance that
///   the prediction was wrong. The condition is read off the budget rather
///   than off the limits, so it is a property of what there is to save and
///   not a case named after a `go` token.
/// - **There is no safety factor on the prediction.** One would be a
///   tunable fraction, which is the thing this rule exists to not be.
/// - **Every way of not knowing answers "start it":** fewer than three
///   completed iterations, or one two back that was too fast to have taken
///   a millisecond. The search must always have a move, the early
///   iterations are the cheap ones, and a wrong "start it" is no worse than
///   the rule this replaces.
///
/// Integer throughout, like the rest of this module: no float reaches a
/// path that decides anything.
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
