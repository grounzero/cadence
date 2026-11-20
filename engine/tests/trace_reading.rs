// SPDX-License-Identifier: GPL-3.0-or-later

//! A reading, not a gate: the bench under each store rule after a fail-low, with the king and queen
//! position's counters, printed for CI.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_engine::bench::{DEPTH, HASH_MB, positions};
use cadence_engine::position::Position;
use cadence_engine::search::{Limits, Search, set_mode, spread_take};
use cadence_engine::tt::Table;
use cadence_engine::tune::Tunables;
use cadence_nnue::Network;
use cadence_nnue::net::EMBEDDED;

const KQK: &str = "8/2k5/8/8/8/8/1Q6/K7 w - - 0 1";

fn read(mode: u8) {
    set_mode(mode);
    let net = Arc::new(Network::shipped(EMBEDDED).expect("a net").expect("it loads"));
    let stop = AtomicBool::new(false);
    let tt = Table::new(HASH_MB).expect("a table");
    let mut total = 0;
    let mut per = Vec::new();
    for fen in positions() {
        tt.clear();
        let board = Board::from_fen(fen).expect("a position");
        let mut pos = Position::with_net(board, Arc::clone(&net));
        let mut s = Search::new(&stop, &tt);
        s.set_limits(Limits::depth(DEPTH));
        s.set_tunables(Tunables::DEFAULT);
        let _ = spread_take();
        let best = s.run(&mut pos, &mut std::io::sink());
        let spread = spread_take();
        total += s.nodes();
        per.push(s.nodes());
        if fen.starts_with(KQK) || per.len() == 23 {
            let at = |i: usize| spread.get(i).copied().unwrap_or(0);
            println!(
                "MODE {mode} POS {} {fen} best {best:?} nodes {}",
                per.len(),
                s.nodes()
            );
            println!(
                "MODE {mode} POS {} failow_held_differs {} held_check {} best_check {} held_check_best_not {} gap<=10 {} gap<=50 {} gap<=200 {} gap>200 {} held_unsearched {} reopened {} first {} first_checks {} cut_first {} cut_later {}",
                per.len(),
                at(110),
                at(111),
                at(112),
                at(113),
                at(114),
                at(115),
                at(116),
                at(117),
                at(119),
                at(97),
                at(100),
                at(99),
                at(104),
                at(105)
            );
        }
    }
    println!("MODE {mode} TOTAL {total}");
    println!("MODE {mode} PER {per:?}");
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
fn store_rules() {
    let mode: u8 = std::env::var("TRACE_MODE")
        .ok()
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    on_big_stack(move || read(mode));
}
