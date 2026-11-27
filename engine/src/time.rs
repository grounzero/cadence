// SPDX-License-Identifier: GPL-3.0-or-later

//! A pure function of the `go` limits and the side to move, so it is testable as arithmetic and the
//! search reads a clock only when this says there is one.

use cadence_core::Colour;

use crate::score::{Score, is_decisive};
use crate::search::Limits;

/// For the pipe, thread scheduling and the GUI's own clock; `Move Overhead` replaces it.
pub const MOVE_OVERHEAD_MS: u64 = 5;

/// The move a clock with no increment is kept for, some way past the longest of the engine's own
/// online games without one. Every move before it keeps back the lag of the moves still to it.
pub const MOVES_LEFT_HORIZON: u64 = 150;

/// The fewest moves kept for, so a game past the horizon still keeps back for its end.
pub const MOVES_LEFT_FLOOR: u64 = 60;

/// The most one move's reading of the lag counts for, so a single stall does not empty the clock.
pub const MAX_LAG_READING_MS: u64 = 500;

/// Moves from `move_number` to the horizon, never under the floor; an unknown move number is the
/// first.
#[must_use]
pub fn moves_left(move_number: Option<u32>) -> u64 {
    let number = u64::from(move_number.unwrap_or(1));
    MOVES_LEFT_HORIZON
        .saturating_sub(number)
        .max(MOVES_LEFT_FLOOR)
}

/// The lag with one more move's reading folded in, a quarter of the way to it; the first reading
/// is taken as it is.
#[must_use]
pub fn fold_lag(lag: Option<u64>, reading: u64) -> u64 {
    let reading = reading.min(MAX_LAG_READING_MS);
    match lag {
        None => reading,
        Some(lag) => (lag * 3 + reading) / 4,
    }
}

/// Milliseconds from the start of the search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Start no iteration after this.
    pub soft: u64,
    /// Stop, mid-iteration if need be.
    pub hard: u64,
}

impl Budget {
    /// Both deadlines later by `offset`, the time pondered before a hit.
    #[must_use]
    pub fn moved(self, offset: u64) -> Budget {
        Budget {
            soft: self.soft.saturating_add(offset),
            hard: self.hard.saturating_add(offset),
        }
    }
}

/// `None` with no `movetime` and no clock for either side; `movetime` wins over a clock.
#[must_use]
pub fn budget(limits: &Limits, us: Colour) -> Option<Budget> {
    // A ponder answers on `stop` or on the budget a `ponderhit` brings, never on this one.
    if limits.ponder {
        return None;
    }
    let overhead = limits.move_overhead.unwrap_or(MOVE_OVERHEAD_MS);
    if let Some(movetime) = limits.movetime {
        let t = movetime.saturating_sub(overhead);
        return Some(Budget { soft: t, hard: t });
    }
    if !limits.is_clocked() {
        return None;
    }
    // A clock that is not ours tells nothing of our time, and the safe reading is zero: the first
    // iteration and no more.
    let (time, inc) = limits.clock(us).unwrap_or((0, 0));
    // With no increment and no count of moves nothing comes back, and every move left loses the
    // lag again, so that much is kept back before the share is taken.
    let reserve = if inc == 0 && limits.movestogo.is_none() {
        moves_left(limits.move_number).saturating_mul(limits.lag.unwrap_or(0))
    } else {
        0
    };
    let avail = time.saturating_sub(overhead).saturating_sub(reserve);
    let cap = avail / 2;
    let share = match limits.movestogo {
        Some(mtg) => avail / u64::from(mtg.max(1)),
        None => avail / 25,
    };
    // Every move pays the overhead, so only the increment past it is new time. Saturating, so a
    // clock or increment near the limit bounds the budget rather than overflowing.
    let gained = inc.saturating_sub(overhead);
    let soft = share.saturating_add(gained.saturating_mul(3) / 4).min(cap);
    let hard = soft.saturating_mul(3).min(cap);
    Some(Budget { soft, hard })
}

/// What a `ponderhit` brings: the same limits' budget without the ponder flag, and `None` for a
/// search that is not pondering.
#[must_use]
pub fn hit_budget(limits: &Limits, us: Colour) -> Option<Budget> {
    if !limits.ponder {
        return None;
    }
    let clocked = Limits {
        ponder: false,
        ..*limits
    };
    budget(&clocked, us)
}

/// Thousandths of the soft budget by the run of completed iterations on one move, in bins of 1, 2,
/// 3 to 4, 5 to 8 and 9 or more. Each is proportional to how often one more iteration changed the
/// move in self-play at that run, the first two bins pooled.
pub const STABILITY_SCALE: [u64; 5] = [1936, 1936, 1704, 1590, 738];

/// How many calls of a self-play reading stopped between iterations in each bin, the weights the
/// table averages one over.
pub const STABILITY_CALLS: [u64; 5] = [1221, 289, 398, 895, 8467];

// Within half a thousandth, the table's rounding, so an edit to one value cannot move the average
// spend unnoticed.
const _: () = {
    let (mut spent, mut calls, mut i) = (0, 0, 0);
    while i < STABILITY_SCALE.len() {
        spent += STABILITY_SCALE[i] * STABILITY_CALLS[i];
        calls += STABILITY_CALLS[i];
        i += 1;
    }
    assert!(spent.abs_diff(1000 * calls) * 2 <= calls);
};

/// A run of zero, no completed iteration, falls in the first bin.
fn stability_bin(run: usize) -> usize {
    match run {
        0..=1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        _ => 4,
    }
}

/// The soft budget for a run of `run` completed iterations on one move, never past `hard`. A
/// `movetime` means exactly that time, so its budget is not scaled.
#[must_use]
pub fn soft_for_run(limits: &Limits, budget: Budget, run: usize) -> u64 {
    if limits.movetime.is_some() {
        return budget.soft;
    }
    let scale = STABILITY_SCALE[stability_bin(run)];
    (budget.soft.saturating_mul(scale) / 1000).min(budget.hard)
}

/// How many completed iterations before the last one its score is compared with.
pub const DROP_WINDOW: usize = 4;

/// Thousandths of the soft budget added for each centipawn the score has fallen.
pub const DROP_STEP: u64 = 5;

/// The most a falling score can stretch the soft budget, in thousandths; the hard budget still
/// bounds it.
pub const DROP_SCALE_MAX: u64 = 1600;

/// Thousandths of the soft budget for a search whose iteration scores are `scores`, oldest first.
/// Only the last `DROP_WINDOW + 1` are read, so a caller may pass just those.
#[must_use]
pub fn drop_scale(scores: &[Score]) -> u64 {
    let Some((&last, before)) = scores.split_last() else {
        return 1000;
    };
    let window = &before[before.len().saturating_sub(DROP_WINDOW)..];
    if window.is_empty() || is_decisive(last) || window.iter().any(|&s| is_decisive(s)) {
        return 1000;
    }
    let sum: i64 = window.iter().map(|&s| i64::from(s)).sum();
    let reference = sum / i64::try_from(window.len()).unwrap_or(1);
    let fall = u64::try_from(reference - i64::from(last)).unwrap_or(0);
    let stretch = fall.saturating_mul(DROP_STEP);
    stretch.saturating_add(1000).min(DROP_SCALE_MAX)
}

/// The soft budget stretched by `scale` thousandths, never past `hard`. A `movetime` means exactly
/// that time, so it is not stretched.
#[must_use]
pub fn soft_for_drop(limits: &Limits, budget: Budget, soft: u64, scale: u64) -> u64 {
    if limits.movetime.is_some() {
        return soft;
    }
    let stretched = soft.saturating_mul(scale) / 1000;
    stretched.clamp(soft.min(budget.hard), budget.hard)
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

/// Whether a search whose last completed iteration ends `completed` starts another, the soft budget
/// stretched by `scale` thousandths and both deadlines moved by `offset`. A hit passes the time
/// pondered and every other search zero, so a hit decides as a fresh search with the same times.
#[must_use]
pub fn starts_another(
    limits: &Limits,
    budget: Budget,
    completed: &[u64],
    run: usize,
    scale: u64,
    offset: u64,
) -> bool {
    let Some(&elapsed) = completed.last() else {
        return true;
    };
    let soft = soft_for_drop(limits, budget, soft_for_run(limits, budget, run), scale);
    let soft = soft.saturating_add(offset);
    elapsed < soft && another_iteration_fits(completed, budget.moved(offset))
}

#[cfg(kani)]
mod proofs {
    use super::*;

    fn limits() -> Limits {
        Limits {
            movetime: kani::any(),
            ponder: kani::any(),
            time: [kani::any(), kani::any()],
            inc: [kani::any(), kani::any()],
            movestogo: kani::any(),
            move_overhead: kani::any(),
            lag: kani::any(),
            move_number: kani::any(),
            ..Limits::default()
        }
    }

    fn side() -> Colour {
        if kani::any() {
            Colour::White
        } else {
            Colour::Black
        }
    }

    /// A ponder gets no budget, and once a hit clears it, soft <= hard <= the clock less overhead.
    #[kani::proof]
    #[kani::unwind(6)]
    fn a_budget_stays_inside_the_clock() {
        let (limits, us) = (limits(), side());
        if limits.ponder {
            assert!(budget(&limits, us).is_none());
        }
        let hit = Limits {
            ponder: false,
            ..limits
        };
        let overhead = hit.move_overhead.unwrap_or(MOVE_OVERHEAD_MS);
        if let Some(b) = budget(&hit, us) {
            assert!(b.soft <= b.hard);
            match hit.movetime {
                Some(t) => assert!(b.hard == t.saturating_sub(overhead)),
                None => {
                    let time = hit.time[us.index()].unwrap_or(0);
                    assert!(b.hard <= time.saturating_sub(overhead));
                }
            }
        }
    }

    /// The hit budget `Clock::begin` takes is none unless pondering, and else inside the clock.
    #[kani::proof]
    #[kani::unwind(6)]
    fn the_hit_budget_stays_inside_the_clock() {
        let (limits, us) = (limits(), side());
        let hit = hit_budget(&limits, us);
        assert!(limits.ponder || hit.is_none());
        if let Some(b) = hit {
            let overhead = limits.move_overhead.unwrap_or(MOVE_OVERHEAD_MS);
            assert!(b.soft <= b.hard);
            match limits.movetime {
                Some(t) => assert!(b.hard == t.saturating_sub(overhead)),
                None => {
                    let time = limits.time[us.index()].unwrap_or(0);
                    assert!(b.hard <= time.saturating_sub(overhead));
                }
            }
        }
    }

    /// The run-scaled soft budget never passes the hard one.
    #[kani::proof]
    fn the_run_scale_stays_under_the_hard_budget() {
        let b = Budget {
            soft: kani::any(),
            hard: kani::any(),
        };
        kani::assume(b.soft <= b.hard);
        assert!(soft_for_run(&limits(), b, kani::any()) <= b.hard);
    }
}
