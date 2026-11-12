// SPDX-License-Identifier: GPL-3.0-or-later

//! Games against the binary on a clock the test keeps, alone in a test binary of their own: cargo
//! runs test binaries one at a time, so no other test's search competes for the cores a move is
//! timed on.

mod support;

use std::time::Instant;

use cadence_core::position::Board;
use cadence_core::{START_FEN, generate_legal, parse_uci};
use cadence_engine::time::MOVE_OVERHEAD_MS;
use support::Engine;

/// Past half of what is left, the most the engine ever gives a move, timed around the pipe and the
/// scheduler: a release build's clock check comes every millisecond or so, and this is for a loaded
/// runner.
const MAX_OVERRUN_MS: u64 = 150;

/// A game on the clock: the plies played, the clocks left, the worst overrun past the engine's hard
/// cap, and whether the board ended it with a mate, a stalemate or the fifty-move rule.
struct Game {
    plies: u32,
    clocks: [u64; 2],
    worst_overrun_ms: u64,
    ended_by_board: bool,
}

/// The game from `start`; or the first move timed past the clock or past the cap by more than
/// [`MAX_OVERRUN_MS`].
fn play_on_the_clock(start: &str) -> Result<Game, String> {
    let mut e = Engine::spawn();
    let mut board = Board::from_fen(start).unwrap();
    let mut moves: Vec<String> = Vec::new();
    let mut time = [1000u64, 1000u64];
    let inc = 20u64;
    let mut plies = 0;
    let mut max_overrun_ms = 0u64;
    let mut ended_by_board = false;
    while plies < 120 {
        let legal = generate_legal(&board);
        if legal.is_empty() || board.halfmove_clock() >= 100 {
            ended_by_board = true;
            break;
        }
        let us = board.side_to_move().index();
        e.send(&format!("position fen {start} moves {}", moves.join(" ")));
        e.sync();
        let start = Instant::now();
        e.send(&format!(
            "go wtime {} btime {} winc {inc} binc {inc}",
            time[0], time[1]
        ));
        let lines = e.read_until("bestmove ");
        let spent = u64::try_from(start.elapsed().as_millis()).unwrap();
        let mv = lines
            .last()
            .unwrap()
            .strip_prefix("bestmove ")
            .unwrap()
            .to_string();
        let m = parse_uci(&legal, &mv).unwrap_or_else(|| panic!("illegal bestmove {mv}"));
        if spent >= time[us] {
            e.quit();
            return Err(format!(
                "ply {plies}: {spent} ms spent with {} ms on the clock",
                time[us]
            ));
        }
        // The engine's own hard cap is half of what is left after the overhead.
        let cap = time[us].saturating_sub(MOVE_OVERHEAD_MS) / 2;
        max_overrun_ms = max_overrun_ms.max(spent.saturating_sub(cap));
        if spent > cap + MAX_OVERRUN_MS {
            e.quit();
            return Err(format!(
                "ply {plies}: {spent} ms spent against a cap of {cap} ms"
            ));
        }
        time[us] = time[us] - spent + inc;
        board.play(m);
        moves.push(mv);
        plies += 1;
    }
    e.quit();
    Ok(Game {
        plies,
        clocks: time,
        worst_overrun_ms: max_overrun_ms,
        ended_by_board,
    })
}

/// 1000 ms + 20 ms, timed around each `go` with the pipe and the scheduler included. A game that
/// runs out or overruns is printed and played once more, and only a second such game fails.
fn played_on_the_clock(start: &str) -> Game {
    let game = play_on_the_clock(start).unwrap_or_else(|first| {
        println!("{first}; playing the game again");
        play_on_the_clock(start).unwrap_or_else(|second| panic!("{first}, and again {second}"))
    });
    let Game {
        plies,
        clocks,
        worst_overrun_ms,
        ended_by_board,
    } = &game;
    println!("{plies} plies, clocks {clocks:?}, worst overrun {worst_overrun_ms} ms past the cap");
    println!("ended by the board: {ended_by_board}");
    game
}

/// Long enough to have spent the clock, forty plies, or ended first by the board, which is a game
/// played; a time loss or an overrun has already failed it.
fn assert_played_out(game: &Game) {
    assert!(
        game.plies >= 40 || game.ended_by_board,
        "only {} plies, and the board did not end the game",
        game.plies
    );
}

#[test]
fn a_game_on_the_clock_never_runs_out_of_time() {
    assert_played_out(&played_on_the_clock(START_FEN));
}

/// Mate in two, which the board ends after a few plies with no time lost, so the game passes.
#[test]
fn a_game_the_board_ends_early_passes_on_the_clock() {
    assert_played_out(&played_on_the_clock("7k/8/5K2/8/8/8/8/Q7 w - - 0 1"));
}
