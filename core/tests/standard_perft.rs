// SPDX-License-Identifier: GPL-3.0-or-later

//! If Cadence disagrees with these six positions, Cadence is wrong; in the DFRC sections the corpus
//! is a live suspect. The deep tier takes 23.56 s for all six, release, M5 Max, measured
//! 2026-08-25.
//!
//! ```text
//! cargo test --release --test standard_perft -- --ignored deep_perft_startpos deep_perft_kiwipete
//! ```

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
