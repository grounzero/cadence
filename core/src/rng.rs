// SPDX-License-Identifier: GPL-3.0-or-later

//! Feeds the Zobrist keys and self-play openings, so its first outputs are pinned by a test.

/// A bijection of a counter, so draws are distinct until it wraps.
#[must_use]
pub const fn splitmix64(state: u64) -> (u64, u64) {
    let state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31), state)
}

/// The same seed gives the same stream on every target.
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

    /// Bias under `n` in 2^64.
    ///
    /// # Panics
    ///
    /// If `n` is zero.
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
