// SPDX-License-Identifier: GPL-3.0-or-later

//! Time management: how much of the clock one move may use. A pure function of the `go` limits
//! and the side to move, in integer milliseconds, so that it can be tested as arithmetic
//! (`tests/time.rs`) and so that the search consults a clock only when this says there is one.

use cadence_core::Colour;

use crate::search::Limits;

/// Milliseconds held back from every budget for the cost of getting the move out: the pipe to
/// the GUI, thread scheduling, the GUI's own clock.
pub const MOVE_OVERHEAD_MS: u64 = 20;

/// A time budget for one move, in milliseconds from the start of the search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Do not start another iteration once this much has elapsed.
    pub soft: u64,
    /// Stop searching, mid-iteration if need be, once this much has elapsed.
    pub hard: u64,
}

/// The budget for `us` under `limits`, or `None` when nothing in `limits` constrains the time:
/// no `movetime`, and no clock named for either side. `movetime` takes precedence over a clock
/// when both are given.
#[must_use]
pub fn budget(limits: &Limits, us: Colour) -> Option<Budget> {
    if let Some(movetime) = limits.movetime {
        let t = movetime.saturating_sub(MOVE_OVERHEAD_MS);
        return Some(Budget { soft: t, hard: t });
    }
    if !limits.is_clocked() {
        return None;
    }
    // A clock was named and it was not ours. There is no information here about our own time,
    // and the safe reading of that is not "unlimited" but zero: soft and hard of zero, the
    // first iteration returned and no more.
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

/// Whether to start another iteration, given the elapsed milliseconds at the end of each
/// completed one and the budget. **EBF is read over two iterations and never one**, and the
/// obvious simplification to a single ratio is the bug this exists to not be.
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
    // Two iterations apart the elapsed times are in the ratio EBF squared. Scaled by a million
    // so the root comes back in thousandths, which is enough resolution to be exact at this
    // scale and keeps every step an integer one.
    let ebf_milli = (elapsed.saturating_mul(1_000_000) / two_back).isqrt();
    let predicted = elapsed.saturating_mul(ebf_milli) / 1_000;
    predicted <= budget.hard
}
