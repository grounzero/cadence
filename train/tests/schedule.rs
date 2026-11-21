// SPDX-License-Identifier: GPL-3.0-or-later

//! The two learning-rate schedules start and end at the same rates, the stepped one falling five
//! times and the cosine never rising.

use bullet_lib::trainer::schedule::lr::LrScheduler;
use cadence_train::schedule::{FINAL_LR, INITIAL_LR, cosine, stepped};

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= b.abs() * 1e-5
}

#[test]
fn both_schedules_run_between_the_same_two_rates() {
    for superbatches in [40, 37, 12] {
        let (c, s) = (cosine(superbatches), stepped(superbatches));
        assert!(near(c.lr(0, 1), INITIAL_LR) && near(s.lr(0, 1), INITIAL_LR));
        assert!(
            near(c.lr(0, superbatches), FINAL_LR),
            "the cosine ends at {} over {superbatches}",
            c.lr(0, superbatches)
        );
        assert!(
            near(s.lr(0, superbatches), FINAL_LR),
            "the steps end at {} over {superbatches}",
            s.lr(0, superbatches)
        );
    }
}

#[test]
fn the_steps_fall_five_times_and_the_cosine_never_rises() {
    let (c, s) = (cosine(40), stepped(40));
    let falls = (2..=40).filter(|&sb| s.lr(0, sb) < s.lr(0, sb - 1)).count();
    assert_eq!(falls, 5);
    assert!((2..=40).all(|sb| c.lr(0, sb) <= c.lr(0, sb - 1)));
}
