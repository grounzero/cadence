// SPDX-License-Identifier: GPL-3.0-or-later

//! The kind each node is expected to be, derived from how its parent searched it. Nothing reads it
//! yet; the call sites are gated beside them in `search::node`.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_engine::search::{Expected, Limits, Via};
use support::table;

/// Every case, written out, so a change to any one is a change to this table.
#[test]
fn the_derivation_table() {
    use Expected::{All, Cut, Pv};
    let table = [
        (Pv, Via::First, Pv),
        (Cut, Via::First, All),
        (All, Via::First, Cut),
        (Pv, Via::Open, Pv),
        (Cut, Via::Open, Pv),
        (All, Via::Open, Pv),
        (Pv, Via::Reduced, Cut),
        (Cut, Via::Reduced, Cut),
        (All, Via::Reduced, Cut),
        (Pv, Via::Null, Cut),
        (Cut, Via::Null, All),
        (All, Via::Null, Cut),
    ];
    for (parent, via, child) in table {
        assert_eq!(parent.child(via), child, "{parent:?} via {via:?}");
    }
}

/// A debug build checks every node's kind against its window, so a whole search through the root,
/// the aspiration windows and every rule that searches a child is the gate.
#[test]
fn every_node_of_a_search_is_pv_exactly_where_its_window_is_open() {
    let stop = AtomicBool::new(false);
    for fen in [
        support::standard_fen("kiwipete"),
        "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14".to_string(),
        support::PAWN_ENDGAMES[0].to_string(),
    ] {
        let tt = table();
        let mut b = support::position(&fen);
        let mut s = support::search(Limits::depth(8), &stop, &tt);
        s.run(&mut b, &mut Vec::new());
        assert!(
            s.nodes() > 10_000,
            "{fen}: too small a tree to say anything"
        );
    }
}
