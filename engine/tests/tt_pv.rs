// SPDX-License-Identifier: GPL-3.0-or-later

//! The ttPv flag a node stores: set by a full window or a flagged hit, and taken from the parent by
//! a node that fails low. Nothing reads it yet, so these read the table directly.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::{Move, generate_legal};
use cadence_engine::score::{INFINITE, Score};
use cadence_engine::search::Limits;
use cadence_engine::tt::Bound;
use support::table;

/// A quiet middlegame, deep enough at the depths below that the line has several interior nodes.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// Below the depth at which a node with no table move is searched shallower, so every root here
/// stores at the depth it was asked.
const DEPTH: u32 = 3;

/// Far above the position's value, so the root fails low with no preamble rule cutting it.
const NULL_WINDOW: (Score, Score) = (2_000, 2_001);
const FULL_WINDOW: (Score, Score) = (-INFINITE, INFINITE);

/// The flag one root stores, searched alone on a fresh table. A seed is stored at depth zero with
/// the flag given, so it is a hit that names a move and answers nothing.
fn stored_flag(depth: u32, window: (Score, Score), seed: Option<bool>) -> bool {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(MIDDLEGAME);
    let key = b.key();
    if let Some(pv) = seed {
        let mv = generate_legal(&b).iter().next().expect("a legal move");
        tt.store(key, mv, 0, 0, Bound::Upper, pv);
    }
    let mut s = support::search(Limits::depth(depth), &stop, &tt);
    let _ = s.node_window(&mut b, depth, 0, window.0, window.1);
    let hit = tt.probe(key).expect("the root stored its result");
    assert_eq!(
        u32::from(hit.depth),
        depth,
        "the root's own store, not the seed"
    );
    hit.pv
}

/// Through the real root: every position on the line with a move below it was searched with the
/// window open and stored that way.
#[test]
fn every_interior_node_of_the_reported_line_stores_the_flag() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(MIDDLEGAME);
    let mut s = support::search(Limits::depth(9), &stop, &tt);
    s.run(&mut b, &mut Vec::new());
    let line: Vec<Move> = s.pv().to_vec();
    assert!(
        line.len() >= 5,
        "a line of {} moves reads too little",
        line.len()
    );
    let mut read = 0;
    for (i, &m) in line[..line.len() - 1].iter().enumerate() {
        b.make_move(m);
        let hit = tt
            .probe(b.key())
            .unwrap_or_else(|| panic!("ply {} was not stored", i + 1));
        assert!(hit.pv, "ply {} of the line stored no flag", i + 1);
        read += 1;
    }
    assert!(read >= 4, "only {read} interior nodes read");
}

#[test]
fn a_full_window_node_stores_the_flag_and_a_null_window_node_does_not() {
    assert!(stored_flag(DEPTH, FULL_WINDOW, None), "the full window");
    assert!(!stored_flag(DEPTH, NULL_WINDOW, None), "the null window");
}

/// The seed is too shallow to cut, so the node searches and stores again.
#[test]
fn a_null_window_node_keeps_the_flag_its_hit_carried() {
    assert!(stored_flag(DEPTH, NULL_WINDOW, Some(true)), "a flagged hit");
    assert!(
        !stored_flag(DEPTH, NULL_WINDOW, Some(false)),
        "an unflagged hit"
    );
}
