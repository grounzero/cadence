// SPDX-License-Identifier: GPL-3.0-or-later

//! The probe happens, every cut was verified by a reduced search rather than the quiescence screen
//! alone, and the full window refuses it through a counter that sees the refusal decide. The
//! counters are on no decision path, so the assertions are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_engine::score::{MATE_IN_MAX_PLY, mate_in};
use cadence_engine::search::{Limits, bound_for, probcut_bound};
use cadence_engine::tt::Bound;
use support::table;

/// Past the rule's minimum by enough that the probe runs at several depths, still a fraction of a
/// second in debug.
const GATE_DEPTH: u32 = 8;

/// Once the depth asked for is known to have completed.
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

/// The start position cut once under the hand-written table and not at all under the fitted one, a
/// knife-edge; this Italian cuts 113 and 167 times, Kiwipete 279 and 221.
fn fens() -> [String; 2] {
    [
        "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10".to_string(),
        support::standard_fen("kiwipete"),
    ]
}

/// Attempts prove the conditions admit the probe; cutoffs prove a shallow capture search stands
/// above the raised beta. Never admitted passes neither, never cutting only the first.
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

/// The quiescence screen only picks captures for the reduced search, so a cut without one would be
/// the screen cutting on a horizon score.
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

/// A principal-variation node wants the exact score, which a shallow bound is not. On the Italian,
/// since Kiwipete's refusals came and went with the evaluation.
#[test]
fn the_full_window_refuses_the_probe() {
    searched(&fens()[0], |s| {
        assert!(
            s.probcut_refused_by_window() > 0,
            "no full-window node met the probe's other conditions, so this tree presented no case"
        );
    });
}

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

/// Arithmetic that left `negamax` under the line-count limit, pinned so the extraction is a gate's
/// business.
#[test]
fn a_fail_soft_value_carries_the_bound_its_window_says() {
    assert_eq!(bound_for(10, 0, 10), Bound::Lower, "at beta");
    assert_eq!(bound_for(11, 0, 10), Bound::Lower, "past beta");
    assert_eq!(bound_for(5, 0, 10), Bound::Exact, "inside the window");
    assert_eq!(
        bound_for(0, 0, 10),
        Bound::Upper,
        "at the alpha it started with"
    );
    assert_eq!(bound_for(-5, 0, 10), Bound::Upper, "below it");
}
