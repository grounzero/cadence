// SPDX-License-Identifier: GPL-3.0-or-later

//! What the inference costs on the machine in hand: nanoseconds an accumulator update and an
//! evaluation, each over the bench positions and walks from them, and their share of a node.

mod support;

use std::hint::black_box;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{DirtyPieces, generate_legal};
use cadence_engine::position::Position;
use cadence_engine::search::Limits;
use cadence_nnue::{Accumulator, Network, Provenance};

/// Each round runs at least this long, so the clock's resolution is far below what it reads.
const ROUND: Duration = Duration::from_millis(120);
const ROUNDS: usize = 9;
/// Positions a bench root contributes, itself included.
const WALK: usize = 12;
const NODES: u64 = 200_000;

#[expect(
    clippy::cast_precision_loss,
    reason = "every count here is far below 2^52"
)]
fn float(n: u64) -> f64 {
    n as f64
}

/// Nanoseconds a call: repetitions doubled until a round reaches `ROUND`, then the median round.
fn per_call(calls: usize, mut pass: impl FnMut()) -> f64 {
    let mut reps = 1;
    loop {
        let t = Instant::now();
        for _ in 0..reps {
            pass();
        }
        if t.elapsed() >= ROUND {
            break;
        }
        reps *= 2;
    }
    let mut rounds: Vec<f64> = (0..ROUNDS)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..reps {
                pass();
            }
            t.elapsed().as_secs_f64() * 1e9 / float((reps * calls) as u64)
        })
        .collect();
    rounds.sort_by(f64::total_cmp);
    rounds[ROUNDS / 2]
}

/// Each bench root and the positions of a random walk from it, a dead end restarting the walk.
fn walks() -> Vec<Board> {
    let mut rng = Rng::new(0x2026_1115_0000_0001);
    let mut set = Vec::new();
    for fen in cadence_engine::bench::positions() {
        let root = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        let mut board = root.duplicate();
        set.push(board.duplicate());
        while set.len() % WALK != 0 {
            let legal = generate_legal(&board);
            match legal.as_slice().get(rng.below(legal.len().max(1))) {
                Some(&m) => {
                    let _ = board.make_move(m);
                }
                None => board = root.duplicate(),
            }
            set.push(board.duplicate());
        }
    }
    set
}

/// Nodes, static evaluations, null moves and seconds over every bench position at `NODES`.
fn searched(net: Option<&Arc<Network>>) -> (u64, u64, u64, f64) {
    let (mut nodes, mut evals, mut nulls, mut seconds) = (0, 0, 0, 0.0);
    for fen in cadence_engine::bench::positions() {
        let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        let mut pos = match net {
            Some(net) => Position::with_net(board, Arc::clone(net)),
            None => Position::new(board),
        };
        let stop = AtomicBool::new(false);
        let tt = support::table();
        let limits = Limits {
            nodes: Some(NODES),
            ..Limits::default()
        };
        let mut search = support::search(limits, &stop, &tt);
        let t = Instant::now();
        let _ = search.run(&mut pos, &mut std::io::sink());
        seconds += t.elapsed().as_secs_f64();
        nodes += search.nodes();
        evals += search.static_evals();
        nulls += search.null_attempts();
    }
    (nodes, evals, nulls, seconds)
}

/// A reading for an idle machine in the release profile; it asserts nothing about speed.
#[test]
#[ignore = "a timing: cargo test --release -p cadence-engine --test nnue_timing -- --ignored --nocapture"]
fn the_inference_cost_per_call_and_per_node() {
    let net = Arc::new(Network::seeded(0xc0de, Provenance::SelfPlay));
    let set = walks();
    let parents: Vec<Accumulator> = set.iter().map(|b| net.refresh(b)).collect();
    let mut moves: Vec<(usize, DirtyPieces)> = Vec::new();
    for (i, board) in set.iter().enumerate() {
        for &m in generate_legal(board).as_slice() {
            let mut child = board.duplicate();
            moves.push((i, child.make_move(m)));
        }
    }
    let update = per_call(moves.len(), || {
        for (i, dirty) in &moves {
            black_box(net.update(&parents[*i], dirty));
        }
    });
    let evaluation = per_call(set.len(), || {
        for (acc, board) in parents.iter().zip(&set) {
            black_box(net.evaluate(acc, board.side_to_move()));
        }
    });
    let (nodes, evals, nulls, seconds) = searched(Some(&net));
    let (hce_nodes, _, _, hce_seconds) = searched(None);
    let per_node = |n: u64, s: f64| s * 1e9 / float(n);
    let evals_per_node = float(evals) / float(nodes);
    // Every node but a null move's child is reached by a make, so by one update.
    let updates_per_node = float(nodes - nulls) / float(nodes);
    let node = per_node(nodes, seconds);
    let share = (update * updates_per_node + evaluation * evals_per_node) / node;
    println!(
        "update {update:.2} ns over {} moves; evaluation {evaluation:.2} ns over {} positions",
        moves.len(),
        set.len()
    );
    println!(
        "net search: {nodes} nodes, {node:.2} ns a node, {evals_per_node:.4} evaluations and \
         {updates_per_node:.4} updates a node; inference {:.1}% of a node",
        share * 100.0
    );
    println!(
        "handcrafted search: {hce_nodes} nodes, {:.2} ns a node",
        per_node(hce_nodes, hce_seconds)
    );
    assert!(update > 0.0 && evaluation > 0.0 && node > 0.0);
}
