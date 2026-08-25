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
