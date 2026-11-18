// SPDX-License-Identifier: GPL-3.0-or-later

//! A node limit that stops an iteration after one has completed. The unfinished iteration's move is
//! played only where its score beat the floor of the window it was searched in.

mod support;

use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

use cadence_core::Move;
use cadence_engine::score::{INFINITE, Score};
use cadence_engine::search::Limits;
use support::table;

/// Middlegames whose best move changes between iterations, so some stops land after a change.
const FENS: [&str; 8] = [
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    "rnbqkb1r/1p2pppp/p2p1n2/8/3NP3/2N5/PPP2PPP/R1BQKB1R w KQkq - 0 6",
    "r1bqk1nr/pppp1ppp/2n5/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4",
    "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14",
    "r1bq1rk1/pp2bppp/2n1pn2/3p4/2PP4/2N1PN2/PP3PPP/R1BQKB1R w KQ - 0 8",
    "rnbqkb1r/ppp2ppp/4pn2/3p4/2PP4/2N5/PP2PPPP/R1BQKBNR w KQkq - 0 4",
    "r1b2rk1/2q1bppp/p2p1n2/1p2p3/3nP3/1BN1BN2/PPP2PPP/R2QR1K1 w - - 0 13",
];

/// Limits growing by a tenth from 2,000 nodes, so stops fall densely at every depth from the first
/// aspirated one to about the tenth.
const LIMITS: usize = 48;

struct Stopped {
    fen: &'static str,
    nodes: u64,
    played: Move,
    pv: Vec<Move>,
    completed: Move,
    /// The unfinished iteration's best fully searched move, its score and its window's floor.
    unfinished: (Move, Score, Score),
}

impl Stopped {
    fn proven(&self) -> bool {
        let (_, score, floor) = self.unfinished;
        score > floor
    }

    /// A move finished, but its score is a bound at or under the floor, as a fail low leaves.
    fn unproven(&self) -> bool {
        let (_, score, floor) = self.unfinished;
        score > -INFINITE && score <= floor
    }
}

/// Searched once and shared, since every gate here reads the same stops.
fn stopped() -> &'static [Stopped] {
    static RUNS: OnceLock<Vec<Stopped>> = OnceLock::new();
    RUNS.get_or_init(|| {
        let mut runs = Vec::new();
        for fen in FENS {
            let mut nodes = 2_000u64;
            for _ in 0..LIMITS {
                if let Some(run) = stop_at(fen, nodes) {
                    runs.push(run);
                }
                nodes += nodes / 10;
            }
        }
        runs
    })
}

/// `None` where the limit fell inside the first iteration, where no completed move stands.
fn stop_at(fen: &'static str, nodes: u64) -> Option<Stopped> {
    let stop = AtomicBool::new(false);
    let tt = table();
    let limits = Limits {
        nodes: Some(nodes),
        ..Limits::default()
    };
    let mut search = support::search(limits, &stop, &tt);
    let played = search.run(&mut support::position(fen), &mut std::io::sink());
    let unfinished = search.unfinished_iteration()?;
    let &(completed, _) = search.iteration_roots().last()?;
    Some(Stopped {
        fen,
        nodes,
        played,
        pv: search.pv().to_vec(),
        completed,
        unfinished,
    })
}

/// The first half of the rule, seen where it changes the move: a stopped iteration whose move beat
/// its window's floor is played, its line with it.
#[test]
fn a_stopped_search_plays_the_unfinished_move_where_it_beat_the_floor() {
    let proven: Vec<&Stopped> = stopped().iter().filter(|run| run.proven()).collect();
    assert!(
        proven.iter().any(|run| run.unfinished.0 != run.completed),
        "no stop of {} proved a move the completed iteration did not play",
        stopped().len()
    );
    for run in proven {
        assert_eq!(
            run.played, run.unfinished.0,
            "{} at {} nodes: completed {:?}",
            run.fen, run.nodes, run.completed
        );
        assert_eq!(run.pv.first(), Some(&run.played), "{}", run.fen);
    }
}

/// The second half, seen where it decides: a move that finished at or under the floor is a fail
/// low's bound, so the completed iteration's move is kept, its line with it.
#[test]
fn a_stopped_search_keeps_the_finished_move_where_the_unfinished_one_did_not_beat_the_floor() {
    let refused: Vec<&Stopped> = stopped().iter().filter(|run| !run.proven()).collect();
    assert!(
        refused
            .iter()
            .any(|run| run.unproven() && run.unfinished.0 != run.completed),
        "no stop of {} fell in a fail low on a move the completed iteration did not play",
        stopped().len()
    );
    for run in refused {
        assert_eq!(
            run.played, run.completed,
            "{} at {} nodes: unfinished {:?}",
            run.fen, run.nodes, run.unfinished
        );
        assert_eq!(run.pv.first(), Some(&run.played), "{}", run.fen);
    }
}
