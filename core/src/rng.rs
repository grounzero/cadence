// SPDX-License-Identifier: GPL-3.0-or-later

//! splitmix64, as one step and as a seeded stream: the Zobrist keys are drawn from it at compile
//! time and self-play draws its openings from it. Changing either output changes every key and
//! every generated game, which is why the first outputs of a fixed seed are pinned by a test.

/// One draw: the output and the state after it. Its output is a bijection of a counter, so
/// distinct draws are distinct until the counter wraps at 2^64.
#[must_use]
pub const fn splitmix64(state: u64) -> (u64, u64) {
    let state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31), state)
}

/// A seeded stream of splitmix64 draws. The same seed gives the same stream on every build and
/// every target, which is what a reproducible dataset rests on.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    #[must_use]
    pub const fn new(seed: u64) -> Rng {
        Rng { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        let (out, state) = splitmix64(self.state);
        self.state = state;
        out
    }

    /// A draw in `0..n`, by the high half of a widening multiply. Its bias is under `n` in 2^64,
    /// which no count this is used for can see.
    ///
    /// # Panics
    ///
    /// If `n` is zero. There is no draw from an empty range.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the high half of a product with a usize is below that usize"
    )]
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        let wide = u128::from(self.next_u64()) * n as u128;
        (wide >> 64) as usize
    }
}
