// SPDX-License-Identifier: GPL-3.0-or-later

//! A reading, not a gate: the best move's share at each completed iteration over the bench
//! positions and a few hand-built ones, with the net the build ships or the handcrafted evaluation.

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_engine::position::Position;
use cadence_engine::search::{Limits, Search};
use cadence_engine::tt::Table;
use cadence_nnue::AnyNetwork;
use cadence_nnue::net::EMBEDDED;

const CANDIDATES: [(&str, &str); 4] = [
    ("queen-en-prise", "4k3/8/8/8/3q4/5N2/PPP3PP/4K3 w - - 0 1"),
    ("two-recaptures", "4k3/8/8/8/3q4/1N3N2/PPP3PP/4K3 w - - 0 1"),
    (
        "start",
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    ),
    ("one-legal", "4k3/8/8/8/8/4p3/3q4/4K2R w K - 0 1"),
];

fn read(net: Option<&AnyNetwork>, depth: u32) {
    let stop = AtomicBool::new(false);
    let tt = Table::new(16).expect("a table");
    let mut by_depth: Vec<Vec<u32>> = vec![Vec::new(); depth as usize + 1];
    let mut cumulative: Vec<Vec<u32>> = vec![Vec::new(); depth as usize + 1];
    let mut rows: Vec<(String, &str)> = cadence_engine::bench::positions()
        .into_iter()
        .enumerate()
        .map(|(i, f)| (format!("bench{}", i + 1), f))
        .collect();
    rows.extend(CANDIDATES.iter().map(|&(n, f)| (n.to_string(), f)));
    for (name, fen) in rows {
        tt.clear();
        let board = Board::from_fen(fen).expect("a position");
        let mut pos = match net {
            Some(net) => Position::with_net(board, net.clone()),
            None => Position::new(board),
        };
        let mut search = Search::new(&stop, &tt);
        search.set_limits(Limits::depth(depth));
        search.run(&mut pos, &mut std::io::sink());
        let roots = search.iteration_roots();
        let shares = search.iteration_shares();
        let mut run = 0;
        for (d, (&(mv, _), &(share, total))) in roots.iter().zip(shares).enumerate() {
            let changed = d > 0 && roots[d - 1].0 != mv;
            run = if changed || d == 0 { 1 } else { run + 1 };
            println!(
                "SHARE {name} depth {} share {share} cumulative {total} run {run} changed {}",
                d + 1,
                u8::from(changed)
            );
            if name.starts_with("bench") {
                by_depth[d + 1].push(share);
                cumulative[d + 1].push(total);
            }
        }
        println!("NODES {name} {}", search.nodes());
    }
    for (label, table) in [("DEPTH", by_depth), ("CUMULATIVE", cumulative)] {
        for (d, mut v) in table.into_iter().enumerate().skip(1) {
            if v.is_empty() {
                continue;
            }
            v.sort_unstable();
            let q = |p: usize| v[(v.len() - 1) * p / 4];
            println!(
                "{label} {d} n {} q1 {} median {} q3 {} min {} max {}",
                v.len(),
                q(1),
                q(2),
                q(3),
                v[0],
                v[v.len() - 1]
            );
        }
    }
}

fn depth() -> u32 {
    std::env::var("SHARE_DEPTH")
        .ok()
        .and_then(|d| d.parse().ok())
        .unwrap_or(16)
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

#[test]
#[ignore = "a reading"]
fn share_with_the_net() {
    let net = AnyNetwork::shipped(EMBEDDED)
        .expect("a shipped net")
        .expect("it loads");
    on_big_stack(|| read(Some(&net), depth()));
}

#[test]
#[ignore = "a reading"]
fn share_with_the_handcrafted_evaluation() {
    on_big_stack(|| read(None, depth()));
}
