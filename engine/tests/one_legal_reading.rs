// SPDX-License-Identifier: GPL-3.0-or-later

//! A reading, not a gate: the iteration ladder of a root with one legal move and of the position
//! after it, each stopped after a fixed wall time, with the net the build ships or the handcrafted one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_engine::position::Position;
use cadence_engine::search::{Limits, Search};
use cadence_engine::tt::Table;
use cadence_nnue::AnyNetwork;
use cadence_nnue::net::EMBEDDED;

const ROWS: [(&str, &str); 3] = [
    ("one-legal", "4k3/8/8/8/8/4p3/3q4/4K2R w K - 0 1"),
    ("after-kf1", "4k3/8/8/8/8/4p3/3q4/5K1R b - - 1 1"),
    ("two-recaptures", "4k3/8/8/8/3q4/1N3N2/PPP3PP/4K3 w - - 0 1"),
];

fn read(net: Option<&AnyNetwork>) {
    let wall = Duration::from_secs(600);
    for (name, fen) in ROWS {
        let stop = AtomicBool::new(false);
        let done = AtomicBool::new(false);
        let tt = Table::new(16).expect("a table");
        let board = Board::from_fen(fen).expect("a position");
        let mut pos = match net {
            Some(net) => Position::with_net(board, net.clone()),
            None => Position::new(board),
        };
        println!("POSITION {name}");
        let start = Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| {
                while !done.load(Ordering::Relaxed) && start.elapsed() < wall {
                    std::thread::sleep(Duration::from_millis(200));
                }
                stop.store(true, Ordering::Relaxed);
            });
            let mut search = Search::new(&stop, &tt);
            search.set_limits(Limits::depth(40));
            search.run(&mut pos, &mut std::io::stdout());
            done.store(true, Ordering::Relaxed);
            println!(
                "END {name} nodes {} ms {}",
                search.nodes(),
                start.elapsed().as_millis()
            );
        });
    }
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
fn ladder_with_the_net() {
    let net = AnyNetwork::shipped(EMBEDDED)
        .expect("a shipped net")
        .expect("it loads");
    on_big_stack(|| read(Some(&net)));
}

#[test]
#[ignore = "a reading"]
fn ladder_with_the_handcrafted_evaluation() {
    on_big_stack(|| read(None));
}
