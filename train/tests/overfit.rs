// SPDX-License-Identifier: GPL-3.0-or-later

//! The overfit test's pieces that need no device: the synthetic set, and each bar passing and
//! failing on planted outputs and losses.

use std::collections::BTreeSet;

use cadence_train::overfit::{
    BLEND_GAP, MEMORISED, SYNTHETIC_POSITIONS, constant_loss, real_reading, synthetic,
    synthetic_bar,
};
use cadence_train::validate::{Example, example};

const SHARE: f32 = 0.75;
const SCALE: f32 = 400.0;

/// The target at `share`, written out here so the bars are checked against an oracle of their own.
fn blended(e: &Example, share: f64) -> f64 {
    let sigmoid = 1.0 / (1.0 + (-f64::from(e.score) / f64::from(SCALE)).exp());
    share * f64::from(e.result) / 2.0 + (1.0 - share) * sigmoid
}

fn logit(p: f64) -> f32 {
    (p / (1.0 - p)).ln() as f32
}

fn examples() -> Vec<Example> {
    synthetic(1)
        .iter()
        .map(|r| example(r).expect("a record decodes"))
        .collect()
}

#[test]
fn the_synthetic_set_is_distinct_inputs_drawn_the_same_each_time() {
    let records = synthetic(1);
    assert_eq!(records.len(), SYNTHETIC_POSITIONS);
    assert_eq!(records, synthetic(1));
    let inputs: BTreeSet<&[u8]> = records.iter().map(|r| &r[..24]).collect();
    assert_eq!(inputs.len(), SYNTHETIC_POSITIONS, "an input repeats");
    let results: BTreeSet<u8> = examples().iter().map(|e| e.result).collect();
    assert_eq!(results.len(), 3, "a result never drawn");
}

/// Memorised within `MEMORISED`, outputs at the stated share sit farther than `BLEND_GAP` from the
/// swapped reading on average, so the gap tells the two meanings apart.
#[test]
fn the_two_readings_are_far_enough_apart_to_tell_apart() {
    let set = examples();
    assert_eq!(set.len(), SYNTHETIC_POSITIONS);
    let apart: f64 = set
        .iter()
        .map(|e| (blended(e, 0.75) - blended(e, 0.25)).abs())
        .sum::<f64>()
        / f64::from(u32::try_from(SYNTHETIC_POSITIONS).expect("a count"));
    assert!(
        apart > BLEND_GAP + MEMORISED,
        "the readings sit {apart} apart"
    );
}

#[test]
fn outputs_at_the_stated_reading_pass_and_at_the_swapped_reading_fail() {
    let set = examples();
    let stated: Vec<f32> = set.iter().map(|e| logit(blended(e, 0.75))).collect();
    let swapped: Vec<f32> = set.iter().map(|e| logit(blended(e, 0.25))).collect();
    let reading = synthetic_bar(&stated, &set, SHARE, SCALE).expect("the stated reading passes");
    assert!(reading.worst < 1e-4, "{reading:?}");
    let refused = synthetic_bar(&swapped, &set, SHARE, SCALE);
    assert!(refused.is_err(), "the swapped reading passed");
}

/// The real part is a reading and not a bar: the trained loss, the best constant guess's and
/// their ratio, said to be a reading.
#[test]
fn the_real_part_reads_the_trained_loss_against_the_constant_guess() {
    let reading = real_reading(0.06, 0.1);
    assert!(reading.contains("0.060000"), "{reading}");
    assert!(reading.contains("0.100000"), "{reading}");
    assert!(reading.contains("ratio 0.600"), "{reading}");
    assert!(reading.contains("a reading, not a bar"), "{reading}");
}

/// With the result alone, three results' targets are 0, 1/2 and 1, whose mean squared deviation
/// is 1/6.
#[test]
fn the_constant_guess_loses_its_targets_variance() {
    let at = |result: u8| Example {
        fen: "4k3/8/8/8/8/8/8/4K3 w - - 0 1".to_string(),
        score: 300,
        result,
    };
    let loss = constant_loss(&[at(0), at(1), at(2)], 1.0, SCALE);
    assert!((loss - 1.0 / 6.0).abs() < 1e-12, "{loss}");
}
