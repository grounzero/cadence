// SPDX-License-Identifier: GPL-3.0-or-later

//! A pure function of the `go` limits and the side to move, so it is testable as arithmetic and the
//! search reads a clock only when this says there is one.

use cadence_core::Colour;

use crate::search::Limits;

/// For the pipe, thread scheduling and the GUI's own clock.
pub const MOVE_OVERHEAD_MS: u64 = 20;

/// Milliseconds from the start of the search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Start no iteration after this.
    pub soft: u64,
    /// Stop, mid-iteration if need be.
    pub hard: u64,
}

/// `None` with no `movetime` and no clock for either side; `movetime` wins over a clock.
#[must_use]
pub fn budget(limits: &Limits, us: Colour) -> Option<Budget> {
    // A ponder answers on `stop` or on the budget a `ponderhit` brings, never on this one.
    if limits.ponder {
        return None;
    }
    if let Some(movetime) = limits.movetime {
        let t = movetime.saturating_sub(MOVE_OVERHEAD_MS);
        return Some(Budget { soft: t, hard: t });
    }
    if !limits.is_clocked() {
        return None;
    }
    // A clock that is not ours tells nothing of our time, and the safe reading is zero: the first
    // iteration and no more.
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

/// EBF is read over two iterations and never one; the single-ratio simplification is the bug this
/// exists to not be.
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
    // Two iterations apart the times are in the ratio EBF squared; scaled by a million so the root
    // comes back in thousandths, all integer.
    let ebf_milli = (elapsed.saturating_mul(1_000_000) / two_back).isqrt();
    let predicted = elapsed.saturating_mul(ebf_milli) / 1_000;
    predicted <= budget.hard
}
