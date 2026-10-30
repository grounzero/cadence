// SPDX-License-Identifier: GPL-3.0-or-later

//! The kind each node is expected to be, derived from how its parent searched it. The call sites
//! are gated beside them in `search::node`, and the record of each call that these read exists
//! only in a debug build.

mod support;

use std::sync::atomic::AtomicBool;

#[cfg(debug_assertions)]
use cadence_engine::search::Call;
use cadence_engine::search::{Expected, Limits, Via};
use support::table;

/// Kiwipete, a quiet middlegame and a pawn endgame.
fn positions() -> [String; 3] {
    [
        support::standard_fen("kiwipete"),
        MIDDLEGAME.to_string(),
        support::PAWN_ENDGAMES[0].to_string(),
    ]
}

const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

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

/// A debug build checks the kind of every call into the main search against its window before the
/// call can return, so a whole search through the root, the aspiration windows and every rule that
/// searches a child is the gate.
#[test]
fn every_node_of_a_search_is_pv_exactly_where_its_window_is_open() {
    let stop = AtomicBool::new(false);
    for fen in positions() {
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

/// Every call that derives its kind names the kind its parent was given, which is the last one
/// recorded a ply up; below the root, every other is the null move's all node.
#[cfg(debug_assertions)]
#[test]
fn every_child_is_derived_from_the_kind_its_parent_was_given() {
    let stop = AtomicBool::new(false);
    for fen in positions() {
        let tt = table();
        let mut b = support::position(&fen);
        let mut s = support::search(Limits::depth(8), &stop, &tt);
        s.trace_calls();
        s.run(&mut b, &mut Vec::new());
        let mut path: Vec<Expected> = Vec::new();
        let mut derived = 0;
        for call in s.calls() {
            path.truncate(call.ply);
            assert_eq!(path.len(), call.ply, "{fen}: no parent recorded");
            match call.asked {
                Some((parent, via)) => {
                    assert_eq!(path.last(), Some(&parent), "{fen}: ply {}", call.ply);
                    assert_eq!(call.kind, parent.child(via), "{fen}: ply {}", call.ply);
                    derived += 1;
                }
                None if call.ply > 0 => assert_eq!(call.kind, Expected::All, "{fen}: {call:?}"),
                None => assert_eq!(call.kind, Expected::Pv, "{fen}: the root"),
            }
            path.push(call.kind);
        }
        assert!(derived > 10_000, "{fen}: too few calls to say anything");
    }
}

/// Each root search's first child is PV, each later one a cut node under the null window, and one
/// that beats alpha is searched again as PV. With two lines the second skips the first's move and
/// still searches its first child as PV.
#[cfg(debug_assertions)]
#[test]
fn the_roots_first_child_is_pv_and_its_later_children_are_cut_nodes() {
    use Expected::{Cut, Pv};
    let stop = AtomicBool::new(false);
    for lines in [1, 2] {
        let tt = table();
        let mut b = support::position(MIDDLEGAME);
        let mut s = support::search(Limits::depth(7), &stop, &tt);
        s.set_multipv(lines);
        s.trace_calls();
        s.run(&mut b, &mut Vec::new());
        let mut searches: Vec<Vec<Call>> = Vec::new();
        for call in s.calls() {
            match call.ply {
                0 => searches.push(Vec::new()),
                1 => searches.last_mut().expect("the root first").push(*call),
                _ => {}
            }
        }
        assert!(
            searches.len() >= 7 * lines,
            "{lines} lines: too few root searches"
        );
        let mut reached = Vec::new();
        for (n, children) in searches.iter().enumerate() {
            let read: Vec<_> = children.iter().map(|c| (c.asked, c.kind, c.open)).collect();
            let at = format!("{lines} lines, root search {n}");
            assert_eq!(read[0], (Some((Pv, Via::First)), Pv, true), "{at}");
            for (j, pair) in read.windows(2).enumerate() {
                match pair[1] {
                    (Some((Pv, Via::Null)), Cut, false) => {}
                    (Some((Pv, Via::Open)), Pv, true) => {
                        assert_eq!(pair[0].0, Some((Pv, Via::Null)), "{at}, child {}", j + 1);
                    }
                    other => panic!("{at}, child {}: {other:?}", j + 1),
                }
            }
            reached.push(read.iter().filter(|c| c.0 != Some((Pv, Via::Open))).count());
        }
        // Over the whole window no root move cuts, so each line searches every move it may.
        if lines == 2 {
            for (i, pair) in reached.chunks(2).enumerate() {
                assert_eq!(pair[1] + 1, pair[0], "iteration {}", i + 1);
            }
        }
    }
}
