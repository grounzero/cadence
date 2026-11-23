// SPDX-License-Identifier: GPL-3.0-or-later

//! A hit ponder timed against the hard budget it takes, alone in a test binary of its own: cargo
//! runs test binaries one at a time, so no other test's search competes for the cores it is timed
//! on.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};

use cadence_core::position::Board;
use cadence_core::{Colour, START_FEN};
use cadence_engine::position::Position;
use cadence_engine::search::Limits;
use cadence_engine::time::budget;
use cadence_engine::tt::Table;

fn limits(s: &str) -> Limits {
    Limits::parse(s.split_whitespace())
}

/// The hit is raised before the run, so the whole search is what a `ponderhit` leaves: it records a
/// ladder and returns without `stop`.
#[test]
fn a_ponder_that_is_hit_becomes_a_clocked_search() {
    // The clock is read every 1024 nodes, so a runner that stalls between two reads can finish an
    // iteration past the limit by the stall: 9 ms on one Windows runner.
    const STALL_MS: u64 = 100;

    let stop = AtomicBool::new(false);
    let hit = AtomicBool::new(true);
    let tt = Table::new(16).expect("a table");
    let mut board = Position::new(Board::from_fen(START_FEN).expect("the start position"));
    let mut s = support::search(limits("ponder wtime 20000 btime 20000"), &stop, &tt);
    s.set_ponder_hit(&hit);
    let best = s.run(&mut board, &mut Vec::new());

    assert!(!best.is_null(), "no move from a hit ponder");
    assert!(
        !stop.load(Ordering::Relaxed),
        "the test raised stop, so this measured nothing"
    );
    assert!(
        !s.iterations_ms().is_empty(),
        "a hit ponder recorded no iteration, so it never took the clock"
    );
    let budget = budget(&limits("wtime 20000 btime 20000"), Colour::White).expect("a budget");
    assert!(
        s.iterations_ms()
            .last()
            .is_some_and(|&ms| ms <= budget.hard + STALL_MS),
        "the ladder ran past the hard budget it took on the hit: {:?}",
        s.iterations_ms()
    );
}
