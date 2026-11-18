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
const FENS: [&str; 5] = [
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    "rnbqkb1r/1p2pppp/p2p1n2/8/3NP3/2N5/PPP2PPP/R1BQKB1R w KQkq - 0 6",
    "r1bqk1nr/pppp1ppp/2n5/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4",
    "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14",
];

/// Limits growing by a fifth from 2,000 nodes, so stops fall at every depth from the first
/// aspirated one to about the tenth.
const LIMITS: usize = 24;

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
                nodes += nodes / 5;
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

#[test]
fn an_unfinished_iteration_s_move_is_played_only_where_it_beat_its_floor() {
    let runs = stopped();
    assert!(
        !runs.is_empty(),
        "no limit stopped an iteration after one completed"
    );
    for run in runs {
        let (fen, nodes) = (run.fen, run.nodes);
        let expected = if run.proven() {
            run.unfinished.0
        } else {
            run.completed
        };
        assert_eq!(
            run.played,
            expected,
            "{fen} at {nodes} nodes: unfinished {:?}, completed {:?}",
            run.unfinished,
            run.completed
        );
        assert_eq!(run.pv.first(), Some(&run.played), "{fen} at {nodes} nodes");
    }
}

/// The rule is not inert on this sample: some stop plays a move the completed iteration did not.
#[test]
fn some_stop_plays_a_proven_move_the_completed_iteration_did_not() {
    let changed = stopped()
        .iter()
        .filter(|run| run.proven() && run.played != run.completed)
        .count();
    assert!(
        changed > 0,
        "no stop of {} played a proven new move",
        stopped().len()
    );
}

/// The case the floor is for: a move finished under it, which is a bound and not a score.
#[test]
fn some_stop_refuses_a_move_that_finished_under_its_floor() {
    let refused: Vec<&Stopped> = stopped().iter().filter(|run| run.unproven()).collect();
    assert!(
        !refused.is_empty(),
        "no stop of {} fell in a fail low",
        stopped().len()
    );
    for run in refused {
        assert_eq!(
            run.played,
            run.completed,
            "{} at {} nodes",
            run.fen,
            run.nodes
        );
    }
}
