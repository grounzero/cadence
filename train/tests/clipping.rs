// SPDX-License-Identifier: GPL-3.0-or-later

//! The pinned trainer's weight clip against the engine's loader: it keeps each weight, each
//! accumulator lane and each perspective's output sum inside the loader's bounds.

use bullet_lib::nn::optimiser::AdamWParams;
use cadence_nnue::HIDDEN;
use cadence_nnue::net::{QA, QB};

/// Active features a lane sums at most, one a piece on the board.
const MAX_ACTIVE: i64 = 32;

#[expect(
    clippy::cast_precision_loss,
    reason = "the scales are small powers and near them"
)]
fn quantised(weight: f32, scale: i32) -> i64 {
    (weight * scale as f32).round() as i64
}

/// The recipe's optimiser clips every weight to 1.98 either way, read at the pinned revision; a
/// trainer that changed it would change what every bound below rests on.
#[test]
fn the_trainers_clip_is_the_one_read_at_its_pin() {
    let params = AdamWParams::default();
    assert!((params.max_weight - 1.98).abs() < f32::EPSILON);
    assert!((params.min_weight + 1.98).abs() < f32::EPSILON);
}

/// A clipped output weight quantises to at most 127, the loader's range for one, and a lane of 32
/// clipped feature weights and a clipped bias stays inside `i16`.
#[test]
fn the_clip_holds_each_weight_and_lane_inside_the_loader() {
    let clip = AdamWParams::default().max_weight;
    assert_eq!(quantised(clip, QB), 127);
    let lane = (MAX_ACTIVE + 1) * quantised(clip, QA);
    assert!(lane <= i64::from(i16::MAX), "a lane could reach {lane}");
}

/// Every output weight at the clip keeps one perspective's sum inside `i32`, which the loader
/// bounds, while both together pass it, which is why the evaluation adds them in `i64`.
#[test]
fn the_clip_holds_each_perspectives_output_sum() {
    let clip = AdamWParams::default().max_weight;
    let weights = i64::try_from(HIDDEN).expect("a count");
    let one = weights * quantised(clip, QB) * i64::from(QA * QA);
    assert!(one <= i64::from(i32::MAX), "one perspective reaches {one}");
    assert!(
        2 * one > i64::from(i32::MAX),
        "both together fit i32 at {}",
        2 * one
    );
}
