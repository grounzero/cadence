// SPDX-License-Identifier: GPL-3.0-or-later

//! A legal stored move is searched before the node's list is generated, and a cut on it leaves the
//! list unmade. A warm table, which holds such moves, still finds the mate a cold one finds.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::START_FEN;
use cadence_engine::score::{Score, mate_in};
use cadence_engine::search::Limits;
use support::table;

const GATE_DEPTH: u32 = 7;

const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// Rb8 is mate.
const MATE_IN_ONE: &str = "7k/8/6K1/8/8/8/8/1R6 w - - 0 1";

#[test]
fn stored_moves_are_tried_first_and_some_cut_before_the_list_is_made() {
    for fen in [
        START_FEN.to_string(),
        support::standard_fen("kiwipete"),
        MIDDLEGAME.to_string(),
    ] {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = support::position(&fen);
        let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
        let best = s.run(&mut b, &mut Vec::new());
        assert!(!best.is_null(), "{fen}: no move");
        let (tried, cut) = (s.stored_first(), s.stored_first_cutoffs());
        eprintln!("{fen}: {tried} stored moves tried first, {cut} cut");
        assert!(
            tried > 0 && cut > 0 && cut <= tried,
            "{fen}: {tried} tried, {cut} cut"
        );
    }
}

#[test]
fn a_warm_table_finds_the_mate_a_cold_one_finds() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let score = |depth| -> Score {
        let mut b = support::position(MATE_IN_ONE);
        let mut s = support::search(Limits::depth(depth), &stop, &tt);
        s.node(&mut b, depth, 0)
    };
    let cold = score(3);
    let warm = score(3);
    assert_eq!(cold, mate_in(1), "the cold search");
    assert_eq!(
        warm, cold,
        "the same search again, over the table the first one filled"
    );
}
