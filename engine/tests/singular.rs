// SPDX-License-Identifier: GPL-3.0-or-later

//! Singular extensions and multicut: the admission, the singular beta, the verification's depth,
//! and the rule firing in searches with a table at depths it acts at.

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
use cadence_engine::tune::Tunables;
use support::table;

/// What a net's search reads where a GUI sets nothing.
const DEFAULT: Tunables = Tunables::DEFAULT;

fn hit(mv: Move, score: i16, depth: u8, bound: Bound) -> Hit {
    Hit {
        mv,
        score,
        depth,
        bound,
        pv: false,
    }
}

fn shallowest() -> u32 {
    SINGULAR_DEPTH.unsigned_abs()
}

/// Every refusal, each alone against an entry that is admitted, including the root; a lower bound
/// and an exact one are both admitted.
#[test]
fn admission_refuses_each_condition_alone() {
    let b = support::position(&support::standard_fen("kiwipete"));
    let m = generate_legal(&b).as_slice()[0];
    let d = shallowest() + 2;
    let near = u8::try_from(d - 3).expect("a small depth");
    let s_beta = singular_beta(100, d, SINGULAR_MARGIN);
    for bound in [Bound::Lower, Bound::Exact] {
        let entry = Some(hit(m, 100, near, bound));
        assert_eq!(singular_admission(entry, d, 1, &DEFAULT), Some(s_beta));
    }
    let lower = |mv, depth| Some(hit(mv, 100, depth, Bound::Lower));
    let refused = [
        ("no entry", None, d, 1),
        ("no move", lower(Move::NULL, near), d, 1),
        (
            "an upper bound",
            Some(hit(m, 100, near, Bound::Upper)),
            d,
            1,
        ),
        ("four plies short", lower(m, near - 1), d, 1),
        ("too shallow", lower(m, near), shallowest() - 1, 1),
        ("the root", lower(m, near), d, 0),
    ];
    for (why, entry, depth, ply) in refused {
        assert_eq!(
            singular_admission(entry, depth, ply, &DEFAULT),
            None,
            "{why}"
        );
    }
    let mate = score::to_tt(score::mate_in(5), 1);
    let entry = Some(hit(m, mate, near, Bound::Lower));
    assert_eq!(singular_admission(entry, d, 1, &DEFAULT), None, "a mate");
    // A stored score just off the decided scale whose singular beta falls onto it.
    let edge = -score::TB_WIN_IN_MAX_PLY + 1;
    let stored = score::to_tt(edge, 1);
    assert!(!score::is_decisive(edge));
    assert!(score::is_decisive(singular_beta(edge, d, SINGULAR_MARGIN)));
    let entry = Some(hit(m, stored, near, Bound::Lower));
    assert_eq!(singular_admission(entry, d, 1, &DEFAULT), None, "the edge");
    // The handcrafted evaluation's set asks nothing of an entry the net's admits.
    let entry = Some(hit(m, 100, near, Bound::Lower));
    let hce = Tunables::HCE;
    assert_eq!(singular_admission(entry, d, 1, &hce), None, "handcrafted");
}

#[test]
fn the_singular_beta_is_the_stored_score_less_the_margin_a_ply() {
    for (tt_score, depth, margin) in [(0, 6, 2), (100, 8, 2), (-250, 9, 4), (1_000, 20, 6)] {
        let want = tt_score - margin * Score::try_from(depth).expect("a depth");
        assert_eq!(singular_beta(tt_score, depth, margin), want);
    }
    assert_eq!(
        singular_beta(Score::MIN, u32::MAX, 8),
        Score::MIN,
        "total at the extremes"
    );
}

/// Half the depth below the node at every depth a search reaches, and an admitted node's table
/// move never searched below the shallowest depth asked by a non-negative extension.
#[test]
fn the_verification_runs_at_half_the_depth_below_the_node() {
    for depth in 0..=u32::try_from(MAX_PLY).expect("a ply") {
        assert_eq!(
            verification_depth(depth),
            depth.saturating_sub(1) / 2,
            "{depth}"
        );
    }
    for depth in shallowest()..=20 {
        for ext in [0, 1] {
            assert!(child_depth(depth, ext) >= shallowest() - 1);
        }
    }
}

/// Over the first bench positions scored by a net at a depth past the rule's, the rule asks its
/// question, extends, and cuts by multicut.
#[test]
fn the_rule_fires_over_the_bench_positions() {
    let stop = AtomicBool::new(false);
    let (mut tests, mut extended, mut cut) = (0, 0, 0);
    for fen in bench::positions().into_iter().take(6) {
        let tt = table();
        let mut b = support::netted(fen);
        let mut s = support::search(Limits::depth(10), &stop, &tt);
        s.run(&mut b, &mut Vec::new());
        tests += s.singular_tests();
        extended += s.singular_extensions();
        cut += s.multicut_cutoffs();
    }
    assert!(tests > 100, "only {tests} verifications");
    assert!(extended > 10, "only {extended} extensions");
    assert!(cut > 0, "no multicut");
}
