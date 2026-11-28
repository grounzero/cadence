// SPDX-License-Identifier: GPL-3.0-or-later

//! Quiescence reads the table before its stand pat, cutting only away from the PV, and writes a
//! bound by beta at depth 0, never an exact one. An entry that keeps only its move cuts nowhere.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::{Move, generate_legal};
use cadence_engine::score::Score;
use cadence_engine::search::{Limits, Search};
use cadence_engine::tt::{Bound, MOVE_ONLY, Table};
use support::table;

/// Captures on both sides, out of check, and far from any mate.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// Nowhere near what the middlegame is worth, so only the table can return it.
const PLANTED: i16 = 777;

/// The value at depth 0 and ply 1, and the cutoffs the table gave.
fn quiesce(tt: &Table, fen: &str, alpha: Score, beta: Score) -> (Score, u64) {
    let stop = AtomicBool::new(false);
    let mut b = support::position(fen);
    let mut s = Search::new(&stop, tt);
    let value = s.node_window(&mut b, 0, 1, alpha, beta);
    (value, s.quiescence_table_cutoffs())
}

fn key(fen: &str) -> u64 {
    support::position(fen).key()
}

fn planted(fen: &str, score: i16, bound: Bound) -> Table {
    let tt = table();
    tt.store(key(fen), Move::NULL, score, 0, bound, false);
    tt
}

#[test]
fn a_null_window_takes_a_planted_bound_and_a_full_window_does_not() {
    let lower = planted(MIDDLEGAME, PLANTED, Bound::Lower);
    assert_eq!(
        quiesce(&lower, MIDDLEGAME, 99, 100),
        (Score::from(PLANTED), 1)
    );
    let upper = planted(MIDDLEGAME, -PLANTED, Bound::Upper);
    assert_eq!(
        quiesce(&upper, MIDDLEGAME, -100, -99),
        (-Score::from(PLANTED), 1)
    );
    // Above beta, so only the window keeps it out.
    let (value, cutoffs) = quiesce(&lower, MIDDLEGAME, -1_000, 500);
    assert_ne!(
        value,
        Score::from(PLANTED),
        "a full window took the table's bound"
    );
    assert_eq!(cutoffs, 0);
}

#[test]
fn an_entry_that_keeps_only_its_move_gives_no_cutoff() {
    let (empty, _) = quiesce(&table(), MIDDLEGAME, 99, 100);
    for bound in [Bound::Lower, Bound::Upper, Bound::Exact] {
        let tt = planted(MIDDLEGAME, MOVE_ONLY, bound);
        assert_eq!(quiesce(&tt, MIDDLEGAME, 99, 100), (empty, 0), "{bound:?}");
    }
}

#[test]
fn a_clock_of_ninety_takes_no_cutoff_from_the_table() {
    let late = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 90 60";
    let tt = planted(late, PLANTED, Bound::Lower);
    let (value, cutoffs) = quiesce(&tt, late, 99, 100);
    assert_ne!(value, Score::from(PLANTED));
    assert_eq!(cutoffs, 0);
}

/// A stand-pat cut stores beta as a lower bound, a node failing low its best as an upper bound,
/// and a full window neither as exact; each keeps the evaluation it read.
#[test]
fn quiescence_stores_a_bound_by_beta_at_depth_0_with_its_evaluation() {
    for (alpha, beta, bound) in [
        (-1_000, -999, Some(Bound::Lower)),
        (999, 1_000, Some(Bound::Upper)),
        (-1_000, 1_000, None),
    ] {
        let tt = table();
        let (value, _) = quiesce(&tt, MIDDLEGAME, alpha, beta);
        let hit = tt
            .probe(key(MIDDLEGAME))
            .expect("quiescence stored nothing");
        assert_eq!(hit.depth, 0, "({alpha}, {beta})");
        assert_ne!(hit.bound, Bound::Exact, "({alpha}, {beta})");
        if let Some(bound) = bound {
            assert_eq!((hit.bound, Score::from(hit.score)), (bound, value));
        }
        assert!(
            hit.eval.is_some(),
            "({alpha}, {beta}): no evaluation stored"
        );
    }
}

#[test]
fn a_second_quiescence_search_reads_the_evaluations_the_first_stored() {
    let tt = table();
    let reads = || {
        let stop = AtomicBool::new(false);
        let mut b = support::position(MIDDLEGAME);
        let mut s = Search::new(&stop, &tt);
        let _ = s.node_window(&mut b, 0, 1, -1_000, 1_000);
        s.table_evals()
    };
    let first = reads();
    let second = reads();
    assert!(
        second > first,
        "{second} table evaluations, against {first} cold"
    );
}

/// Every child of the root is searched where its line reaches the fifty-move draw, so each entry
/// there keeps only its move.
#[test]
fn a_result_that_reaches_the_fifty_move_draw_keeps_only_its_move() {
    let late = "7k/8/8/8/8/8/8/KQ6 w - - 95 1";
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(late);
    let mut s = support::search(Limits::depth(6), &stop, &tt);
    let _ = s.run(&mut b, &mut Vec::new());
    let mut seen = 0;
    for m in generate_legal(&b).iter() {
        b.make_move(m);
        if let Some(hit) = tt.probe(b.key()) {
            assert_eq!((hit.depth, hit.score), (0, MOVE_ONLY), "after {m:?}");
            seen += 1;
        }
        b.unmake_move(m);
    }
    assert!(seen > 0, "no child of the root was stored");
}
