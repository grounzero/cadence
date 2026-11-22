// SPDX-License-Identifier: GPL-3.0-or-later

//! The two learning-rate schedules start and end at the same rates, the stepped one falling five
//! times and the cosine never rising.

use bullet_lib::trainer::schedule::lr::LrScheduler;
use bullet_lib::trainer::schedule::wdl::WdlScheduler;
use cadence_train::schedule::{FINAL_LR, INITIAL_LR, cosine, cosine_from, share, stepped};

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

/// A run that continues another starts its cosine at the rate it is given and ends where the
/// recipe's does, never rising between.
#[test]
fn a_continued_run_falls_from_its_own_first_rate_to_the_same_last_one() {
    let c = cosine_from(0.000_01, 4);
    assert!(near(c.lr(0, 1), 0.000_01) && near(c.lr(0, 4), FINAL_LR));
    assert!((2..=4).all(|sb| c.lr(0, sb) <= c.lr(0, sb - 1)));
    assert_eq!(
        cosine(40).lr(0, 7).to_bits(),
        cosine_from(INITIAL_LR, 40).lr(0, 7).to_bits()
    );
}

/// The result share runs on a line from its first value to its last, and with the two equal it is
/// that value to the bit in every superbatch, as the constant share was.
#[test]
fn the_result_share_runs_on_a_line_and_equal_ends_are_a_constant() {
    let rising = share(0.3, 0.7);
    assert!(near(rising.blend(0, 1, 40), 0.3) && near(rising.blend(0, 40, 40), 0.7));
    assert!((2..=40).all(|sb| rising.blend(0, sb, 40) > rising.blend(0, sb - 1, 40)));
    let level = share(0.5, 0.5);
    assert!((1..=40).all(|sb| level.blend(0, sb, 40).to_bits() == 0.5_f32.to_bits()));
}
