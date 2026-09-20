// SPDX-License-Identifier: GPL-3.0-or-later

//! The capture probe: at a null-window node the null move did not cut, a capture whose shallow
//! search still stands above a raised beta cuts the node without its move list being searched.
//!
//! What these gates demonstrate is that the rule **happens**, that every cut it takes was
//! verified by a reduced search rather than by the quiescence screen alone, and that the
//! full window **refuses** it, through a counter that sees the refusal decide rather than a
//! tree in which the question never came up. The admission function is pinned on its own,
//! because the conditions it holds are the ones the other engines' logs say were fiddly.
//!
//! The counters are written wherever the rule runs and read on no decision path, so a
//! depth-limited search here reads no clock and the assertions are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::START_FEN;
use cadence_engine::score::{MATE_IN_MAX_PLY, mate_in};
use cadence_engine::search::{Limits, probcut_bound};
use support::table;

/// The depth the search gates run to. Eight: past the rule's minimum by enough that the probe
/// runs at several depths of the tree, and still a fraction of a second in debug.
const GATE_DEPTH: u32 = 8;

/// Search `fen` to [`GATE_DEPTH`] and hand the finished search to `check`, once the depth it
/// was asked for is known to have completed.
fn searched(fen: &str, check: impl FnOnce(&cadence_engine::search::Search<'_>)) {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(fen);
    let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
    let best = s.run(&mut b, &mut Vec::new());
    assert!(!best.is_null(), "{fen}: no move");
    assert_eq!(
        s.completed_depth(),
        GATE_DEPTH,
        "{fen}: the search did not complete depth {GATE_DEPTH}"
    );
    check(&s);
}

fn fens() -> [String; 2] {
    [START_FEN.to_string(), support::standard_fen("kiwipete")]
}

/// The rule happens: a middlegame search runs the probe and cuts on it.
///
/// Attempts prove the conditions admit the probe somewhere real, and cutoffs prove a shallow
/// capture search stands above the raised beta somewhere, which is the entire mechanism. A
/// rule wired in and never admitted passes neither; one admitted and never cutting passes only
/// the first.
#[test]
fn a_middlegame_search_cuts_through_the_capture_probe() {
    for fen in fens() {
        searched(&fen, |s| {
            assert!(
                s.probcut_attempts() > 0,
                "{fen}: depth {GATE_DEPTH} searched {} nodes and never ran the probe",
                s.nodes()
            );
            assert!(
                s.probcut_cutoffs() > 0,
                "{fen}: the probe ran at {} nodes and never cut",
                s.probcut_attempts()
            );
        });
    }
}

/// Every cut was a reduced search that ran. The quiescence screen only decides which captures
/// are worth the reduced search, so a cut counted without one would be the screen cutting the
/// node on a horizon score.
#[test]
fn every_cut_is_a_reduced_search_that_ran() {
    for fen in fens() {
        searched(&fen, |s| {
            assert!(
                s.probcut_searches() > 0,
                "{fen}: the probe ran at {} nodes and never passed a capture to the reduced search",
                s.probcut_attempts()
            );
            assert!(
                s.probcut_cutoffs() <= s.probcut_searches(),
                "{fen}: {} cuts from {} reduced searches",
                s.probcut_cutoffs(),
                s.probcut_searches()
            );
        });
    }
}

/// The full window refuses the probe, and the refusal is seen deciding. A principal-variation
/// node is where the exact score is wanted, so a bound from a shallow search is not an answer
/// there; the counter proves such a node met every other condition and was still refused.
#[test]
fn the_full_window_refuses_the_probe() {
    searched(&support::standard_fen("kiwipete"), |s| {
        assert!(
            s.probcut_refused_by_window() > 0,
            "no full-window node met the probe's other conditions, so this tree presented no case"
        );
    });
}

/// The admission function: a raised beta above beta where the rule may run, and `None`
/// everywhere it may not.
#[test]
fn the_bound_refuses_what_the_rule_may_not_touch() {
    let deep = 64;
    let raised = probcut_bound(Some(0), deep, 0, 1).expect("a deep null-window node is admitted");
    assert!(raised > 1, "the raised beta {raised} is not above beta");

    assert_eq!(
        probcut_bound(None, deep, 0, 1),
        None,
        "no static reading, which is in check"
    );
    assert_eq!(
        probcut_bound(Some(0), deep, -100, 100),
        None,
        "a full window"
    );
    assert_eq!(probcut_bound(Some(0), 0, 0, 1), None, "the horizon");

    let beta = mate_in(3);
    assert_eq!(
        probcut_bound(Some(0), deep, beta - 1, beta),
        None,
        "beta on the mate scale"
    );
    let beta = MATE_IN_MAX_PLY - 1;
    assert_eq!(
        probcut_bound(Some(0), deep, beta - 1, beta),
        None,
        "a raised beta that crosses onto the mate scale"
    );

    // Admission is a floor on depth: once a depth admits, every deeper one does.
    let first = (0..=deep)
        .find(|&d| probcut_bound(Some(0), d, 0, 1).is_some())
        .expect("some depth admits");
    assert!(
        (first..=deep).all(|d| probcut_bound(Some(0), d, 0, 1).is_some()),
        "admission is not monotone in depth"
    );
}
