// SPDX-License-Identifier: GPL-3.0-or-later

//! What a quiet move has been worth elsewhere in this search. A butterfly table: one signed
//! score per side per `from`-`to` pair, 4,096 pairs and two sides, indexed by
//! [`Move::from_to`].

use cadence_core::{Colour, Move};

/// The largest magnitude an entry can reach, and the scale everything else here is expressed
/// against. Sixteen thousand three hundred and eighty four.
pub const HISTORY_MAX: i32 = 16_384;

/// The number of `from`-`to` pairs, which is [`Move::from_to`]'s range: six bits of origin and
/// six of destination.
pub const SPAN: usize = 1 << 12;

/// What a squared depth is multiplied by, and the reason the cap above is not decoration: the
/// scale is what makes the ageing term in [`apply`] carry most of a bonus away at the top of
/// the range. It changes nothing about the order the sort reads, because it multiplies every
/// entry alike.
const BONUS_SCALE: i64 = 16;

/// The deepest node whose bonus is not capped, which is where the scaled square reaches
/// [`HISTORY_MAX`]. A bonus at the cap moves an entry to the cap in one act, which is a killer
/// slot with extra steps.
const MAX_BONUS_DEPTH: u32 = 32;
const _: () =
    assert!(BONUS_SCALE * MAX_BONUS_DEPTH as i64 * MAX_BONUS_DEPTH as i64 == HISTORY_MAX as i64);

/// How much history moves a reduction by one ply. A sixteenth of the cap.
const HISTORY_PLY: i32 = HISTORY_MAX / 16;

/// The most plies history may move a reduction, either way. Two, against a base reduction that
/// runs from one to six plies over the region the search visits.
pub const SHIFT_MAX: i32 = 2;

/// The bonus a cutoff at `depth` is worth: the depth squared and scaled by [`BONUS_SCALE`],
/// capped at [`HISTORY_MAX`]. Squared rather than linear because a cutoff deeper in the tree is
/// evidence about a larger subtree: it stood up to more search.
#[must_use]
pub fn bonus(depth: u32) -> i32 {
    let d = i64::from(depth.min(MAX_BONUS_DEPTH));
    // The `min` above holds the product at `HISTORY_MAX`, so the conversion cannot fail and the
    // fallback is the value it would have produced anyway.
    i32::try_from(d * d * BONUS_SCALE).unwrap_or(HISTORY_MAX)
}

/// `entry` after `bonus` is applied to it: the bonus, less the share of the entry the bonus's
/// own size claims. The two terms are what make it an ageing update rather than an accumulator.
#[must_use]
pub fn apply(entry: i32, bonus: i32) -> i32 {
    let e = entry.clamp(-HISTORY_MAX, HISTORY_MAX);
    let b = bonus.clamp(-HISTORY_MAX, HISTORY_MAX);
    (e + b - e * b.abs() / HISTORY_MAX).clamp(-HISTORY_MAX, HISTORY_MAX)
}

/// How many plies `history` moves a late move's reduction: positive shortens it, negative
/// lengthens it, and the caller subtracts. A total function of one argument, gateable without a
/// search, for the same reason [`crate::search::lmr_reduction`] is.
#[must_use]
pub fn shift(history: i32) -> i32 {
    (history / HISTORY_PLY).clamp(-SHIFT_MAX, SHIFT_MAX)
}

/// The table: one score per side per `from`-`to` pair. Thirty-two kibibytes, on the heap rather
/// than inline like `killers` and `evals`, for the reason `PvTable`'s rows are: `Search` is
/// built and returned by value, and a struct that large moving through a return is a cost with
/// nothing to show for it.
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

    /// Forget everything. Called where the killers are cleared, so the two have one lifetime
    /// and neither can quietly acquire another.
    pub fn clear(&mut self) {
        self.rows.fill(0);
    }

    /// `side`'s whole row, which is what `picker` ranks a quiet move by. A caller with no table
    /// to offer passes an empty slice, and the sort reads a zero for every move, which is the
    /// order it had before this table existed.
    #[must_use]
    pub fn side(&self, side: Colour) -> &[i32] {
        let start = side.index() * SPAN;
        &self.rows[start..start + SPAN]
    }

    /// What `side` has been getting out of `m`.
    #[must_use]
    pub fn get(&self, side: Colour, m: Move) -> i32 {
        self.rows[side.index() * SPAN + m.from_to()]
    }

    /// Credit or debit `m` for `side` by `bonus`, through [`apply`].
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
