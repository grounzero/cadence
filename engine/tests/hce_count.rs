// SPDX-License-Identifier: GPL-3.0-or-later

//! The handcrafted evaluation's node count over the bench positions at two depths, printed by
//! failing. A diagnostic, never for merging.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_engine::bench;
use cadence_engine::search::{Limits, Search};
use cadence_engine::tt::Table;

#[test]
fn the_handcrafted_count_over_the_bench_positions() {
    let stop = AtomicBool::new(false);
    let tt = Table::new(bench::HASH_MB).expect("a bench-sized table");
    let mut shown = Vec::new();
    for depth in [8, 9] {
        let mut total = 0;
        for fen in bench::positions() {
            tt.clear();
            let mut board = support::position(fen);
            let mut search = Search::new(&stop, &tt);
            search.set_limits(Limits::depth(depth));
            search.run(&mut board, &mut std::io::sink());
            total += search.nodes();
        }
        shown.push(format!("depth {depth}: {total} nodes"));
    }
    panic!("handcrafted: {}", shown.join(", "));
}
