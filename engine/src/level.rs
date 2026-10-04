// SPDX-License-Identifier: GPL-3.0-or-later

//! Compiled in so a tune cannot change what the engine is. Integers only: `exp` on `f64` is not
//! reproducible across hosts.

/// Also the lowest number the option accepts.
pub const MIN_ELO: u32 = 1000;

/// Far enough below full strength that the top rung is still a level; full strength is the option's
/// absence.
pub const MAX_ELO: u32 = 1800;

/// `margin` is centipawns below the best line; `halving` is the deficit at which a candidate is
/// half as likely as the best.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub candidates: usize,
    pub margin: i32,
    pub halving: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rung {
    pub elo: u32,
    pub policy: Policy,
}

/// Chosen by a rule fixed before the reading. Monotone in expected centipawn loss, not in any one
/// column: the top rung samples two lines at a narrow margin, so it needs a flatter distribution to
/// lose less.
pub const LADDER: &[Rung] = &[
    Rung {
        elo: 1000,
        policy: Policy {
            candidates: 8,
            margin: 500,
            halving: 59,
        },
    },
    Rung {
        elo: 1200,
        policy: Policy {
            candidates: 8,
            margin: 500,
            halving: 34,
        },
    },
    Rung {
        elo: 1400,
        policy: Policy {
            candidates: 8,
            margin: 500,
            halving: 19,
        },
    },
    Rung {
        elo: 1600,
        policy: Policy {
            candidates: 8,
            margin: 500,
            halving: 8,
        },
    },
    Rung {
        elo: 1800,
        policy: Policy {
            candidates: 2,
            margin: 100,
            halving: 18,
        },
    },
];

/// A rating between rungs resolves down, so a number never buys strength it did not ask for.
#[must_use]
pub fn policy(elo: u32) -> Policy {
    let mut chosen = LADDER[0].policy;
    for rung in LADDER {
        if elo >= rung.elo {
            chosen = rung.policy;
        }
    }
    chosen
}

/// A weight is a table lookup and a shift, which keeps the choice free of floating point.
pub const ONE: u64 = 1 << 20;
const STEPS: i64 = 16;

/// Compiled in, because computing it is the float this module avoids.
const DECAY: [u64; 16] = [
    1048576, 1004120, 961548, 920782, 881744, 844361, 808563, 774282, 741455, 710020, 679917,
    651091, 623487, 597053, 571740, 547500,
];

/// Far enough down is zero rather than a rounding of it.
#[must_use]
pub fn weight(deficit: i32, halving: i32) -> u64 {
    let deficit = i64::from(deficit.max(0));
    let halving = i64::from(halving.max(1));
    let steps = deficit * STEPS / halving;
    let shift = steps / STEPS;
    // Past sixty-three halvings the shift is undefined and the weight zero anyway.
    if shift >= 63 {
        return 0;
    }
    #[allow(
        clippy::cast_sign_loss,
        reason = "both operands are non-negative by the clamps above"
    )]
    let fraction = (steps % STEPS) as usize;
    DECAY[fraction] >> shift
}

/// Turns one position into one number, never carrying state.
const fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The seed is the position, not the process, so a complaint about a move can be reproduced.
///
/// # Panics
///
/// If `deficits` is empty.
#[must_use]
pub fn choose(deficits: &[i32], halving: i32, seed: u64) -> usize {
    assert!(!deficits.is_empty(), "no candidates to choose among");
    let weights: Vec<u64> = deficits.iter().map(|d| weight(*d, halving)).collect();
    let total: u64 = weights.iter().sum();
    // Every candidate rounded to nothing; the best line is the honest answer.
    if total == 0 {
        return 0;
    }
    let mut ticket = mix(seed) % total;
    for (i, w) in weights.iter().enumerate() {
        if ticket < *w {
            return i;
        }
        ticket -= *w;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_spans_the_declared_range() {
        assert_eq!(LADDER.first().map(|r| r.elo), Some(MIN_ELO));
        assert_eq!(LADDER.last().map(|r| r.elo), Some(MAX_ELO));
    }

    #[test]
    fn a_value_between_rungs_resolves_down() {
        assert_eq!(policy(1199), policy(1000));
        assert_eq!(policy(1200), LADDER[1].policy);
    }

    #[test]
    fn a_value_under_the_ladder_takes_the_bottom_rung() {
        assert_eq!(policy(0), LADDER[0].policy);
        assert_eq!(policy(MIN_ELO - 1), LADDER[0].policy);
    }

    #[test]
    fn a_value_over_the_ladder_takes_the_top_rung() {
        assert_eq!(policy(u32::MAX), LADDER[LADDER.len() - 1].policy);
    }

    #[test]
    fn the_best_line_carries_full_weight() {
        assert_eq!(weight(0, 50), ONE);
    }

    #[test]
    fn one_halving_halves_the_weight() {
        for halving in [8, 19, 34, 59] {
            let half = weight(halving, halving);
            let want = ONE / 2;
            let slack = want / 1000;
            assert!(
                half.abs_diff(want) <= slack,
                "one halving of {halving} gave {half}, wanted about {want}"
            );
        }
    }

    #[test]
    fn a_far_enough_deficit_weighs_nothing() {
        assert_eq!(weight(100_000, 8), 0);
    }

    #[test]
    fn the_weight_never_rises_with_the_deficit() {
        let mut last = u64::MAX;
        for deficit in 0..2000 {
            let w = weight(deficit, 19);
            assert!(w <= last, "weight rose at a deficit of {deficit}");
            last = w;
        }
    }

    #[test]
    fn the_same_seed_chooses_the_same_candidate() {
        let deficits = [0, 20, 45, 90];
        let first = choose(&deficits, 19, 0x1234_5678_9ABC_DEF0);
        for _ in 0..8 {
            assert_eq!(choose(&deficits, 19, 0x1234_5678_9ABC_DEF0), first);
        }
    }

    #[test]
    fn a_sole_candidate_is_the_one_chosen() {
        assert_eq!(choose(&[0], 19, 7), 0);
    }

    #[test]
    fn the_best_line_is_the_answer_where_every_weight_rounds_away() {
        assert_eq!(choose(&[100_000, 100_001], 8, 99), 0);
    }

    #[test]
    fn the_choice_spreads_over_seeds_and_prefers_the_best() {
        let deficits = [0, 30, 120];
        let mut counts = [0u32; 3];
        for seed in 0..4000u64 {
            counts[choose(&deficits, 34, seed)] += 1;
        }
        assert!(
            counts.iter().all(|c| *c > 0),
            "a candidate was never chosen: {counts:?}"
        );
        assert!(
            counts[0] > counts[1] && counts[1] > counts[2],
            "the choice did not fall away with the deficit: {counts:?}"
        );
    }
}
