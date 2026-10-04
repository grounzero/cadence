// SPDX-License-Identifier: GPL-3.0-or-later

//! The corpus's arrays must be reproduced byte for byte; the support decoder, written separately,
//! is the second opinion over all 960.

mod support;

use cadence_core::chess960::{ARRAYS, back_rank, dfrc_fen};
use cadence_core::position::Board;
use support::generative as generate;

#[test]
fn every_corpus_array_is_reproduced_from_its_numbers() {
    let arrays = support::dfrc_arrays();
    assert_eq!(arrays.len(), 20);
    for a in arrays {
        assert_eq!(dfrc_fen(a.wid, a.bid), a.fen, "{} {}", a.wid, a.bid);
    }
}

#[test]
fn every_back_rank_agrees_with_the_second_decoder() {
    for n in 0..ARRAYS {
        let rank: String = back_rank(n).iter().map(|&p| char::from(p)).collect();
        assert_eq!(rank, generate::scharnagl(n), "array {n}");
    }
}

#[test]
fn the_standard_array_is_518_and_every_start_parses() {
    assert_eq!(&back_rank(518), b"rnbqkbnr");
    for n in 0..ARRAYS {
        let fen = dfrc_fen(n, ARRAYS - 1 - n);
        assert_eq!(fen, generate::dfrc_start_fen(n, ARRAYS - 1 - n));
        Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
    }
}

#[test]
#[should_panic(expected = "960 is not a Chess960 start array")]
fn a_number_past_the_last_array_panics() {
    let _ = back_rank(ARRAYS);
}
