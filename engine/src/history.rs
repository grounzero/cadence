// SPDX-License-Identifier: GPL-3.0-or-later

//! A butterfly table: one signed score per side per `from`-`to` pair.

use cadence_core::{Colour, Move};

/// The scale everything else here is expressed against.
pub const HISTORY_MAX: i32 = 16_384;

/// [`Move::from_to`]'s range.
pub const SPAN: usize = 1 << 12;

/// What makes the ageing term in [`apply`] carry most of a bonus away near the cap. It leaves the
/// sort's order unchanged, multiplying every entry alike.
const BONUS_SCALE: i64 = 16;

/// Where the scaled square reaches [`HISTORY_MAX`]; a bonus at the cap moves an entry to the cap in
/// one act, a killer slot with extra steps.
const MAX_BONUS_DEPTH: u32 = 32;
const _: () =
    assert!(BONUS_SCALE * MAX_BONUS_DEPTH as i64 * MAX_BONUS_DEPTH as i64 == HISTORY_MAX as i64);

const HISTORY_PLY: i32 = HISTORY_MAX / 16;

/// Against a base reduction of one to six plies where the search visits.
pub const SHIFT_MAX: i32 = 2;

/// Squared because a deeper cutoff stood up to more search.
#[must_use]
pub fn bonus(depth: u32) -> i32 {
    let d = i64::from(depth.min(MAX_BONUS_DEPTH));
    // The `min` above holds the product at `HISTORY_MAX`, so the fallback is never taken.
    i32::try_from(d * d * BONUS_SCALE).unwrap_or(HISTORY_MAX)
}

/// The bonus less the share of the entry its size claims: an ageing update, not an accumulator.
#[must_use]
pub fn apply(entry: i32, bonus: i32) -> i32 {
    let e = entry.clamp(-HISTORY_MAX, HISTORY_MAX);
    let b = bonus.clamp(-HISTORY_MAX, HISTORY_MAX);
    (e + b - e * b.abs() / HISTORY_MAX).clamp(-HISTORY_MAX, HISTORY_MAX)
}

/// Positive shortens the reduction; the caller subtracts. Total, so gateable without a search.
#[must_use]
pub fn shift(history: i32) -> i32 {
    (history / HISTORY_PLY).clamp(-SHIFT_MAX, SHIFT_MAX)
}

/// Thirty-two kibibytes, on the heap: `Search` is returned by value.
pub struct History {
    rows: Box<[i32]>,
}

impl History {
    #[must_use]
    pub fn new() -> History {
        History {
            rows: vec![0; 2 * SPAN].into_boxed_slice(),
        }
    }

    /// Called where the killers are cleared, so the two share one lifetime.
    pub fn clear(&mut self) {
        self.rows.fill(0);
    }

    /// A caller with no table passes an empty slice, and every move reads zero.
    #[must_use]
    pub fn side(&self, side: Colour) -> &[i32] {
        let start = side.index() * SPAN;
        &self.rows[start..start + SPAN]
    }

    #[must_use]
    pub fn get(&self, side: Colour, m: Move) -> i32 {
        self.rows[side.index() * SPAN + m.from_to()]
    }

    pub fn update(&mut self, side: Colour, m: Move, bonus: i32) {
        let i = side.index() * SPAN + m.from_to();
        self.rows[i] = apply(self.rows[i], bonus);
    }
}

impl Default for History {
    fn default() -> History {
        History::new()
    }
}
