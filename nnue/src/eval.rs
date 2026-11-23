// SPDX-License-Identifier: GPL-3.0-or-later

//! The output layer: each activation clamped to [0, 255] and squared, weighted, summed and
//! scaled to centipawns from the side to move's view.

use cadence_core::Colour;

use crate::accumulator::Accumulator;
use crate::net::{Network, QA, QB, SCALE};

impl<const H: usize> Network<H> {
    /// The fast form, multiplying the clamped activation by its weight in `i16` and widening only
    /// the second product, which the load check keeps exact; the two perspectives add in `i64`.
    #[must_use]
    pub fn evaluate(&self, acc: &Accumulator<H>, stm: Colour) -> i32 {
        let us = &acc.sums[stm.index()];
        let them = &acc.sums[stm.flip().index()];
        let sum = i64::from(layer(us, &self.output[0])) + i64::from(layer(them, &self.output[1]));
        let value = round_div(
            (sum / i64::from(QA) + i64::from(self.output_bias)) * i64::from(SCALE),
            i64::from(QA * QB),
        );
        i32::try_from(value).unwrap_or_else(|_| unreachable!("the load check bounds the sum"))
    }

    /// The same value by the plainest arithmetic, in `i64` throughout: the oracle the fast form
    /// and every target's build must equal.
    #[must_use]
    pub fn evaluate_reference(&self, acc: &Accumulator<H>, stm: Colour) -> i32 {
        let mut sum = 0_i64;
        for (sums, weights) in [
            (&acc.sums[stm.index()], &self.output[0]),
            (&acc.sums[stm.flip().index()], &self.output[1]),
        ] {
            for (&s, &w) in sums.iter().zip(weights) {
                let c = i64::from(s).clamp(0, i64::from(QA));
                sum += c * c * i64::from(w);
            }
        }
        let value = round_div(
            (sum / i64::from(QA) + i64::from(self.output_bias)) * i64::from(SCALE),
            i64::from(QA * QB),
        );
        i32::try_from(value).unwrap_or_else(|_| unreachable!("the load check bounds the sum"))
    }
}

/// To the nearest whole, halves away from zero, so a score and its negation round alike; `d` is
/// positive.
fn round_div(n: i64, d: i64) -> i64 {
    let half = d / 2;
    if n >= 0 {
        (n + half) / d
    } else {
        (n - half) / d
    }
}

/// Lanes summed side by side, each in its own `i32`, so the compiler keeps them in vector
/// registers; a whole number of chunks at either width, and any remainder is summed after.
const LANES: usize = 32;

/// The load check bounds each perspective's sum inside `i32` at either width, which at the wide one
/// holds only for output weights of magnitude 64 or less. Integer addition is exact in any order, so
/// the lanes give the sum a single running total would.
fn layer<const H: usize>(sums: &[i16; H], weights: &[i16; H]) -> i32 {
    let ceiling = i16::try_from(QA).unwrap_or(i16::MAX);
    let term = |s: i16, w: i16| {
        let c = s.clamp(0, ceiling);
        i32::from(c * w) * i32::from(c)
    };
    let (sum_chunks, sum_rest) = sums.as_chunks::<LANES>();
    let (weight_chunks, weight_rest) = weights.as_chunks::<LANES>();
    let mut lanes = [0_i32; LANES];
    for (s, w) in sum_chunks.iter().zip(weight_chunks) {
        for ((lane, &s), &w) in lanes.iter_mut().zip(s).zip(w) {
            *lane += term(s, w);
        }
    }
    let rest: i32 = sum_rest
        .iter()
        .zip(weight_rest)
        .map(|(&s, &w)| term(s, w))
        .sum();
    lanes.iter().sum::<i32>() + rest
}

#[cfg(test)]
mod tests {
    use super::round_div;

    /// Halves go away from zero on both signs, and everything short of a half goes toward it.
    #[test]
    fn the_final_division_rounds_halves_away_from_zero() {
        let d = 16_320;
        for (n, want) in [
            (0, 0),
            (8_159, 0),
            (8_160, 1),
            (-8_159, 0),
            (-8_160, -1),
            (3 * d + 8_159, 3),
            (3 * d + 8_160, 4),
            (-3 * d - 8_160, -4),
        ] {
            assert_eq!(round_div(n, d), want, "{n} / {d}");
            assert_eq!(round_div(-n, d), -want, "-{n} / {d}");
        }
    }
}
