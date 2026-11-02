// SPDX-License-Identifier: GPL-3.0-or-later

//! Singular extensions: the admission, the singular beta, the verification's depth, and the rule
//! firing in searches with a table at depths it acts at.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::{MAX_PLY, Move, generate_legal};
use cadence_engine::bench;
use cadence_engine::score::{self, Score};
use cadence_engine::search::{
    Limits, SINGULAR_DEPTH, SINGULAR_MARGIN, child_depth, singular_admission, singular_beta,
    verification_depth,
};
use cadence_engine::tt::{Bound, Hit};
use support::table;

fn hit(mv: Move, score: i16, depth: u8, bound: Bound) -> Option<Hit> {
    Some(Hit {
        mv,
        score,
        depth,
        bound,
        pv: false,
    })
}

/// Every refusal, each alone against an entry that is admitted, including the root; a lower bound
/// and an exact one are both admitted.
#[test]
fn admission_refuses_each_condition_alone() {
    let b = support::position(&support::standard_fen("kiwipete"));
    let m = generate_legal(&b).as_slice()[0];
    let d = SINGULAR_DEPTH + 2;
    let near = u8::try_from(d - 3).expect("a small depth");
    let s_beta = singular_beta(100, d);
    for bound in [Bound::Lower, Bound::Exact] {
        assert_eq!(
            singular_admission(hit(m, 100, near, bound), true, d, 1),
            Some(s_beta)
        );
    }
    let refused = [
        ("no entry", None, true, d, 1),
        (
            "no move",
            hit(Move::NULL, 100, near, Bound::Lower),
            true,
            d,
            1,
        ),
        (
            "a move not listed",
            hit(m, 100, near, Bound::Lower),
            false,
            d,
            1,
        ),
        (
            "an upper bound",
            hit(m, 100, near, Bound::Upper),
            true,
            d,
            1,
        ),
        (
            "four plies short",
            hit(m, 100, near - 1, Bound::Lower),
            true,
            d,
            1,
        ),
        (
            "too shallow",
            hit(m, 100, near, Bound::Lower),
            true,
            SINGULAR_DEPTH - 1,
            1,
        ),
        ("the root", hit(m, 100, near, Bound::Lower), true, d, 0),
        (
            "a mate score",
            hit(m, score::to_tt(score::mate_in(5), 1), near, Bound::Lower),
            true,
            d,
            1,
        ),
    ];
    for (why, h, listed, depth, ply) in refused {
        assert_eq!(singular_admission(h, listed, depth, ply), None, "{why}");
    }
    // A stored score just off the mate scale whose singular beta falls onto it.
    let edge = -score::MATE_IN_MAX_PLY + 1;
    let stored = score::to_tt(edge, 1);
    assert!(!score::is_mate(edge) && score::is_mate(singular_beta(edge, d)));
    assert_eq!(
        singular_admission(hit(m, stored, near, Bound::Lower), true, d, 1),
        None
    );
}

#[test]
fn the_singular_beta_is_the_stored_score_less_the_margin_a_ply() {
    for (tt_score, depth) in [(0, 6), (100, 6), (-250, 9), (1_000, 20)] {
        let want = tt_score - SINGULAR_MARGIN * Score::try_from(depth).expect("a depth");
        assert_eq!(singular_beta(tt_score, depth), want);
    }
    assert_eq!(singular_beta(100, 6), 100 - 6 * SINGULAR_MARGIN);
    assert_eq!(
        singular_beta(Score::MIN, u32::MAX),
        Score::MIN,
        "total at the extremes"
    );
}

/// Half the depth below the node at every depth a search reaches, and an admitted node's table
/// move never searched below four plies by a non-negative extension.
#[test]
fn the_verification_runs_at_half_the_depth_below_the_node() {
    for depth in 0..=u32::try_from(MAX_PLY).expect("a ply") {
        assert_eq!(
            verification_depth(depth),
            depth.saturating_sub(1) / 2,
            "{depth}"
        );
    }
    for depth in SINGULAR_DEPTH..=20 {
        for ext in [0, 1] {
            assert!(child_depth(depth, ext) >= 4);
        }
    }
}

/// Over the first bench positions at a depth past the rule's, with the bench's table, the rule
/// asks its question and extends.
#[test]
fn the_rule_fires_over_the_bench_positions() {
    let stop = AtomicBool::new(false);
    let (mut tests, mut extended) = (0, 0);
    for fen in bench::positions().into_iter().take(6) {
        let tt = table();
        let mut b = support::position(fen);
        let mut s = support::search(Limits::depth(10), &stop, &tt);
        s.run(&mut b, &mut Vec::new());
        tests += s.singular_tests();
        extended += s.singular_extensions();
    }
    assert!(tests > 100, "only {tests} verifications");
    assert!(extended > 10, "only {extended} extensions");
}

/// A search with a table, above the rule's depth, in this file: a rule the ordering gates never
/// reach fires here, since they search with no table and one ply below it.
#[test]
fn the_rule_fires_where_the_ordering_gates_cannot_see_it() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(&support::standard_fen("kiwipete"));
    let mut s = support::search(Limits::depth(SINGULAR_DEPTH + 3), &stop, &tt);
    s.run(&mut b, &mut Vec::new());
    assert!(s.singular_tests() > 0, "no verification ran");
    assert!(s.singular_extensions() > 0, "nothing was extended");
}
