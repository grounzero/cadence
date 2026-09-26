// SPDX-License-Identifier: GPL-3.0-or-later

//! The tuner: that it reads the evaluation the search plays with, that its
//! gradient is the derivative of its loss, and that it finds weights it was
//! told to find.
//!
//! The data here is synthetic. Results are the sigmoid of an evaluation
//! under a known table, so a correct tuner recovers that table and the tests
//! need no games to have been played.

mod support;

use cadence_core::position::Board;
use cadence_core::{Colour, START_FEN, generate_legal};
use cadence_engine::eval::{self, MATERIAL, evaluate};
use cadence_engine::score::MAX_EVAL;
use cadence_engine::texel::{
    Real, Sample, Settings, fit_k, gradient, initial_weights, loss, mask, parse_line, sigmoid, tune,
};
use support::Rng;

/// Random walks from the start position and the DFRC arrays.
fn boards() -> Vec<Board> {
    let mut seeds = vec![START_FEN.to_string()];
    seeds.extend(support::dfrc_arrays().into_iter().map(|(_, _, f)| f));
    let mut rng = Rng::new(0x7E8E_1000_0000_0001);
    let mut out = Vec::new();
    for fen in &seeds {
        for _ in 0..3 {
            let mut b = Board::from_fen(fen).expect("seed");
            for _ in 0..90 {
                let legal = generate_legal(&b);
                if legal.is_empty() {
                    break;
                }
                b.play(legal.as_slice()[rng.below(legal.len())]);
                out.push(b.duplicate());
            }
        }
    }
    out
}

/// Samples whose results are the sigmoid of White's evaluation under
/// `truth`, so `truth` is the table that minimises the loss.
fn labelled(truth: &[Real], k: f64) -> Vec<Sample> {
    boards()
        .iter()
        .map(|b| {
            let e = Sample::new(b, 0.0).evaluate(truth);
            Sample::new(b, sigmoid(e, k))
        })
        .collect()
}

#[test]
fn the_tuners_evaluation_is_the_searchs_under_todays_table() {
    let weights = initial_weights();
    let bound = f64::from(MAX_EVAL - 1);
    let boards = boards();
    assert!(boards.len() >= 5000, "only {} positions", boards.len());
    for b in &boards {
        let e = Sample::new(b, 0.0).evaluate(&weights);
        let white = match b.side_to_move() {
            Colour::White => evaluate(b),
            Colour::Black => -evaluate(b),
        };
        assert!(e.abs() < bound, "{e}");
        // The search truncates its division toward zero, and so does this.
        assert_eq!(e.trunc() as i32, white, "{e}");
    }
}

#[test]
fn a_line_is_a_fen_a_bar_and_a_result() {
    let start = format!("{START_FEN} |");
    for (tail, want) in [("1-0", 1.0), ("0-1", 0.0), ("1/2-1/2", 0.5), ("0.25", 0.25)] {
        let (_, r) = parse_line(&format!("{start} {tail}"))
            .expect("parses")
            .expect("not blank");
        assert!((r - want).abs() < f64::EPSILON, "{tail}");
    }
    assert!(parse_line("").expect("blank").is_none());
    assert!(parse_line("# a comment").expect("comment").is_none());
    assert!(parse_line(START_FEN).is_err(), "no bar");
    assert!(parse_line(&format!("{start} 2-0")).is_err());
    assert!(parse_line(&format!("{start} 1.5")).is_err());
    assert!(parse_line("not a fen | 1-0").is_err());
}

#[test]
fn the_gradient_is_the_derivative_of_the_loss() {
    let mut truth = initial_weights();
    truth[MATERIAL + 1][0] += 60.0;
    let samples = labelled(&truth, 1.2);
    let weights = initial_weights();
    let (_, grad) = gradient(&samples, &weights, 1.2, 1);
    // Central differences on weights that occur often: material and a few
    // squares, both halves of each pair.
    for index in [MATERIAL, MATERIAL + 1, MATERIAL + 3, eval::PST + 64 + 27] {
        for half in 0..2 {
            let h = 1e-3;
            let (mut up, mut down) = (weights.clone(), weights.clone());
            up[index][half] += h;
            down[index][half] -= h;
            let numeric = (loss(&samples, &up, 1.2, 1) - loss(&samples, &down, 1.2, 1)) / (2.0 * h);
            let analytic = grad[index][half];
            let scale = numeric.abs().max(analytic.abs()).max(1e-12);
            assert!(
                (numeric - analytic).abs() / scale < 1e-4,
                "{} half {half}: numeric {numeric:e} analytic {analytic:e}",
                eval::weight_name(index)
            );
        }
    }
}

#[test]
fn the_thread_count_does_not_change_a_single_bit() {
    let samples = labelled(&initial_weights(), 1.0);
    let weights = initial_weights();
    let one = gradient(&samples, &weights, 1.0, 1);
    for threads in [2, 3, 8] {
        let many = gradient(&samples, &weights, 1.0, threads);
        assert_eq!(one.0.to_bits(), many.0.to_bits(), "{threads} threads");
        for (a, b) in one.1.iter().zip(&many.1) {
            assert_eq!(a[0].to_bits(), b[0].to_bits(), "{threads} threads");
            assert_eq!(a[1].to_bits(), b[1].to_bits(), "{threads} threads");
        }
    }
}

#[test]
fn k_is_recovered() {
    let weights = initial_weights();
    let samples = labelled(&weights, 1.3);
    let k = fit_k(&samples, &weights, 1);
    assert!((k - 1.3).abs() < 1e-3, "k {k}");
}

#[test]
fn a_planted_weight_is_found_and_every_frozen_one_is_left_alone() {
    let start = initial_weights();
    let mut truth = start.clone();
    truth[MATERIAL + 1] = [400.0, 380.0];
    let samples = labelled(&truth, 1.0);
    let tuned = mask(&["material.knight".to_string()]);
    assert_eq!(tuned.iter().filter(|t| **t).count(), 1);
    let settings = Settings {
        iterations: 400,
        rate: 2.0,
        threads: 1,
        report: 0,
    };
    let end = tune(
        &samples,
        &start,
        &tuned,
        1.0,
        &settings,
        &mut std::io::sink(),
    );
    let knight = end[MATERIAL + 1];
    assert!(
        (knight[0] - 400.0).abs() < 2.0 && (knight[1] - 380.0).abs() < 2.0,
        "knight {knight:?}"
    );
    assert!(loss(&samples, &end, 1.0, 1) < loss(&samples, &start, 1.0, 1));
    for (i, (a, b)) in start.iter().zip(&end).enumerate() {
        if !tuned[i] {
            let bits = |w: &Real| w.map(f64::to_bits);
            assert_eq!(bits(a), bits(b), "{} moved", eval::weight_name(i));
        }
    }
}
