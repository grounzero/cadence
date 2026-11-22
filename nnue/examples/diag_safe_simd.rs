// SPDX-License-Identifier: GPL-3.0-or-later

//! Times the accumulator update and the output layer as the crate computes them and as two safe
//! SIMD libraries compute them, after checking that all three agree bit for bit.

#![forbid(unsafe_code)]

use std::hint::black_box;
use std::time::Instant;

use cadence_core::chess960::dfrc_fen;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{Colour, DirtyPieces, generate_legal};
use cadence_nnue::net::EMBEDDED;
use cadence_nnue::{Accumulator, Network, Provenance, WIDE};

type Net = Network<WIDE>;
type Acc = Accumulator<WIDE>;

/// Random games from random arrays, so captures, castles and promotions all appear.
const GAMES: usize = 1_000;
const PLIES: usize = 100;
/// Passes over the move stream in one timing, about a million updates.
const PASSES: usize = 10;
/// Child accumulators written in a ring of this many slots, a search's stack depth.
const RING: usize = 32;
const EVALS: usize = 1_000_000;
/// Timings a variant gets in one process, interleaved with the others; the fastest is kept.
const ROUNDS: usize = 3;
const VARIANTS: [&str; 3] = ["baseline", "wide", "pulp"];

struct Game {
    root: Acc,
    /// Each move's changed pieces and the side to move after it.
    moves: Vec<(DirtyPieces, Colour)>,
}

#[inline(never)]
fn baseline_update(net: &Net, parent: &Acc, dirty: &DirtyPieces) -> Acc {
    net.update(parent, dirty)
}

#[inline(never)]
fn wide_update(net: &Net, parent: &Acc, dirty: &DirtyPieces) -> Acc {
    net.update_wide(parent, dirty)
}

#[inline(never)]
fn pulp_update(net: &Net, parent: &Acc, dirty: &DirtyPieces) -> Acc {
    net.update_pulp(parent, dirty)
}

#[inline(never)]
fn baseline_evaluate(net: &Net, acc: &Acc, stm: Colour) -> i32 {
    net.evaluate(acc, stm)
}

#[inline(never)]
fn wide_evaluate(net: &Net, acc: &Acc, stm: Colour) -> i32 {
    net.evaluate_wide(acc, stm)
}

#[inline(never)]
fn pulp_evaluate(net: &Net, acc: &Acc, stm: Colour) -> i32 {
    net.evaluate_pulp(acc, stm)
}

/// The shipped net when it has the wide width, else a seeded one of that width.
fn load() -> (Net, String) {
    match Network::<WIDE>::load(EMBEDDED) {
        Ok(net) => (net, format!("shipped net, H = {WIDE}")),
        Err(e) => (
            Network::<WIDE>::generate(0xc0de, Provenance::SelfPlay),
            format!("seeded net, H = {WIDE} (the shipped one did not load: {e})"),
        ),
    }
}

fn mismatch(what: &str, game: usize, ply: usize) -> ! {
    eprintln!("BIT MISMATCH: {what} differs from the baseline at game {game}, ply {ply}");
    panic!("BIT MISMATCH: {what} at game {game}, ply {ply}");
}

/// The move stream, with every position's refresh checked against the baseline's as it is made.
fn games(net: &Net) -> Vec<Game> {
    let mut rng = Rng::new(0x5afe_51ad);
    let mut out = Vec::with_capacity(GAMES);
    for g in 0..GAMES {
        let white = u32::try_from(rng.below(960)).unwrap_or(518);
        let black = u32::try_from(rng.below(960)).unwrap_or(518);
        let mut board = Board::from_fen(&dfrc_fen(white, black)).expect("a start position");
        let root = net.refresh(&board);
        let mut moves = Vec::with_capacity(PLIES);
        for ply in 0..=PLIES {
            let want = net.refresh(&board);
            if net.refresh_wide(&board) != want {
                mismatch("wide refresh", g, ply);
            }
            if net.refresh_pulp(&board) != want {
                mismatch("pulp refresh", g, ply);
            }
            if ply == PLIES {
                break;
            }
            let legal = generate_legal(&board);
            let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                break;
            };
            let dirty = board.make_move(m);
            moves.push((dirty, board.side_to_move()));
        }
        out.push(Game { root, moves });
    }
    out
}

/// Every variant chained along every game on its own accumulators, compared after every move,
/// and every evaluation from both sides compared too.
fn check_stream(net: &Net, games: &[Game]) -> (usize, usize) {
    let (mut updates, mut evals) = (0, 0);
    for (g, game) in games.iter().enumerate() {
        let (mut base, mut wide, mut pulp) = (game.root, game.root, game.root);
        for (ply, (dirty, stm)) in game.moves.iter().enumerate() {
            base = net.update(&base, dirty);
            wide = net.update_wide(&wide, dirty);
            pulp = net.update_pulp(&pulp, dirty);
            if wide != base {
                mismatch("wide update", g, ply);
            }
            if pulp != base {
                mismatch("pulp update", g, ply);
            }
            updates += 1;
            for side in [*stm, stm.flip()] {
                let want = net.evaluate(&base, side);
                if net.evaluate_reference(&base, side) != want {
                    mismatch("the reference evaluation", g, ply);
                }
                if net.evaluate_wide(&base, side) != want {
                    mismatch("wide evaluate", g, ply);
                }
                if net.evaluate_pulp(&base, side) != want {
                    mismatch("pulp evaluate", g, ply);
                }
                evals += 1;
            }
        }
    }
    (updates, evals)
}

/// Accumulators of uniformly random lanes, so both clamp edges and the widest products appear.
fn check_random(net: &Net) -> usize {
    let mut rng = Rng::new(0xc1a3_9ed9);
    let count = 20_000;
    for i in 0..count {
        let mut acc = Accumulator {
            sums: [[0_i16; WIDE]; 2],
        };
        for lane in acc.sums.as_flattened_mut() {
            *lane = rng.next_u64() as i16;
        }
        for side in [Colour::White, Colour::Black] {
            let want = net.evaluate(&acc, side);
            if net.evaluate_reference(&acc, side) != want {
                mismatch("the reference evaluation (random lanes)", i, 0);
            }
            if net.evaluate_wide(&acc, side) != want {
                mismatch("wide evaluate (random lanes)", i, 0);
            }
            if net.evaluate_pulp(&acc, side) != want {
                mismatch("pulp evaluate (random lanes)", i, 0);
            }
        }
    }
    2 * count
}

/// The parent's slot read and the child's written, as two borrows of one ring.
fn pair(ring: &mut [Acc], parent: usize, child: usize) -> (&Acc, &mut Acc) {
    if parent < child {
        let (lo, hi) = ring.split_at_mut(child);
        (&lo[parent], &mut hi[0])
    } else {
        let (lo, hi) = ring.split_at_mut(parent);
        (&hi[0], &mut lo[child])
    }
}

/// Nanoseconds an update over the whole stream `PASSES` times, and a checksum of what it wrote.
fn time_updates(
    net: &Net,
    games: &[Game],
    ring: &mut [Acc],
    update: fn(&Net, &Acc, &DirtyPieces) -> Acc,
) -> (f64, u64) {
    let mut count = 0_usize;
    let mut check = 0_u64;
    let start = Instant::now();
    for _ in 0..PASSES {
        for game in games {
            ring[0] = game.root;
            for (i, (dirty, _)) in game.moves.iter().enumerate() {
                let (parent, child) = pair(ring, i % RING, (i + 1) % RING);
                *child = update(net, parent, black_box(dirty));
            }
            count += game.moves.len();
            let last = &ring[game.moves.len() % RING];
            check = check
                .wrapping_mul(31)
                .wrapping_add(u64::from(last.sums[0][7] as u16));
        }
        black_box(&*ring);
    }
    let ns = start.elapsed().as_secs_f64() * 1e9 / count as f64;
    (ns, check)
}

/// Nanoseconds an evaluation, cycling over a ring of the stream's accumulators.
fn time_evals(
    net: &Net,
    sample: &[(Acc, Colour)],
    evaluate: fn(&Net, &Acc, Colour) -> i32,
) -> (f64, u64) {
    let mut check = 0_u64;
    let start = Instant::now();
    for i in 0..EVALS {
        let (acc, stm) = &sample[i % sample.len()];
        let v = evaluate(net, black_box(acc), *stm);
        check = check.wrapping_mul(31).wrapping_add(v as u64);
    }
    let ns = start.elapsed().as_secs_f64() * 1e9 / EVALS as f64;
    (ns, black_box(check))
}

/// `RING` accumulators spread along the stream, each with its side to move.
fn sample(net: &Net, games: &[Game]) -> Vec<(Acc, Colour)> {
    let mut out = Vec::with_capacity(RING);
    let step = (games.len() / RING).max(1);
    for game in games.iter().step_by(step).take(RING) {
        let mut acc = game.root;
        let mut stm = Colour::White;
        for (dirty, side) in game.moves.iter().take(PLIES / 2) {
            acc = net.update(&acc, dirty);
            stm = *side;
        }
        out.push((acc, stm));
    }
    out
}

fn target() -> String {
    let mut f = Vec::new();
    for (on, name) in [
        (cfg!(target_feature = "sse2"), "sse2"),
        (cfg!(target_feature = "ssse3"), "ssse3"),
        (cfg!(target_feature = "sse4.1"), "sse4.1"),
        (cfg!(target_feature = "avx2"), "avx2"),
        (cfg!(target_feature = "avx512f"), "avx512f"),
        (cfg!(target_feature = "avx512bw"), "avx512bw"),
        (cfg!(target_feature = "neon"), "neon"),
        (cfg!(target_feature = "dotprod"), "dotprod"),
    ] {
        if on {
            f.push(name);
        }
    }
    format!(
        "{} with compile-time features [{}]",
        std::env::consts::ARCH,
        f.join(", ")
    )
}

fn main() {
    let (net, which) = load();
    println!("target: {}", target());
    println!("pulp: {}", cadence_nnue::diag::pulp_path());
    println!("net: {which}");

    let t = Instant::now();
    let games = games(&net);
    let (updates, evals) = check_stream(&net, &games);
    let random = check_random(&net);
    println!(
        "check: {updates} updates, {evals} stream evaluations and {random} random-lane evaluations \
         bit-identical across baseline, wide and pulp ({:.1} s)",
        t.elapsed().as_secs_f64()
    );

    let mut ring = vec![games[0].root; RING];
    let sample = sample(&net, &games);
    let update_fns: [fn(&Net, &Acc, &DirtyPieces) -> Acc; 3] =
        [baseline_update, wide_update, pulp_update];
    let evaluate_fns: [fn(&Net, &Acc, Colour) -> i32; 3] =
        [baseline_evaluate, wide_evaluate, pulp_evaluate];
    let mut best = [(f64::INFINITY, f64::INFINITY); 3];
    let mut checks = [(0_u64, 0_u64); 3];
    for _ in 0..ROUNDS {
        for v in 0..VARIANTS.len() {
            let (ns_u, check_u) = time_updates(&net, &games, &mut ring, update_fns[v]);
            let (ns_e, check_e) = time_evals(&net, &sample, evaluate_fns[v]);
            best[v].0 = best[v].0.min(ns_u);
            best[v].1 = best[v].1.min(ns_e);
            checks[v] = (check_u, check_e);
        }
    }
    for v in 1..VARIANTS.len() {
        if checks[v] != checks[0] {
            eprintln!(
                "BIT MISMATCH: {} timing checksums {:?}, baseline {:?}",
                VARIANTS[v], checks[v], checks[0]
            );
            panic!("BIT MISMATCH in the timed loops");
        }
    }
    for (v, name) in VARIANTS.iter().enumerate() {
        println!(
            "RESULT variant={name} ns_update={:.3} ns_eval={:.3}",
            best[v].0, best[v].1
        );
    }
}
