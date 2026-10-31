// SPDX-License-Identifier: GPL-3.0-or-later

//! The improving flag at every case: a node's evaluation against the one two plies up its line.

use cadence_engine::search::improving;

/// True only where both readings exist and the later is higher; the reading one ply up is never
/// asked.
#[test]
fn a_higher_reading_than_two_plies_up_is_improving() {
    assert!(improving(&[Some(10), None, Some(11)], 2));
    assert!(improving(&[Some(10), Some(1_000), Some(11)], 2));
    assert!(improving(
        &[Some(0), Some(0), Some(-50), Some(0), Some(-40)],
        4
    ));
}

#[test]
fn an_equal_or_lower_reading_is_not_improving() {
    assert!(!improving(&[Some(10), None, Some(10)], 2));
    assert!(!improving(&[Some(10), None, Some(9)], 2));
    assert!(!improving(&[Some(10), Some(-1_000), Some(9)], 2));
}

/// A missing reading is a node in check, now or two plies up.
#[test]
fn a_missing_reading_is_not_improving() {
    assert!(!improving(&[Some(10), Some(0), None], 2));
    assert!(!improving(&[None, Some(0), Some(10)], 2));
    assert!(!improving(&[None, None, None], 2));
}

/// No reading exists two plies above plies zero and one, and none past the end of the slice.
#[test]
fn the_first_two_plies_and_past_the_end_are_not_improving() {
    let evals = [Some(-100), Some(0), Some(100)];
    assert!(!improving(&evals, 0));
    assert!(!improving(&evals, 1));
    assert!(!improving(&evals, 3));
    assert!(!improving(&evals, usize::MAX));
    assert!(!improving(&[], 0));
}
