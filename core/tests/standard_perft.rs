// SPDX-License-Identifier: GPL-3.0-or-later

//! Corpus section 1: the standard perft suite.
//!
//! These six positions and their node counts have been published, recomputed
//! and argued over for decades. The corpus recomputed them independently and
//! agrees. If Cadence disagrees with section 1, Cadence is wrong, unlike
//! sections 2 and 3,
//! where the corpus is a live suspect.
//!
//! The tiers:
//!
//! ```text
//! fast     depth <= 5   `cargo test`                        every PR
//! nightly  depth >= 6   `cargo test --release -- --ignored`  scheduled
//! gate     startpos d7 and Kiwipete d6, run once by hand and recorded:
//!            cargo test --release --test standard_perft -- --ignored \
//!                deep_perft_startpos deep_perft_kiwipete
//! ```
//!
//! The gate is a filter over the nightly tests rather than a tier of its own,
//! so that no value is ever run twice in one nightly.
//!
//! The deep tier is cheap to run: 23.56 s for all six positions together,
//! release, M5 Max, measured 2026-08-25. An earlier ignore label said
//! "minutes to hours per position", which was the python-chess oracle's cost
//! to *generate* these values, not this engine's cost to check them, and the
//! mislabel kept the tier from being run when the corpus moved.

mod support;

macro_rules! standard_perft_tests {
    ($( $fast:ident, $deep:ident => $name:literal; )*) => { $(
        #[test]
        fn $fast() {
            let p = support::standard($name);
            support::assert_perft(
                concat!("Section 1 ", $name, " (fast)"),
                &p.fen,
                &support::upto(&p.nodes, support::FAST_STANDARD_MAX_DEPTH),
            );
        }

        #[test]
        #[ignore = "nightly tier: run in release via --ignored; seconds per position"]
        fn $deep() {
            let p = support::standard($name);
            support::assert_perft(
                concat!("Section 1 ", $name, " (deep)"),
                &p.fen,
                &support::deeper_than(&p.nodes, support::FAST_STANDARD_MAX_DEPTH),
            );
        }
    )* };
}

standard_perft_tests! {
    perft_startpos, deep_perft_startpos => "startpos";
    perft_kiwipete, deep_perft_kiwipete => "kiwipete";
    perft_pos3,     deep_perft_pos3     => "pos3";
    perft_pos4,     deep_perft_pos4     => "pos4";
    perft_pos5,     deep_perft_pos5     => "pos5";
    perft_pos6,     deep_perft_pos6     => "pos6";
}
