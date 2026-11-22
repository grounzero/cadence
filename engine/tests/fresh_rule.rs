// SPDX-License-Identifier: GPL-3.0-or-later

//! The bench reads no clock, so it cannot see a time decision change. This holds every search
//! without a hit to the rule from before a hit could move the deadlines, over the bench positions'
//! own iteration ladders.

use std::sync::atomic::AtomicBool;

use cadence_core::Colour;
use cadence_core::position::Board;
use cadence_engine::bench::{DEPTH, HASH_MB, positions};
use cadence_engine::position::Position;
use cadence_engine::search::{Limits, Search};
use cadence_engine::time::{Budget, another_iteration_fits, budget, soft_for_run, starts_another};
use cadence_engine::tt::Table;
use cadence_nnue::AnyNetwork;
use cadence_nnue::net::EMBEDDED;

/// Early to late in 3+2 after a bridge's 2 s, sudden death, and a fixed time a move.
const CLOCKS: [&str; 6] = [
    "wtime 178000 btime 178000 winc 2000 binc 2000",
    "wtime 30000 btime 30000 winc 2000 binc 2000",
    "wtime 7700 btime 7700 winc 2000 binc 2000",
    "wtime 6000 btime 6000 winc 2000 binc 2000",
    "wtime 6000 btime 6000",
    "movetime 200",
];

/// Nodes a millisecond, so a ladder at bench depth meets each budget at several of its iterations.
const RATES: [u64; 5] = [2, 10, 50, 200, 1000];

/// The rule as it stood before a hit could move the deadlines.
fn before(limits: &Limits, b: Budget, completed: &[u64], run: usize) -> bool {
    let elapsed = *completed.last().expect("a completed iteration");
    let stops = elapsed >= soft_for_run(limits, b, run) || !another_iteration_fits(completed, b);
    !stops
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let mut it = line.split_whitespace();
    it.find(|t| *t == key)?;
    it.next()
}

/// Each completed iteration's node count and best move, from its `info` line.
fn ladder(fen: &str, net: Option<&AnyNetwork>, tt: &Table) -> Vec<(u64, String)> {
    tt.clear();
    let stop = AtomicBool::new(false);
    let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("bench position {fen}: {e:?}"));
    let mut pos = match net {
        Some(net) => Position::with_net(board, net.clone()),
        None => Position::new(board),
    };
    let mut search = Search::new(&stop, tt);
    search.set_limits(Limits::depth(DEPTH));
    let mut out = Vec::new();
    let _ = search.run(&mut pos, &mut out);
    String::from_utf8_lossy(&out)
        .lines()
        .filter(|l| l.starts_with("info depth "))
        .map(|l| {
            let nodes = field(l, "nodes")
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("no nodes in {l:?}"));
            let best = field(l, "pv").unwrap_or_else(|| panic!("no pv in {l:?}"));
            (nodes, best.to_string())
        })
        .collect()
}

#[test]
fn a_search_without_a_hit_stops_where_it_did_over_the_bench_ladders() {
    let net = AnyNetwork::shipped(EMBEDDED).map(|n| n.expect("the net this build ships"));
    let tt = Table::new(HASH_MB).expect("a bench-sized table");
    let ladders: Vec<Vec<(u64, String)>> = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn_scoped(s, || {
                positions()
                    .into_iter()
                    .map(|fen| ladder(fen, net.as_ref(), &tt))
                    .collect()
            })
            .expect("a search thread")
            .join()
            .expect("the ladders")
    });
    assert_eq!(ladders.len(), positions().len());
    let (mut stops, mut goes) = (0, 0);
    for spec in CLOCKS {
        let l = Limits::parse(spec.split_whitespace());
        let b = budget(&l, Colour::White).expect("a budget");
        for (i, steps) in ladders.iter().enumerate() {
            for rate in RATES {
                let times: Vec<u64> = steps.iter().map(|(n, _)| n / rate).collect();
                for k in 1..=steps.len() {
                    let last = &steps[k - 1].1;
                    let run = steps[..k].iter().rev().take_while(|s| &s.1 == last).count();
                    let old = before(&l, b, &times[..k], run);
                    assert_eq!(
                        starts_another(&l, b, &times[..k], run, 1000, 0),
                        old,
                        "`{spec}`, bench position {}, {rate} nodes a ms, after iteration {k} of \
                         {times:?}",
                        i + 1
                    );
                    if old {
                        goes += 1;
                    } else {
                        stops += 1;
                    }
                }
            }
        }
    }
    assert!(
        stops > 0 && goes > 0,
        "the ladders met only one decision ({stops} stops, {goes} goes), so they compared nothing"
    );
}
