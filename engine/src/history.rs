// SPDX-License-Identifier: GPL-3.0-or-later

//! What a quiet move has been worth elsewhere in this search.
//!
//! A butterfly table: one signed score per side per `from`-`to` pair, 4,096
//! pairs and two sides, indexed by [`Move::from_to`]. It records nothing
//! about the position a move was played in, only about the move.
//!
//! **Ageing is a multiply toward zero, and it is integer.** [`apply`] has the
//! update and the equilibrium it settles to. It uses no float, which nothing
//! that decides a move may: the bench's node count has to be a function of
//! the code alone, and a float on this path would make it a function of the
//! rounding as well.
//!
//! **Lifetime: one search.** [`Search::run`](crate::search::Search::run)
//! clears it where it clears the killers, so the table carries across the
//! iterations of one `go` command and never across two. Carrying it across
//! the moves of a game needs an ageing step between them that this does not
//! have, and the fixed-position bench cannot tell the two apart.

use cadence_core::{Colour, Move};

/// The largest magnitude an entry can reach, and the scale everything else
/// here is expressed against.
///
/// Sixteen thousand three hundred and eighty four. Nothing measures this
/// number, and nothing needs to: [`BONUS_SCALE`] is expressed as a share of
/// it and so is [`HISTORY_PLY`], so halving it and halving the bonus would
/// leave every reduction and every sort exactly where they are. What it
/// fixes is the resolution the two share.
pub const HISTORY_MAX: i32 = 16_384;

/// The number of `from`-`to` pairs, which is [`Move::from_to`]'s range: six
/// bits of origin and six of destination.
pub const SPAN: usize = 1 << 12;

/// What a squared depth is multiplied by, and the reason the cap above is
/// not decoration: the scale is what makes the ageing term in [`apply`] carry
/// most of a bonus away at the top of the range. It changes nothing about the
/// order the sort reads, because it multiplies every entry alike.
const BONUS_SCALE: i64 = 16;

/// The deepest node whose bonus is not capped, which is where the scaled
/// square reaches [`HISTORY_MAX`]. A bonus at the cap moves an entry to the
/// cap in one act, which is a killer slot with extra steps.
const MAX_BONUS_DEPTH: u32 = 32;
const _: () =
    assert!(BONUS_SCALE * MAX_BONUS_DEPTH as i64 * MAX_BONUS_DEPTH as i64 == HISTORY_MAX as i64);

/// How much history moves a reduction by one ply. A sixteenth of the cap.
const HISTORY_PLY: i32 = HISTORY_MAX / 16;

/// The most plies history may move a reduction, either way.
///
/// Two, against a base reduction that runs from one to six plies over the
/// region the search visits. One would make the modulation invisible at
/// the deep nodes where the base is largest; more than two would let the
/// score overrule the index rather than adjust it, and the index is the
/// reading with a landed SPRT behind it.
pub const SHIFT_MAX: i32 = 2;

/// The bonus a cutoff at `depth` is worth: the depth squared and scaled by
/// [`BONUS_SCALE`], capped at [`HISTORY_MAX`].
///
/// Squared rather than linear because a cutoff deeper in the tree is
/// evidence about a larger subtree: it stood up to more search. The cap
/// is what makes this total; nothing in the search reaches
/// [`MAX_BONUS_DEPTH`], and a function that is only correct for the depths
/// something happens to pass it is a function a gate cannot pin.
#[must_use]
pub fn bonus(depth: u32) -> i32 {
    let d = i64::from(depth.min(MAX_BONUS_DEPTH));
    // The `min` above holds the product at `HISTORY_MAX`, so the
    // conversion cannot fail and the fallback is the value it would have
    // produced anyway.
    i32::try_from(d * d * BONUS_SCALE).unwrap_or(HISTORY_MAX)
}

/// `entry` after `bonus` is applied to it: the bonus, less the share of the
/// entry the bonus's own size claims.
///
/// The two terms are what make it an ageing update rather than an
/// accumulator. Near zero the entry moves by the whole bonus; near the cap
/// the subtraction cancels almost all of it, so an entry approaches
/// [`HISTORY_MAX`] and does not pass it. A move that cuts a fraction `p` of
/// the times it is judged settles near `HISTORY_MAX * (2p - 1)` whatever
/// the bonuses were, so the equilibrium reads how reliable the move is and
/// not how often it has been looked at, which is the property a rank over
/// one node's list cannot supply.
///
/// Total: both arguments are clamped into the table's range first, so no
/// product overflows and no caller can drive an entry out of the bound the
/// reader below rests on. The final clamp is worth one instruction and
/// closes the one-off the truncating division leaves.
#[must_use]
pub fn apply(entry: i32, bonus: i32) -> i32 {
    let e = entry.clamp(-HISTORY_MAX, HISTORY_MAX);
    let b = bonus.clamp(-HISTORY_MAX, HISTORY_MAX);
    (e + b - e * b.abs() / HISTORY_MAX).clamp(-HISTORY_MAX, HISTORY_MAX)
}

/// How many plies `history` moves a late move's reduction: positive shortens
/// it, negative lengthens it, and the caller subtracts.
///
/// A total function of one argument, gateable without a search, for the
/// same reason [`crate::search::lmr_reduction`] is.
#[must_use]
pub fn shift(history: i32) -> i32 {
    (history / HISTORY_PLY).clamp(-SHIFT_MAX, SHIFT_MAX)
}

/// The table: one score per side per `from`-`to` pair.
///
/// Thirty-two kibibytes, on the heap rather than inline like `killers` and
/// `evals`, for the reason `PvTable`'s rows are: `Search` is built and
/// returned by value, and a struct that large moving through a return is a
/// cost with nothing to show for it.
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

    /// Forget everything. Called where the killers are cleared, so the two
    /// have one lifetime and neither can quietly acquire another.
    pub fn clear(&mut self) {
        self.rows.fill(0);
    }

    /// `side`'s whole row, which is what `picker` ranks a quiet move by.
    /// A caller with no table to offer passes an empty slice, and the sort
    /// reads a zero for every move, which is the order it had before this
    /// table existed.
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
