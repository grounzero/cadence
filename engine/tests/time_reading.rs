// SPDX-License-Identifier: GPL-3.0-or-later

//! A reading, not a gate: each bench position's iteration ladder, with the time rule replayed over
//! it for fresh searches and ponder hits, printed for CI.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadence_core::Colour;
use cadence_core::position::Board;
use cadence_engine::bench::positions;
use cadence_engine::position::Position;
use cadence_engine::search::{Limits, Search};
use cadence_engine::time::{
    another_iteration_fits, budget, hit_budget, soft_for_run, starts_another,
};
use cadence_engine::tt::Table;
use cadence_engine::tune::Tunables;
use cadence_nnue::AnyNetwork;
use cadence_nnue::net::EMBEDDED;

/// The bot's table size.
const HASH_MB: usize = 64;

/// Nodes per position for the recorded ladders.
const CAP: u64 = 40_000_000;

/// One completed iteration: when it completed, in ms from the search's start, and its best move.
#[derive(Clone)]
struct Step {
    ms: u64,
    best: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// Completed past the run-scaled soft budget.
    Soft,
    /// Stopped by the prediction that the next iteration would pass hard.
    Predicted,
    /// An iteration aborted at the hard budget.
    Hard,
    /// After a hit, the ponder's own iteration aborted at the soft budget.
    HitSoft,
    /// The ladder ended before the budget could be decided.
    Censored,
    /// Answered at the hit from the last iteration pondered.
    AtHit,
}

struct Outcome {
    /// Milliseconds charged to the clock, from the go or from the hit.
    spent: u64,
    /// Milliseconds of that inside an iteration that was aborted.
    wasted: u64,
    end: End,
    /// Completed iterations after the hit, or in all for a fresh search.
    completed: usize,
}

fn run_length(roots: &[&str]) -> usize {
    let Some(last) = roots.last() else { return 0 };
    roots.iter().rev().take_while(|m| *m == last).count()
}

/// The search's own rule over a recorded ladder: soft and the prediction after each completed
/// iteration, hard inside one.
fn fresh(steps: &[Step], cap_ms: u64, limits: &Limits) -> Outcome {
    let b = budget(limits, Colour::White).expect("a clocked limit");
    let mut iterations = Vec::new();
    let mut roots = Vec::new();
    let mut last = 0;
    for step in steps {
        if step.ms >= b.hard {
            return aborted(b.hard, last, End::Hard, roots.len());
        }
        roots.push(step.best.as_str());
        iterations.push(step.ms);
        last = step.ms;
        let soft = soft_for_run(limits, b, run_length(&roots));
        if step.ms >= soft {
            return done(step.ms, End::Soft, roots.len());
        }
        if !another_iteration_fits(&iterations, b) {
            return done(step.ms, End::Predicted, roots.len());
        }
    }
    if cap_ms >= b.hard {
        aborted(b.hard, last, End::Hard, roots.len())
    } else {
        aborted(cap_ms, last, End::Censored, roots.len())
    }
}

/// A ponder over the same ladder, hit `hit_ms` after it began, under the fixed rule: the ladder is
/// kept, the deadlines move by the time pondered, and the hit takes the decision skipped.
fn hit(steps: &[Step], cap_ms: u64, limits: &Limits, hit_ms: u64) -> Outcome {
    let b = hit_budget(limits, Colour::White).expect("a ponder with a clock");
    let hard = b.moved(hit_ms).hard;
    let mut iterations = Vec::new();
    let mut roots = Vec::new();
    for step in steps.iter().take_while(|s| s.ms <= hit_ms) {
        roots.push(step.best.as_str());
        iterations.push(step.ms);
    }
    if !starts_another(limits, b, &iterations, run_length(&roots), hit_ms) {
        return done(0, End::AtHit, 0);
    }
    let mut after = 0;
    let mut last = 0;
    for step in steps.iter().skip(iterations.len()) {
        if step.ms >= hard {
            return aborted(hard - hit_ms, last, End::Hard, after);
        }
        roots.push(step.best.as_str());
        iterations.push(step.ms);
        after += 1;
        last = step.ms - hit_ms;
        let soft = soft_for_run(limits, b, run_length(&roots)) + hit_ms;
        if step.ms >= soft {
            return done(last, End::Soft, after);
        }
        if !starts_another(limits, b, &iterations, run_length(&roots), hit_ms) {
            return done(last, End::Predicted, after);
        }
    }
    if cap_ms >= hard {
        aborted(hard - hit_ms, last, End::Hard, after)
    } else {
        aborted(cap_ms.saturating_sub(hit_ms), last, End::Censored, after)
    }
}

fn done(spent: u64, end: End, completed: usize) -> Outcome {
    Outcome {
        spent,
        wasted: 0,
        end,
        completed,
    }
}

fn aborted(spent: u64, last: u64, end: End, completed: usize) -> Outcome {
    Outcome {
        spent,
        wasted: spent.saturating_sub(last),
        end,
        completed,
    }
}

fn limits(s: &str) -> Limits {
    Limits::parse(s.split_whitespace())
}

fn field(line: &str, key: &str) -> Option<String> {
    let mut it = line.split_whitespace();
    it.find(|t| *t == key)?;
    it.next().map(str::to_string)
}

/// The completed iterations from a run's `info` lines.
fn parse(out: &[u8]) -> Vec<(u32, u64, u64, String)> {
    String::from_utf8_lossy(out)
        .lines()
        .filter(|l| l.starts_with("info depth "))
        .filter_map(|l| {
            Some((
                field(l, "depth")?.parse().ok()?,
                field(l, "nodes")?.parse().ok()?,
                field(l, "time")?.parse().ok()?,
                field(l, "pv")?,
            ))
        })
        .collect()
}

fn on_big_stack(work: impl FnOnce() + Send) {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn_scoped(s, work)
            .expect("a thread")
            .join()
            .expect("the reading");
    });
}

fn summary(label: &str, outcomes: &[Outcome], soft: u64) {
    let mut ratios: Vec<f64> = outcomes
        .iter()
        .filter(|o| o.end != End::Censored)
        .map(|o| o.spent as f64 / soft as f64)
        .collect();
    ratios.sort_by(f64::total_cmp);
    let n = ratios.len();
    let censored = outcomes.len() - n;
    if n == 0 {
        println!("SUMMARY {label} n 0 censored {censored}");
        return;
    }
    let q = |f: f64| ratios[((n - 1) as f64 * f).round() as usize];
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let count = |e: End| outcomes.iter().filter(|o| o.end == e).count();
    let wasted: f64 = outcomes
        .iter()
        .filter(|o| o.end != End::Censored)
        .map(|o| o.wasted as f64 / soft as f64)
        .sum::<f64>()
        / n as f64;
    let after: f64 =
        outcomes.iter().map(|o| o.completed as f64).sum::<f64>() / outcomes.len() as f64;
    println!(
        "SUMMARY {label} n {n} censored {censored} median {:.3} p25 {:.3} p75 {:.3} mean {mean:.3} \
         wasted_mean {wasted:.3} soft {} predicted {} hard {} hitsoft {} athit {} completed_mean {after:.2}",
        q(0.5),
        q(0.25),
        q(0.75),
        count(End::Soft),
        count(End::Predicted),
        count(End::Hard),
        count(End::HitSoft),
        count(End::AtHit),
    );
}

/// The clocks the engine is sent in 3+2, after the bridge keeps 2 s back, early to late.
const CLOCKS: [(&str, u64); 4] = [
    ("early", 178_000),
    ("mid", 30_000),
    ("late", 6_000),
    ("late300", 7_700),
];

/// Single-thread ladders in nodes, replayed at several node rates and opponent thinking times.
#[test]
#[ignore = "a reading for CI, minutes in release"]
fn ladders() {
    on_big_stack(|| {
        let net = AnyNetwork::shipped(EMBEDDED)
            .expect("a net")
            .expect("it loads");
        let stop = AtomicBool::new(false);
        let tt = Table::new(HASH_MB).expect("a table");
        let mut recorded: Vec<Vec<(u64, String)>> = Vec::new();
        for (i, fen) in positions().into_iter().enumerate() {
            tt.clear();
            let board = Board::from_fen(fen).expect("a position");
            let mut pos = Position::with_net(board, net.clone());
            let mut s = Search::new(&stop, &tt);
            s.set_limits(Limits {
                nodes: Some(CAP),
                ..Limits::default()
            });
            s.set_tunables(Tunables::DEFAULT);
            let mut out = Vec::new();
            let _ = s.run(&mut pos, &mut out);
            let steps = parse(&out);
            let shown: Vec<String> = steps
                .iter()
                .map(|(d, n, _, m)| format!("{d}:{n}:{m}"))
                .collect();
            println!("LADDER {} {fen} | {}", i + 1, shown.join(" "));
            recorded.push(steps.into_iter().map(|(_, n, _, m)| (n, m)).collect());
        }
        for nps in [1_000_000_u64, 4_000_000] {
            let to_steps = |l: &Vec<(u64, String)>| -> Vec<Step> {
                l.iter()
                    .map(|(n, m)| Step {
                        ms: n * 1000 / nps,
                        best: m.clone(),
                    })
                    .collect()
            };
            let cap_ms = CAP * 1000 / nps;
            for (name, clock) in CLOCKS {
                let go = limits(&format!("wtime {clock} btime {clock} winc 2000 binc 2000"));
                let soft = budget(&go, Colour::White).expect("a budget").soft;
                let outcomes: Vec<Outcome> = recorded
                    .iter()
                    .map(|l| fresh(&to_steps(l), cap_ms, &go))
                    .collect();
                for (i, o) in outcomes.iter().enumerate() {
                    println!(
                        "CASE fresh nps {nps} clock {name} pos {} spent {} soft {soft} end {:?} completed {}",
                        i + 1,
                        o.spent,
                        o.end,
                        o.completed
                    );
                }
                summary(&format!("fresh nps {nps} clock {name}"), &outcomes, soft);
                let ponder = limits(&format!(
                    "ponder wtime {clock} btime {clock} winc 2000 binc 2000"
                ));
                for think in [500_u64, 2_000, 5_000] {
                    let outcomes: Vec<Outcome> = recorded
                        .iter()
                        .map(|l| hit(&to_steps(l), cap_ms, &ponder, think))
                        .collect();
                    for (i, o) in outcomes.iter().enumerate() {
                        println!(
                            "CASE hit{think} nps {nps} clock {name} pos {} spent {} soft {soft} end {:?} completed {}",
                            i + 1,
                            o.spent,
                            o.end,
                            o.completed
                        );
                    }
                    summary(
                        &format!("hit{think} nps {nps} clock {name}"),
                        &outcomes,
                        soft,
                    );
                }
            }
        }
    });
}

/// Real searches on the runner's clock, each replayed over its own `info` ladder, so the replay
/// above is checked against the search it stands for.
#[test]
#[ignore = "a reading for CI, minutes in release"]
fn wall() {
    on_big_stack(|| {
        let net = AnyNetwork::shipped(EMBEDDED)
            .expect("a net")
            .expect("it loads");
        let tt = Table::new(HASH_MB).expect("a table");
        let clock = 6_000;
        let go = limits(&format!("wtime {clock} btime {clock} winc 2000 binc 2000"));
        let ponder = limits(&format!(
            "ponder wtime {clock} btime {clock} winc 2000 binc 2000"
        ));
        let soft = budget(&go, Colour::White).expect("a budget").soft;
        for (i, fen) in positions().into_iter().enumerate().step_by(3) {
            for think in [0_u64, 1_000] {
                tt.clear();
                let stop = AtomicBool::new(false);
                let flag = AtomicBool::new(false);
                let board = Board::from_fen(fen).expect("a position");
                let mut pos = Position::with_net(board, net.clone());
                let mut s = Search::new(&stop, &tt);
                s.set_limits(if think == 0 { go } else { ponder });
                s.set_tunables(Tunables::DEFAULT);
                s.set_ponder_hit(&flag);
                let mut out = Vec::new();
                let started = Instant::now();
                let hit_at = std::thread::scope(|scope| {
                    let setter = scope.spawn(|| {
                        if think == 0 {
                            return 0;
                        }
                        std::thread::sleep(Duration::from_millis(think));
                        flag.store(true, Ordering::Relaxed);
                        u64::try_from(started.elapsed().as_millis()).expect("ms")
                    });
                    let _ = s.run(&mut pos, &mut out);
                    setter.join().expect("the setter")
                });
                let total = u64::try_from(started.elapsed().as_millis()).expect("ms");
                let steps: Vec<Step> = parse(&out)
                    .into_iter()
                    .map(|(_, _, ms, m)| Step { ms, best: m })
                    .collect();
                let (actual, replay) = if think == 0 {
                    (total, fresh(&steps, u64::MAX, &go))
                } else {
                    (total - hit_at, hit(&steps, u64::MAX, &ponder, hit_at))
                };
                println!(
                    "WALL {} think {think} actual {actual} replay {} soft {soft} end {:?} iters {:?}",
                    i + 1,
                    replay.spent,
                    replay.end,
                    s.iterations_ms()
                );
            }
        }
    });
}
