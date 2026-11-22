// SPDX-License-Identifier: GPL-3.0-or-later

//! A diagnostic: datagen's games at several soft node counts, the nodes each move's search spent
//! counted, to find the count whose mean matches the hard count before it.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;

use cadence_core::generate_legal;
use cadence_core::rng::Rng;
use cadence_dataset::chunk::KIND_STANDARD;
use cadence_engine::datagen::packed::{chunk_seed, shipped_net, start_kind};
use cadence_engine::datagen::{self, HASH_MB, game, opening, spawn_search};
use cadence_engine::search::{Learned, Limits, Search};
use cadence_engine::tt::Table;
use cadence_nnue::AnyNetwork;

/// The packed writer's default share of standard starts.
const SHARE: u8 = 50;

/// One game as the packed writer plays it, under `limits`: each move's nodes and finished depth.
fn game_nodes(
    seed: u64,
    index: u32,
    limits: Limits,
    net: Option<&AnyNetwork>,
    tt: &Table,
) -> Vec<(u64, u32)> {
    let mut rng = Rng::new(datagen::game_seed(seed, u64::from(index)));
    let kind = start_kind(&mut rng, SHARE);
    let probe = |board: &mut _, tt: &Table| game::probe(board, opening::SCREEN_NODES, tt);
    let (mut start, _) = opening::next_of_kind(&mut rng, tt, kind == KIND_STANDARD, &probe, net);
    let board = &mut start.board;
    tt.clear();
    let stop = AtomicBool::new(false);
    let mut search = Search::new(&stop, tt);
    search.set_limits(limits);
    let mut learned = Learned::new();
    search.swap_learned(&mut learned);
    let mut moves = Vec::new();
    while game::ending(board).is_none() {
        let best = search.run(board, &mut io::sink());
        assert!(generate_legal(board).contains(best), "an illegal move");
        moves.push((search.nodes(), search.completed_depth()));
        board.play(best);
    }
    search.swap_learned(&mut learned);
    moves
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    let at = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[at]
}

fn main() {
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .map(|a| {
            a.parse()
                .expect("numbers: games, threads, cap, then soft counts (0 for hard)")
        })
        .collect();
    let (games, threads, cap) = (args[0], args[1] as usize, args[2]);
    let net = shipped_net()
        .expect("the shipped net")
        .expect("a shipped net");
    let seed = chunk_seed(0x5eed_5eed_5eed_5eed, 0);
    println!("games {games} threads {threads} cap {cap} share {SHARE} run seed 5eed5eed5eed5eed");
    println!(
        "setting | games | moves | mean nodes/move | se | p50 | p90 | p99 | p99.9 | max | at cap | depth 0 | mean depth"
    );
    for &soft in &args[3..] {
        let limits = if soft == 0 {
            Limits {
                nodes: Some(cap),
                ..Limits::default()
            }
        } else {
            Limits {
                soft_nodes: Some(soft),
                nodes: Some(cap),
                ..Limits::default()
            }
        };
        let next = AtomicU32::new(0);
        let per_game: Vec<Vec<(u64, u32)>> = thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let (next, net) = (&next, &net);
                    spawn_search(scope, move || {
                        let tt = Table::new(HASH_MB).expect("a table");
                        let mut mine = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            if u64::from(index) >= games {
                                return mine;
                            }
                            mine.push((index, game_nodes(seed, index, limits, Some(net), &tt)));
                        }
                    })
                    .expect("a search thread")
                })
                .collect();
            let mut all: Vec<(u32, Vec<(u64, u32)>)> = handles
                .into_iter()
                .flat_map(|h| h.join().expect("joins"))
                .collect();
            all.sort_by_key(|(i, _)| *i);
            all.into_iter().map(|(_, g)| g).collect()
        });
        let mut nodes: Vec<u64> = per_game.iter().flatten().map(|&(n, _)| n).collect();
        let moves = nodes.len() as f64;
        let total: u64 = nodes.iter().sum();
        let mean = total as f64 / moves;
        // The ratio estimator's standard error, games as the unit, since a game's moves share a tree.
        let g = per_game.len() as f64;
        let m_bar = moves / g;
        let ss: f64 = per_game
            .iter()
            .map(|game| {
                let n: u64 = game.iter().map(|&(n, _)| n).sum();
                let d = n as f64 - mean * game.len() as f64;
                d * d
            })
            .sum();
        let se = (ss / (g * (g - 1.0))).sqrt() / m_bar;
        let at_cap = nodes.iter().filter(|&&n| n >= cap).count();
        let depth0 = per_game.iter().flatten().filter(|&&(_, d)| d == 0).count();
        let depth: u64 = per_game.iter().flatten().map(|&(_, d)| u64::from(d)).sum();
        nodes.sort_unstable();
        let name = if soft == 0 {
            format!("hard {cap}")
        } else {
            format!("soft {soft}")
        };
        println!(
            "{name} | {} | {} | {mean:.1} | {se:.1} | {} | {} | {} | {} | {} | {at_cap} | {depth0} | {:.2}",
            per_game.len(),
            nodes.len(),
            percentile(&nodes, 0.5),
            percentile(&nodes, 0.9),
            percentile(&nodes, 0.99),
            percentile(&nodes, 0.999),
            nodes[nodes.len() - 1],
            depth as f64 / moves,
        );
    }
}
