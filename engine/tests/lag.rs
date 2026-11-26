// SPDX-License-Identifier: GPL-3.0-or-later

//! The session measures what a move loses beyond its own thinking from the clocks `go` gives,
//! and only where the two clocks are the same side's in one game.

use cadence_engine::uci::Session;

/// The most one move's reading counts for, so a single stall does not empty the reserve.
const MOST: u64 = 500;

fn session() -> Session {
    let mut s = Session::new();
    s.handle_line("ucinewgame");
    s
}

/// 1,000 ms became 200 over a move stopped at once, far more than the thinking took, so the
/// reading is the most one move counts for.
#[test]
fn the_lag_is_what_the_clock_lost_beyond_the_thinking() {
    let mut s = session();
    s.handle_line("position startpos");
    s.handle_line("go wtime 1000 btime 1000");
    s.handle_line("position startpos moves e2e4 e7e5");
    s.handle_line("go wtime 200 btime 1000");
    s.handle_line("stop");
    assert_eq!(s.measured_lag(), Some(MOST));
    s.handle_line("ucinewgame");
    assert_eq!(s.measured_lag(), None, "a new game starts unmeasured");
}

/// A clock that lost nothing past the thinking reads as no lag.
#[test]
fn a_clock_that_lost_only_the_thinking_reads_as_no_lag() {
    let mut s = session();
    s.handle_line("position startpos");
    s.handle_line("go wtime 1000 btime 1000");
    s.handle_line("position startpos moves e2e4 e7e5");
    s.handle_line("go wtime 1000 btime 1000");
    s.handle_line("stop");
    assert_eq!(s.measured_lag(), Some(0));
}

/// The next clock is the other side's, so it says nothing of this side's move.
#[test]
fn the_other_side_s_clock_measures_nothing() {
    let mut s = session();
    s.handle_line("position startpos");
    s.handle_line("go wtime 1000 btime 1000");
    s.handle_line("position startpos moves e2e4");
    s.handle_line("go wtime 1000 btime 200");
    s.handle_line("stop");
    assert_eq!(s.measured_lag(), None);
}

/// A ponder that was not hit was never a move played, so its time is no reading.
#[test]
fn a_ponder_that_missed_measures_nothing() {
    let mut s = session();
    s.handle_line("setoption name Ponder value true");
    s.handle_line("position startpos moves e2e4 e7e5");
    s.handle_line("go ponder wtime 1000 btime 1000");
    s.handle_line("stop");
    s.handle_line("position startpos moves e2e4 e7e5 g1f3 b8c6");
    s.handle_line("go wtime 200 btime 1000");
    s.handle_line("stop");
    assert_eq!(s.measured_lag(), None);
}
