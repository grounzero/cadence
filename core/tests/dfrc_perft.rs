// SPDX-License-Identifier: GPL-3.0-or-later

//! Corpus section 2: DFRC start arrays.
//!
//! Twenty double-Fischer-random start positions, both sides holding both
//! castling rights, named by the Scharnagl index of each back rank.
//!
//! **These numbers have one oracle and no cross-check.** Nothing in section 2 has
//! been compared against a second implementation, and there is no
//! decades-old published table for DFRC start arrays. When Cadence disagrees
//! here, "the corpus is wrong" is a live hypothesis: check the castling
//! convention in the corpus preamble before suspecting the magics.
//!
//! The trap this section exists to expose: in most arrays the king and rook
//! have pieces between them, so no castle is reachable until depth 5 or
//! deeper, and a run to depth 4 passes with the castling code completely
//! broken. Four of these twenty can castle at their first move and were
//! chosen for it, and are asserted separately at the bottom of this file.
//!
//! These run to **depth 5 in the fast tier**, not depth 4. Depth 4 is the
//! depth the corpus explicitly warns is worthless here, and running twenty
//! arrays to it is twenty near-duplicate tests of non-castling movegen. The
//! whole set at d5 is ~98M nodes against the ~469M the standard suite already
//! contributes.

mod support;

macro_rules! dfrc_perft_tests {
    ($( $name:ident => $wid:literal, $bid:literal; )*) => { $(
        #[test]
        fn $name() {
            let a = support::dfrc($wid, $bid);
            support::assert_perft(
                concat!("Section 2 ", stringify!($wid), "/", stringify!($bid)),
                &a.fen,
                &support::upto(&a.nodes, support::FAST_DFRC_MAX_DEPTH),
            );
        }
    )* };
}

dfrc_perft_tests! {
    perft_dfrc_518_518 => 518, 518;
    perft_dfrc_0_0     => 0, 0;
    perft_dfrc_959_959 => 959, 959;
    perft_dfrc_0_959   => 0, 959;
    perft_dfrc_959_0   => 959, 0;
    perft_dfrc_1_2     => 1, 2;
    perft_dfrc_17_342  => 17, 342;
    perft_dfrc_42_900  => 42, 900;
    perft_dfrc_100_101 => 100, 101;
    perft_dfrc_255_256 => 255, 256;
    perft_dfrc_333_777 => 333, 777;
    perft_dfrc_400_55  => 400, 55;
    perft_dfrc_512_513 => 512, 513;
    perft_dfrc_600_199 => 600, 199;
    perft_dfrc_700_701 => 700, 701;
    perft_dfrc_800_3   => 800, 3;
    perft_dfrc_850_850 => 850, 850;
    perft_dfrc_911_88  => 911, 88;
    perft_dfrc_120_640 => 120, 640;
    perft_dfrc_365_365 => 365, 365;
}

// ---------------------------------------------------------------------------
// The four arrays that can castle at move one
// ---------------------------------------------------------------------------

/// The corpus names four arrays as castling at their first move, and that
/// claim is what the whole fast tier's DFRC castling coverage rests on: most
/// arrays cannot reach a castle before depth 5. Until now the claim lived in a
/// prose table and nothing checked it.
///
/// The Black rows are the position after `1. a3`, a reachable position rather
/// than a contrived side-to-move flip.
macro_rules! immediate_castle_tests {
    ($( $name:ident => $wid:literal, $bid:literal, $stm:ident; )*) => { $(
        #[test]
        fn $name() {
            let c = support::immediate_castle($wid, $bid, support::Stm::$stm);
            let label = format!("immediate castle {}/{} {:?}", c.wid, c.bid, c.stm);
            support::assert_perft(&label, &c.fen, &c.nodes);

            let castles: Vec<String> = support::legal_moves(&label, &c.fen)
                .into_iter()
                .filter(|(_, m)| m.is_castle())
                .map(|(uci, _)| uci)
                .collect();
            assert_eq!(
                castles,
                vec![c.castling.clone()],
                "{label}: the corpus says `{}` is available at this side's first move\n  {}",
                c.castling,
                c.fen
            );
        }
    )* };
}

immediate_castle_tests! {
    immediate_castle_600_199_white => 600, 199, White;
    immediate_castle_800_3_white   => 800, 3, White;
    immediate_castle_800_3_black   => 800, 3, Black;
    immediate_castle_400_55_black  => 400, 55, Black;
    immediate_castle_911_88_black  => 911, 88, Black;
}
