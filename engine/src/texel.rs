// SPDX-License-Identifier: GPL-3.0-or-later

//! Floating point lives here and on no search path; the search's table moves only when a tuned one
//! is written into the source.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::process::ExitCode;
use std::thread;

use cadence_core::position::Board;

use crate::eval::{
    self, ATTACKERS, ATTACKERS_LEN, MOBILITY, MOBILITY_LEN, MOBILITY_OFFSET, PHASE_MAX, PST,
    SHIELD, SHIELD_LEN, WEIGHT_COUNT, WEIGHTS,
};

/// Fixed whatever the thread count: floating-point addition is not associative, so a fixed split
/// summed in order makes a run repeat exactly.
const CHUNKS: usize = 64;

/// Middlegame, endgame.
pub type Real = [f64; 2];

/// Only the nonzero trace coefficients, the phase, and the result from White's point of view.
#[derive(Clone, Debug)]
pub struct Sample {
    coefficients: Vec<(u16, i16)>,
    phase: f64,
    result: f64,
}

impl Sample {
    /// `result` is 1 for a White win, 0.5 for a draw and 0 for a loss.
    #[must_use]
    pub fn new(board: &Board, result: f64) -> Sample {
        let trace = eval::trace(board);
        let coefficients = trace
            .coefficients
            .iter()
            .enumerate()
            .filter(|(_, c)| **c != 0)
            .map(|(i, c)| (i as u16, *c as i16))
            .collect();
        Sample {
            coefficients,
            phase: f64::from(trace.phase),
            result,
        }
    }

    /// Neither truncated nor clamped; under today's table its integer part is the search's
    /// evaluation before the clamp.
    #[must_use]
    pub fn evaluate(&self, weights: &[Real]) -> f64 {
        let (mut mg, mut eg) = (0.0, 0.0);
        for &(i, c) in &self.coefficients {
            let w = weights[usize::from(i)];
            mg += f64::from(c) * w[0];
            eg += f64::from(c) * w[1];
        }
        let max = f64::from(PHASE_MAX);
        (mg * self.phase + eg * (max - self.phase)) / max
    }
}

/// A FEN, a `|`, and the result from White's point of view: `1-0`, `1/2-1/2`, `0-1` or a number in
/// `[0, 1]`. A blank line or one opening `#` is `None`.
///
/// # Errors
///
/// A line with no `|`, a FEN `from_fen` refuses, or a result that is neither form.
pub fn parse_line(line: &str) -> Result<Option<(Board, f64)>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let (fen, result) = line
        .rsplit_once('|')
        .ok_or_else(|| "no `|` between the FEN and the result".to_string())?;
    let board = Board::from_fen(fen.trim()).map_err(|e| format!("bad FEN: {e:?}"))?;
    let result = match result.trim() {
        "1-0" => 1.0,
        "1/2-1/2" => 0.5,
        "0-1" => 0.0,
        other => match other.parse::<f64>() {
            Ok(r) if (0.0..=1.0).contains(&r) => r,
            _ => return Err(format!("bad result `{other}`")),
        },
    };
    Ok(Some((board, result)))
}

/// The table before any of it was fitted. The ridge pulls toward it, so every fit is held near one
/// reasoned table and not the last.
const HAND_WRITTEN: &str = include_str!("../hand-written-weights.txt");

/// # Panics
///
/// If the checked-in file is malformed or out of index order.
#[must_use]
pub fn hand_written_weights() -> Vec<Real> {
    let rows: Vec<Real> = HAND_WRITTEN
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .enumerate()
        .map(|(i, line)| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            assert_eq!(
                fields.first().copied(),
                Some(eval::weight_name(i).as_str()),
                "{line}"
            );
            let value = |f: &str| f.parse::<f64>().unwrap_or_else(|_| panic!("{line}"));
            [value(fields[1]), value(fields[2])]
        })
        .collect();
    assert_eq!(rows.len(), WEIGHT_COUNT, "hand-written weights");
    rows
}

#[must_use]
pub fn initial_weights() -> Vec<Real> {
    WEIGHTS
        .iter()
        .map(|w| [f64::from(w.mg), f64::from(w.eg)])
        .collect()
}

/// Tapered by phase as the evaluation is; `From<f64>` builds one number for every phase.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub mg: f64,
    pub eg: f64,
}

impl Scale {
    /// The endgame plus a share of the difference, so equal halves give back exactly the one
    /// number.
    #[must_use]
    pub fn at(self, phase: f64) -> f64 {
        self.eg + (self.mg - self.eg) * phase / f64::from(PHASE_MAX)
    }
}

impl From<f64> for Scale {
    fn from(k: f64) -> Scale {
        Scale { mg: k, eg: k }
    }
}

#[must_use]
pub fn sigmoid(e: f64, k: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf(-k * e / 400.0))
}

/// The thread count changes how fast this runs and never what it returns.
fn over_chunks<T: Send>(
    samples: &[Sample],
    threads: usize,
    f: &(impl Fn(&[Sample]) -> T + Sync),
) -> Vec<T> {
    let size = samples.len().div_ceil(CHUNKS).max(1);
    let parts: Vec<&[Sample]> = samples.chunks(size).collect();
    let threads = threads.clamp(1, parts.len().max(1));
    let mut out: Vec<(usize, T)> = thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let parts = &parts;
                scope.spawn(move || {
                    (t..parts.len())
                        .step_by(threads)
                        .map(|i| (i, f(parts[i])))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a tuner thread panicked"))
            .collect()
    });
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, t)| t).collect()
}

#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "a sample count is far below 2^52"
)]
pub fn loss(samples: &[Sample], weights: &[Real], k: impl Into<Scale>, threads: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let k = k.into();
    let parts = over_chunks(samples, threads, &|part: &[Sample]| {
        part.iter()
            .map(|s| (s.result - sigmoid(s.evaluate(weights), k.at(s.phase))).powi(2))
            .sum::<f64>()
    });
    parts.iter().sum::<f64>() / samples.len() as f64
}

#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "a sample count is far below 2^52"
)]
pub fn gradient(
    samples: &[Sample],
    weights: &[Real],
    k: impl Into<Scale>,
    threads: usize,
) -> (f64, Vec<Real>) {
    let count = samples.len().max(1) as f64;
    let k = k.into();
    let max = f64::from(PHASE_MAX);
    let parts = over_chunks(samples, threads, &|part: &[Sample]| {
        let mut sum = 0.0;
        let mut grad = vec![[0.0; 2]; weights.len()];
        for s in part {
            let at = k.at(s.phase);
            let predicted = sigmoid(s.evaluate(weights), at);
            sum += (s.result - predicted).powi(2);
            let scale = at * std::f64::consts::LN_10 / 400.0;
            // d(loss)/d(evaluation), before the mean.
            let slope = 2.0 * (predicted - s.result) * predicted * (1.0 - predicted) * scale;
            let dmg = slope * s.phase / max;
            let deg = slope * (max - s.phase) / max;
            for &(i, c) in &s.coefficients {
                let entry = &mut grad[usize::from(i)];
                entry[0] += f64::from(c) * dmg;
                entry[1] += f64::from(c) * deg;
            }
        }
        (sum, grad)
    });
    let mut total = 0.0;
    let mut grad = vec![[0.0; 2]; weights.len()];
    for (sum, g) in parts {
        total += sum;
        for (a, b) in grad.iter_mut().zip(&g) {
            a[0] += b[0];
            a[1] += b[1];
        }
    }
    for entry in &mut grad {
        entry[0] /= count;
        entry[1] /= count;
    }
    (total / count, grad)
}

/// Golden-section search on `(0, 10]`: the loss is unimodal in `k` for any data set with both wins
/// and losses.
#[must_use]
pub fn fit_k(samples: &[Sample], weights: &[Real], threads: usize) -> f64 {
    let ratio = (5f64.sqrt() - 1.0) / 2.0;
    let (mut lo, mut hi) = (0.0f64, 10.0f64);
    for _ in 0..60 {
        let a = hi - ratio * (hi - lo);
        let b = lo + ratio * (hi - lo);
        if loss(samples, weights, a, threads) < loss(samples, weights, b, threads) {
            hi = b;
        } else {
            lo = a;
        }
    }
    f64::midpoint(lo, hi)
}

/// Each half by golden-section search with the other held, starting from [`fit_k`]. A fixed number
/// of rounds, so a run repeats exactly.
#[must_use]
pub fn fit_phase_k(samples: &[Sample], weights: &[Real], threads: usize) -> Scale {
    const ROUNDS: usize = 8;
    let ratio = (5f64.sqrt() - 1.0) / 2.0;
    let mut k = Scale::from(fit_k(samples, weights, threads));
    for _ in 0..ROUNDS {
        for half in 0..2 {
            let with = |v: f64| {
                if half == 0 {
                    Scale { mg: v, eg: k.eg }
                } else {
                    Scale { mg: k.mg, eg: v }
                }
            };
            let (mut lo, mut hi) = (0.0f64, 10.0f64);
            for _ in 0..40 {
                let a = hi - ratio * (hi - lo);
                let b = lo + ratio * (hi - lo);
                if loss(samples, weights, with(a), threads)
                    < loss(samples, weights, with(b), threads)
                {
                    hi = b;
                } else {
                    lo = a;
                }
            }
            k = with(f64::midpoint(lo, hi));
        }
    }
    k
}

/// Adam over the weights `tuned` marks; every other weight is frozen.
#[derive(Clone, Debug)]
pub struct Settings {
    pub iterations: usize,
    pub rate: f64,
    pub threads: usize,
    /// Print the training loss every this many iterations; zero never.
    pub report: usize,
    /// Times the squared distance of each tuned half from the prior. The reported losses never
    /// include it.
    pub ridge: f64,
    /// The starting weights where `None`. The command line always passes [`hand_written_weights`],
    /// since the starting table is the last fit.
    pub prior: Option<Vec<Real>>,
}

/// Full-batch Adam; a frozen weight is returned exactly as given.
pub fn tune(
    samples: &[Sample],
    weights: &[Real],
    tuned: &[bool],
    k: impl Into<Scale>,
    settings: &Settings,
    out: &mut dyn Write,
) -> Vec<Real> {
    let halves: Vec<[bool; 2]> = tuned.iter().map(|&t| [t, t]).collect();
    tune_halves(samples, weights, &halves, k, settings, out)
}

/// Each half frozen or free separately.
pub fn tune_halves(
    samples: &[Sample],
    weights: &[Real],
    tuned: &[[bool; 2]],
    k: impl Into<Scale>,
    settings: &Settings,
    out: &mut dyn Write,
) -> Vec<Real> {
    const BETA1: f64 = 0.9;
    const BETA2: f64 = 0.999;
    const EPSILON: f64 = 1e-8;
    let k = k.into();
    let centre = settings.prior.as_deref().unwrap_or(weights);
    let mut w = weights.to_vec();
    let mut first = vec![[0.0; 2]; w.len()];
    let mut second = vec![[0.0; 2]; w.len()];
    let (mut b1, mut b2) = (1.0, 1.0);
    for iteration in 1..=settings.iterations {
        let (current, grad) = gradient(samples, &w, k, settings.threads);
        if settings.report > 0 && (iteration == 1 || iteration % settings.report == 0) {
            let _ = writeln!(out, "iteration {iteration} train {current:.8}");
        }
        b1 *= BETA1;
        b2 *= BETA2;
        for i in 0..w.len() {
            for j in (0..2).filter(|&j| tuned[i][j]) {
                let slope = grad[i][j] + 2.0 * settings.ridge * (w[i][j] - centre[i][j]);
                first[i][j] = BETA1 * first[i][j] + (1.0 - BETA1) * slope;
                second[i][j] = BETA2 * second[i][j] + (1.0 - BETA2) * slope * slope;
                let mean = first[i][j] / (1.0 - b1);
                let spread = second[i][j] / (1.0 - b2);
                w[i][j] -= settings.rate * mean / (spread.sqrt() + EPSILON);
            }
        }
        pin_levels(&mut w, weights, tuned);
    }
    w
}

/// Holds the mean of the king table, each mobility table and both king-safety tables, per half,
/// where it started. The data cannot place a level that one entry a side cancels, nor split a
/// piece's level between its count and its square, so any of them left free drifts.
fn pin_levels(w: &mut [Real], start: &[Real], tuned: &[[bool; 2]]) {
    let mobility = (1..5).map(|pt| {
        let at = MOBILITY + MOBILITY_OFFSET[pt];
        at..at + MOBILITY_LEN[pt]
    });
    let king = [
        PST + 5 * 64..PST + 6 * 64,
        ATTACKERS..ATTACKERS + ATTACKERS_LEN,
        SHIELD..SHIELD + SHIELD_LEN,
    ];
    for table in king.into_iter().chain(mobility) {
        for j in 0..2 {
            let free: Vec<usize> = table.clone().filter(|&i| tuned[i][j]).collect();
            if free.is_empty() {
                continue;
            }
            let drift: f64 = table.clone().map(|i| w[i][j] - start[i][j]).sum::<f64>();
            #[expect(clippy::cast_precision_loss, reason = "at most 64 entries")]
            let each = drift / free.len() as f64;
            for i in free {
                w[i][j] -= each;
            }
        }
    }
}

/// The sum over positions of the coefficient's size times that half's share of the phase. A half
/// resting on little data is fitted to a few positions, which is what `--min-weight` freezes.
#[must_use]
pub fn weight_mass(samples: &[Sample], threads: usize) -> Vec<Real> {
    let max = f64::from(PHASE_MAX);
    let parts = over_chunks(samples, threads, &|part: &[Sample]| {
        let mut mass = vec![[0.0; 2]; WEIGHT_COUNT];
        for s in part {
            for &(i, c) in &s.coefficients {
                let size = f64::from(c.unsigned_abs());
                mass[usize::from(i)][0] += size * s.phase / max;
                mass[usize::from(i)][1] += size * (max - s.phase) / max;
            }
        }
        mass
    });
    let mut total = vec![[0.0; 2]; WEIGHT_COUNT];
    for part in parts {
        for (a, b) in total.iter_mut().zip(&part) {
            a[0] += b[0];
            a[1] += b[1];
        }
    }
    total
}

/// Every weight if `prefixes` is empty.
#[must_use]
pub fn mask(prefixes: &[String]) -> Vec<bool> {
    (0..WEIGHT_COUNT)
        .map(|i| {
            let name = eval::weight_name(i);
            prefixes.is_empty() || prefixes.iter().any(|p| name.starts_with(p.as_str()))
        })
        .collect()
}

/// # Errors
///
/// The file cannot be read, or a line does not parse.
pub fn read(path: &str) -> Result<Vec<Sample>, String> {
    let file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let mut samples = Vec::new();
    for (n, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| format!("{path}: {e}"))?;
        match parse_line(&line) {
            Ok(Some((board, result))) => samples.push(Sample::new(&board, result)),
            Ok(None) => {}
            Err(e) => return Err(format!("{path}:{}: {e}", n + 1)),
        }
    }
    Ok(samples)
}

/// The table is printed as `name mg eg`, every weight, rounded.
#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    match tune_from_args(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cadence texel: {e}");
            eprintln!(
                "usage: cadence texel <data> [--holdout N | --holdout-file PATH] [--iterations N] [--rate R] \
                 [--threads N] [--report N] [--k K | --phase-k] [--min-weight W] [--min-weight-for PREFIX W]... [--ridge L] [--tune PREFIX]..."
            );
            ExitCode::from(2)
        }
    }
}

/// A named holdout is used whole; otherwise every `holdout`-th position, so the split is a function
/// of the file alone.
fn split(
    all: Vec<Sample>,
    holdout: usize,
    holdout_file: Option<&str>,
) -> Result<(Vec<Sample>, Vec<Sample>), String> {
    if let Some(path) = holdout_file {
        return Ok((all, read(path)?));
    }
    let (mut train, mut held) = (Vec::new(), Vec::new());
    for (i, s) in all.into_iter().enumerate() {
        if holdout > 0 && i % holdout == 0 {
            held.push(s);
        } else {
            train.push(s);
        }
    }
    Ok((train, held))
}

/// A half resting on under its floor of the training set is frozen. A weight's floor is the last of
/// `floors` whose prefix names it, else `min_weight`.
#[must_use]
pub fn freeze_sparse(
    tuned: &[bool],
    mass: &[Real],
    min_weight: f64,
    floors: &[(String, f64)],
) -> (Vec<[bool; 2]>, Vec<String>) {
    let mut lines = Vec::new();
    let halves = (0..WEIGHT_COUNT)
        .map(|i| {
            let weight = eval::weight_name(i);
            let floor = floors
                .iter()
                .rfind(|(prefix, _)| weight.starts_with(prefix.as_str()))
                .map_or(min_weight, |&(_, f)| f);
            let mut half = [tuned[i], tuned[i]];
            for (j, name) in ["mg", "eg"].into_iter().enumerate() {
                if half[j] && mass[i][j] < floor {
                    half[j] = false;
                    let size = mass[i][j];
                    lines.push(format!("sparse {weight} {name} {size:.1}"));
                }
            }
            half
        })
        .collect();
    (halves, lines)
}

/// The one given, or fitted once under the starting table.
fn choose_k(
    k: Option<f64>,
    phase_k: bool,
    train: &[Sample],
    start: &[Real],
    threads: usize,
) -> Result<Scale, String> {
    Ok(match (k, phase_k) {
        (Some(_), true) => return Err("--k and --phase-k are exclusive".to_string()),
        (Some(k), false) => Scale::from(k),
        (None, false) => Scale::from(fit_k(train, start, threads)),
        (None, true) => fit_phase_k(train, start, threads),
    })
}

struct Args {
    data: String,
    holdout: usize,
    holdout_file: Option<String>,
    k: Option<f64>,
    phase_k: bool,
    min_weight: f64,
    floors: Vec<(String, f64)>,
    prefixes: Vec<String>,
    settings: Settings,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut data = None;
    let mut holdout = 10usize;
    let mut holdout_file = None;
    let mut k = None;
    let mut phase_k = false;
    let mut min_weight = 0.0;
    let mut floors = Vec::new();
    let mut prefixes = Vec::new();
    let mut settings = Settings {
        iterations: 1000,
        rate: 1.0,
        threads: 1,
        report: 100,
        ridge: 0.0,
        prior: Some(hand_written_weights()),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--phase-k" {
            phase_k = true;
            continue;
        }
        let mut value = |name: &str| {
            it.next()
                .ok_or_else(|| format!("{name} takes a value"))
                .cloned()
        };
        let number = |name: &str, v: String| v.parse::<f64>().map_err(|_| format!("bad {name}"));
        let count = |name: &str, v: String| v.parse::<usize>().map_err(|_| format!("bad {name}"));
        match arg.as_str() {
            "--holdout" => holdout = count(arg, value(arg)?)?,
            "--holdout-file" => holdout_file = Some(value(arg)?),
            "--iterations" => settings.iterations = count(arg, value(arg)?)?,
            "--rate" => settings.rate = number(arg, value(arg)?)?,
            "--threads" => settings.threads = count(arg, value(arg)?)?.max(1),
            "--report" => settings.report = count(arg, value(arg)?)?,
            "--k" => k = Some(number(arg, value(arg)?)?),
            "--min-weight" => min_weight = number(arg, value(arg)?)?,
            "--min-weight-for" => {
                let prefix = value(arg)?;
                floors.push((prefix, number(arg, value(arg)?)?));
            }
            "--ridge" => settings.ridge = number(arg, value(arg)?)?,
            "--tune" => prefixes.push(value(arg)?),
            flag if flag.starts_with("--") => return Err(format!("unknown flag {flag}")),
            path if data.is_none() => data = Some(path.to_string()),
            extra => return Err(format!("unexpected argument {extra}")),
        }
    }
    Ok(Args {
        data: data.ok_or_else(|| "no data set named".to_string())?,
        holdout,
        holdout_file,
        k,
        phase_k,
        min_weight,
        floors,
        prefixes,
        settings,
    })
}

fn tune_from_args(args: &[String]) -> Result<(), String> {
    let Args {
        data,
        holdout,
        holdout_file,
        k,
        phase_k,
        min_weight,
        floors,
        prefixes,
        settings,
    } = parse_args(args)?;
    let (train, held) = split(read(&data)?, holdout, holdout_file.as_deref())?;
    if train.is_empty() {
        return Err("no training positions".to_string());
    }
    let tuned = mask(&prefixes);
    if !tuned.contains(&true) {
        return Err("no weight matches --tune".to_string());
    }
    let start = initial_weights();
    let t = settings.threads;
    let k = choose_k(k, phase_k, &train, &start, t)?;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "positions train {} holdout {}",
        train.len(),
        held.len()
    );
    if settings.ridge > 0.0 {
        let _ = writeln!(
            out,
            "ridge {} toward the hand-written table",
            settings.ridge
        );
    }
    let _ = writeln!(
        out,
        "tuned {} of {WEIGHT_COUNT} weights",
        tuned.iter().filter(|t| **t).count()
    );
    let _ = if phase_k {
        writeln!(out, "k mg {:.6} eg {:.6}", k.mg, k.eg)
    } else {
        writeln!(out, "k {:.6}", k.mg)
    };
    let _ = writeln!(
        out,
        "before train {:.8} holdout {:.8}",
        loss(&train, &start, k, t),
        loss(&held, &start, k, t)
    );
    for (prefix, floor) in &floors {
        let _ = writeln!(out, "floor {prefix} {floor}");
    }
    let (halves, sparse) = freeze_sparse(&tuned, &weight_mass(&train, t), min_weight, &floors);
    for line in &sparse {
        let _ = writeln!(out, "{line}");
    }
    let end = tune_halves(&train, &start, &halves, k, &settings, &mut out);
    // The table the engine would read is the rounded one, so its loss is the one that counts.
    let rounded: Vec<Real> = end.iter().map(|w| [w[0].round(), w[1].round()]).collect();
    for (label, w) in [("after", &end), ("rounded", &rounded)] {
        let _ = writeln!(
            out,
            "{label} train {:.8} holdout {:.8}",
            loss(&train, w, k, t),
            loss(&held, w, k, t)
        );
    }
    for (i, w) in rounded.iter().enumerate() {
        let _ = writeln!(
            out,
            "{} {} {}",
            eval::weight_name(i),
            w[0] as i32,
            w[1] as i32
        );
    }
    Ok(())
}
