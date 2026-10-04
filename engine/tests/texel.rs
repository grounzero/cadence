// SPDX-License-Identifier: GPL-3.0-or-later

//! The data is synthetic: results are the sigmoid of an evaluation under a known table, so a
//! correct tuner recovers that table with no games played.

mod support;

use cadence_core::position::Board;
use cadence_core::{Colour, START_FEN, generate_legal};
use cadence_engine::eval::{
    self, ATTACKERS, ATTACKERS_LEN, MATERIAL, MOBILITY, MOBILITY_LEN, MOBILITY_OFFSET, PHASE_MAX,
    PST, Rule, SCALE_FULL, SHIELD, SHIELD_LEN, WEIGHTS, evaluate, rule, trace,
};
use cadence_engine::score::MAX_EVAL;
use cadence_engine::texel::{
    Real, Sample, Scale, Settings, fit_k, fit_phase_k, freeze_sparse, gradient,
    hand_written_weights, initial_weights, loss, mask, parse_line, sigmoid, tune, tune_halves,
    weight_mass,
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

/// So `truth` is the table that minimises the loss.
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

/// So `k` is the scaling that minimises the loss under `truth`.
fn labelled_by_phase(truth: &[Real], k: Scale) -> Vec<Sample> {
    boards()
        .iter()
        .map(|b| {
            let e = Sample::new(b, 0.0).evaluate(truth);
            Sample::new(b, sigmoid(e, k.at(f64::from(eval::phase(b)))))
        })
        .collect()
}

#[test]
fn a_per_phase_k_is_recovered() {
    let weights = initial_weights();
    let planted = Scale { mg: 0.6, eg: 1.1 };
    let samples = labelled_by_phase(&weights, planted);
    let k = fit_phase_k(&samples, &weights, 1);
    assert!((k.mg - planted.mg).abs() < 1e-3, "mg {}", k.mg);
    assert!((k.eg - planted.eg).abs() < 1e-3, "eg {}", k.eg);
    // A single k is not the planted one, which is what makes the two halves worth fitting.
    let single = fit_k(&samples, &weights, 1);
    assert!(loss(&samples, &weights, k, 1) < loss(&samples, &weights, single, 1));
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "exact equality is the property: a plain k must reproduce earlier runs bit for bit"
)]
fn one_number_is_the_same_scaling_in_every_phase() {
    let weights = initial_weights();
    let samples = labelled(&weights, 1.3);
    let k = fit_phase_k(&samples, &weights, 1);
    assert!(
        (k.mg - 1.3).abs() < 1e-3 && (k.eg - 1.3).abs() < 1e-3,
        "{k:?}"
    );
    for phase in [0.0, 7.0, 24.0] {
        assert_eq!(Scale::from(0.8).at(phase), 0.8);
    }
    assert_eq!(
        gradient(&samples, &weights, 0.8, 1),
        gradient(&samples, &weights, Scale::from(0.8), 1)
    );
}

#[test]
fn the_gradient_under_a_per_phase_k_is_the_derivative_of_the_loss() {
    let weights = initial_weights();
    let k = Scale { mg: 0.6, eg: 1.1 };
    let samples = labelled_by_phase(&weights, Scale { mg: 0.9, eg: 0.7 });
    let (_, grad) = gradient(&samples, &weights, k, 1);
    let h = 1e-3;
    for (i, j) in [(MATERIAL + 1, 0), (MATERIAL + 1, 1), (MATERIAL + 3, 1)] {
        let (mut up, mut down) = (weights.clone(), weights.clone());
        up[i][j] += h;
        down[i][j] -= h;
        let numeric = (loss(&samples, &up, k, 1) - loss(&samples, &down, k, 1)) / (2.0 * h);
        let analytic = grad[i][j];
        assert!(
            (numeric - analytic).abs() <= 1e-6 * analytic.abs().max(1e-6),
            "weight {i} half {j}: numeric {numeric} analytic {analytic}"
        );
    }
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "the masses are sums of exact binary fractions of 24ths, so equality is the claim"
)]
fn the_mass_behind_a_weight_is_its_coefficient_split_by_phase() {
    // A lone knight weighs one twenty-fourth of the phase: that much middlegame, the rest endgame.
    // It reaches the zone of Black's king, so White's one attacker and Black's none carry it too,
    // and so does tempo, which every position carries.
    let board = Board::from_fen("8/8/8/4k3/3N4/8/8/4K3 w - - 0 1").expect("fen");
    let mass = weight_mass(&[Sample::new(&board, 1.0)], 1);
    let name = |i: usize| eval::weight_name(i);
    let find = |n: &str| (0..mass.len()).find(|&i| name(i) == n).expect(n);
    for n in [
        "material.knight",
        "pst.knight.d4",
        "pst.king.e1",
        "pst.king.e4",
        "mobility.knight.8",
        "attackers.0",
        "attackers.1",
        "tempo",
    ] {
        assert_eq!(mass[find(n)], [1.0 / 24.0, 23.0 / 24.0], "{n}");
    }
    assert_eq!(mass[find("pst.knight.e4")], [0.0, 0.0]);
    assert_eq!(mass.iter().filter(|m| m[0] + m[1] > 0.0).count(), 8);
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "a frozen half is returned exactly as given, which is the property"
)]
fn a_frozen_half_is_left_alone_while_its_other_half_moves() {
    let start = initial_weights();
    let mut truth = start.clone();
    truth[MATERIAL + 1] = [400.0, 380.0];
    let samples = labelled(&truth, 1.0);
    let mut tuned = vec![[false; 2]; start.len()];
    tuned[MATERIAL + 1] = [true, false];
    let settings = Settings {
        iterations: 200,
        rate: 2.0,
        threads: 1,
        report: 0,
        ridge: 0.0,
        prior: None,
    };
    let end = tune_halves(
        &samples,
        &start,
        &tuned,
        1.0,
        &settings,
        &mut std::io::sink(),
    );
    assert_eq!(end[MATERIAL + 1][1], start[MATERIAL + 1][1]);
    assert!(
        end[MATERIAL + 1][0] > start[MATERIAL + 1][0] + 20.0,
        "{:?}",
        end[MATERIAL + 1]
    );
    for i in (0..start.len()).filter(|&i| i != MATERIAL + 1) {
        assert_eq!(end[i], start[i], "{}", eval::weight_name(i));
    }
}

/// The planted knight of the test below, fitted under a ridge of `ridge`.
fn knight_under_ridge(ridge: f64) -> Real {
    let start = initial_weights();
    let mut truth = start.clone();
    truth[MATERIAL + 1] = [400.0, 380.0];
    let samples = labelled(&truth, 1.0);
    let settings = Settings {
        iterations: 400,
        rate: 2.0,
        threads: 1,
        report: 0,
        ridge,
        prior: None,
    };
    let tuned = mask(&["material.knight".to_string()]);
    tune(
        &samples,
        &start,
        &tuned,
        1.0,
        &settings,
        &mut std::io::sink(),
    )[MATERIAL + 1]
}

#[test]
fn the_ridge_holds_a_weight_nearer_its_start_the_stronger_it_is() {
    let start = initial_weights()[MATERIAL + 1];
    let fitted: Vec<Real> = [0.0, 1e-7, 1e-6, 1e-4]
        .iter()
        .map(|&r| knight_under_ridge(r))
        .collect();
    for pair in fitted.windows(2) {
        assert!(pair[1][0] < pair[0][0], "{fitted:?}");
    }
    assert!(fitted[0][0] > 390.0, "unridged {:?}", fitted[0]);
    assert!(
        (fitted[3][0] - start[0]).abs() < 5.0,
        "held {:?}",
        fitted[3]
    );
}

#[test]
fn the_king_tables_level_is_invisible_to_the_loss_and_is_held() {
    let start = initial_weights();
    let samples = labelled(&start, 1.0);
    let mut lifted = start.clone();
    for w in &mut lifted[PST + 5 * 64..PST + 6 * 64] {
        w[0] += 37.0;
        w[1] -= 21.0;
    }
    let (a, b) = (
        loss(&samples, &start, 1.0, 1),
        loss(&samples, &lifted, 1.0, 1),
    );
    assert!((a - b).abs() < 1e-12, "{a} {b}");

    // Fit the king squares towards a truth whose king table is lifted and reshaped.
    let mut truth = lifted.clone();
    truth[PST + 5 * 64 + 6][0] += 60.0;
    let samples = labelled(&truth, 1.0);
    let settings = Settings {
        iterations: 300,
        rate: 2.0,
        threads: 1,
        report: 0,
        ridge: 0.0,
        prior: None,
    };
    let end = tune(
        &samples,
        &start,
        &mask(&["pst.king".to_string()]),
        1.0,
        &settings,
        &mut std::io::sink(),
    );
    let king = PST + 5 * 64..PST + 6 * 64;
    for j in 0..2 {
        let mean = |w: &[Real]| king.clone().map(|i| w[i][j]).sum::<f64>() / 64.0;
        assert!((mean(&end) - mean(&start)).abs() < 1e-9, "half {j}");
    }
}

#[test]
#[expect(
    clippy::float_cmp,
    reason = "the tables hold integers, so equality is exact"
)]
fn the_hand_written_table_is_the_one_before_any_square_was_fitted() {
    let hand = hand_written_weights();
    let compiled = initial_weights();
    assert_eq!(hand.len(), compiled.len());
    // The fitted squares hold their hand-written values, which the build no longer has.
    let named = |n: &str| {
        hand[(0..hand.len())
            .find(|&i| eval::weight_name(i) == n)
            .expect(n)]
    };
    assert_eq!(named("material.knight"), [320.0, 300.0]);
    assert_eq!(named("pst.knight.e4"), [24.0, 16.0]);
    assert_eq!(named("pst.king.g1"), [20.0, -10.0]);
    let moved = (0..hand.len())
        .flat_map(|i| [0, 1].map(|j| (i, j)))
        .filter(|&(i, j)| hand[i][j] != compiled[i][j])
        .count();
    assert!(
        moved > 400,
        "only {moved} halves differ from the fitted build"
    );
}

#[test]
fn the_ridge_pulls_toward_the_prior_and_not_the_start() {
    let start = initial_weights();
    let samples = labelled(&start, 1.0);
    let mut prior = start.clone();
    prior[MATERIAL + 1] = [500.0, 480.0];
    let settings = Settings {
        iterations: 400,
        rate: 2.0,
        threads: 1,
        report: 0,
        ridge: 1e-3,
        prior: Some(prior),
    };
    let tuned = mask(&["material.knight".to_string()]);
    let end = tune(
        &samples,
        &start,
        &tuned,
        1.0,
        &settings,
        &mut std::io::sink(),
    );
    // The labels hold the knight where it starts and a strong ridge holds it at the prior.
    assert!(end[MATERIAL + 1][0] > 480.0, "{:?}", end[MATERIAL + 1]);
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
        ridge: 0.0,
        prior: None,
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

#[test]
fn a_mobility_tables_level_is_invisible_to_the_loss_and_is_held() {
    // Every knight adds one to its material and one to its count, so moving the level between the
    // two changes no evaluation.
    let knight = MOBILITY + MOBILITY_OFFSET[1]..MOBILITY + MOBILITY_OFFSET[1] + MOBILITY_LEN[1];
    let start = initial_weights();
    let samples = labelled(&start, 1.0);
    let mut lifted = start.clone();
    for w in &mut lifted[knight.clone()] {
        w[0] += 30.0;
        w[1] += 12.0;
    }
    lifted[MATERIAL + 1][0] -= 30.0;
    lifted[MATERIAL + 1][1] -= 12.0;
    let (a, b) = (
        loss(&samples, &start, 1.0, 1),
        loss(&samples, &lifted, 1.0, 1),
    );
    assert!((a - b).abs() < 1e-12, "{a} {b}");

    // Fit the knight's counts and squares towards a truth with a shaped count table.
    let mut truth = start.clone();
    for (n, i) in (0u8..).zip(knight.clone()) {
        truth[i][0] = 6.0 * f64::from(n) - 20.0;
        truth[i][1] = 4.0 * f64::from(n) - 10.0;
    }
    let samples = labelled(&truth, 1.0);
    let settings = Settings {
        iterations: 300,
        rate: 2.0,
        threads: 1,
        report: 0,
        ridge: 0.0,
        prior: None,
    };
    let tuned = mask(&["mobility.knight".to_string(), "pst.knight".to_string()]);
    let end = tune(
        &samples,
        &start,
        &tuned,
        1.0,
        &settings,
        &mut std::io::sink(),
    );
    for j in 0..2 {
        let mean = |w: &[Real]| knight.clone().map(|i| w[i][j]).sum::<f64>();
        assert!((mean(&end) - mean(&start)).abs() < 1e-9, "half {j}");
    }
    assert!(
        knight
            .clone()
            .any(|i| (end[i][0] - start[i][0]).abs() > 1.0),
        "the counts moved"
    );
}

#[test]
fn the_king_safety_tables_levels_are_invisible_to_the_loss_and_are_held() {
    // Each side writes one entry of each table, White's added and Black's taken away, so a
    // constant on a whole table changes no evaluation.
    let tables = [
        ATTACKERS..ATTACKERS + ATTACKERS_LEN,
        SHIELD..SHIELD + SHIELD_LEN,
    ];
    let start = initial_weights();
    let samples = labelled(&start, 1.0);
    let mut lifted = start.clone();
    for table in tables.clone() {
        for w in &mut lifted[table] {
            w[0] += 25.0;
            w[1] -= 15.0;
        }
    }
    let (a, b) = (
        loss(&samples, &start, 1.0, 1),
        loss(&samples, &lifted, 1.0, 1),
    );
    assert!((a - b).abs() < 1e-12, "{a} {b}");

    // Fit both tables towards a truth that shapes them.
    let mut truth = start.clone();
    for table in tables.clone() {
        for (n, i) in (0u8..).zip(table) {
            truth[i][0] = 9.0 * f64::from(n) - 20.0;
            truth[i][1] = 3.0 * f64::from(n) - 5.0;
        }
    }
    let samples = labelled(&truth, 1.0);
    let settings = Settings {
        iterations: 300,
        rate: 2.0,
        threads: 1,
        report: 0,
        ridge: 0.0,
        prior: None,
    };
    let tuned = mask(&["attackers".to_string(), "shield".to_string()]);
    let end = tune(
        &samples,
        &start,
        &tuned,
        1.0,
        &settings,
        &mut std::io::sink(),
    );
    for table in tables {
        for j in 0..2 {
            let sum = |w: &[Real]| table.clone().map(|i| w[i][j]).sum::<f64>();
            assert!((sum(&end) - sum(&start)).abs() < 1e-9, "{table:?} half {j}");
        }
        assert!(
            table.clone().any(|i| (end[i][0] - start[i][0]).abs() > 1.0),
            "{table:?} moved"
        );
    }
}

#[test]
fn a_family_floor_overrides_the_run_floor_for_the_weights_it_names() {
    let tuned = mask(&["mobility.knight".to_string(), "pst.knight".to_string()]);
    let mut mass = vec![[5_000.0, 5_000.0]; tuned.len()];
    mass[MOBILITY][1] = 20_000.0;
    let floors = [("mobility".to_string(), 10_000.0)];
    let (halves, sparse) = freeze_sparse(&tuned, &mass, 1_000.0, &floors);
    // The squares rest on 5,000 against the run's 1,000 and move; the counts need 10,000.
    assert_eq!(halves[PST + 64 + 27], [true, true]);
    assert_eq!(halves[MOBILITY], [false, true]);
    assert_eq!(halves[MOBILITY + 1], [false, false]);
    assert!(sparse.contains(&"sparse mobility.knight.0 mg 5000.0".to_string()));
    assert!(!sparse.iter().any(|l| l.contains("pst.")));
    // Without the family floor the counts move like the squares.
    let (halves, _) = freeze_sparse(&tuned, &mass, 1_000.0, &[]);
    assert_eq!(halves[MOBILITY + 1], [true, true]);
}

// The rules
// ---------------------------------------------------------------------------

/// Every placement of a few pieces on the given squares that is legal and that a rule reads.
fn ruled() -> Vec<Board> {
    let mut out = Vec::new();
    for white_king in [2, 4, 10, 17, 26, 34, 43] {
        for black_king in [48, 49, 56, 57, 58, 59, 60, 63] {
            for pawn in [8, 16, 24, 32, 40, 9, 17, 25, 33, 15, 23, 31, 39] {
                for extra in ["", "B@2", "B@11", "B@19"] {
                    let mut pieces = vec![(white_king, 'K'), (black_king, 'k'), (pawn, 'P')];
                    if let Some(at) = extra.strip_prefix("B@") {
                        pieces.push((at.parse().expect("square"), 'B'));
                    }
                    let squares: Vec<usize> = pieces.iter().map(|p| p.0).collect();
                    if (1..squares.len()).any(|i| squares[..i].contains(&squares[i])) {
                        continue;
                    }
                    let mut fen = String::new();
                    for rank in (0..8).rev() {
                        let mut empty = 0;
                        for file in 0..8 {
                            match pieces.iter().find(|p| p.0 == rank * 8 + file) {
                                Some(&(_, c)) => {
                                    if empty > 0 {
                                        fen.push_str(&empty.to_string());
                                        empty = 0;
                                    }
                                    fen.push(c);
                                }
                                None => empty += 1,
                            }
                        }
                        if empty > 0 {
                            fen.push_str(&empty.to_string());
                        }
                        if rank > 0 {
                            fen.push('/');
                        }
                    }
                    for stm in ["w", "b"] {
                        let Ok(b) = Board::from_fen(&format!("{fen} {stm} - - 0 1")) else {
                            continue;
                        };
                        if !b.opponent_in_check() && rule(&b).is_some() {
                            out.push(support::mirror(&b));
                            out.push(b);
                        }
                    }
                }
            }
        }
    }
    out
}

/// The search's arithmetic in wide integers, as it would be with `scale` in the rule's entry.
fn integer(b: &Board, scale: i64) -> i64 {
    let t = trace(b);
    let (mut mg, mut eg) = (0i64, 0i64);
    for (c, w) in t.coefficients.iter().zip(WEIGHTS.iter()) {
        mg += i64::from(*c) * i64::from(w.mg);
        eg += i64::from(*c) * i64::from(w.eg);
    }
    let (p, max) = (i64::from(t.phase), i64::from(PHASE_MAX));
    (mg * p + eg * (max - p)) * scale / (max * i64::from(SCALE_FULL))
}

#[test]
fn a_rules_scale_reaches_its_own_positions_and_truncates_as_the_search_would() {
    let boards = ruled();
    let weights = initial_weights();
    let mut seen = [0usize; Rule::ALL.len()];
    for b in &boards {
        let own = rule(b).expect("ruled");
        seen[own.index()] += 1;
        let full = Sample::new(b, 0.5).evaluate(&weights);
        for other in Rule::ALL.into_iter().filter(|r| *r != own) {
            let mut s = Sample::new(b, 0.5);
            s.set_scale(other, 0);
            assert_eq!(s.evaluate(&weights).to_bits(), full.to_bits(), "{other:?}");
        }
        for scale in [0, 1, 17, 32, 63, SCALE_FULL] {
            let mut s = Sample::new(b, 0.5);
            s.set_scale(own, scale);
            let e = s.evaluate(&weights);
            assert_eq!(e.trunc() as i64, integer(b, i64::from(scale)), "{scale}");
        }
    }
    assert!(seen.iter().all(|&n| n >= 100), "{seen:?}");
}

#[test]
fn the_gradient_under_a_rule_scale_is_the_derivative_of_the_loss() {
    let weights = initial_weights();
    let mut samples: Vec<Sample> = ruled()
        .iter()
        .map(|b| {
            let e = Sample::new(b, 0.0).evaluate(&weights);
            Sample::new(b, sigmoid(e - 40.0, 1.1))
        })
        .collect();
    for s in &mut samples {
        for r in Rule::ALL {
            s.set_scale(r, 24);
        }
    }
    let (_, grad) = gradient(&samples, &weights, 1.1, 1);
    for index in [MATERIAL, MATERIAL + 2, PST + 8 * 3, PST + 64 * 2 + 2] {
        for half in 0..2 {
            let h = 1e-3;
            let (mut up, mut down) = (weights.clone(), weights.clone());
            up[index][half] += h;
            down[index][half] -= h;
            let numeric = (loss(&samples, &up, 1.1, 1) - loss(&samples, &down, 1.1, 1)) / (2.0 * h);
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
