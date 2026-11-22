// SPDX-License-Identifier: GPL-3.0-or-later

//! A limit that stops an iteration after one has completed, in process and through the binary. The
//! unfinished iteration's move is played only where its score beat the floor of the window it was
//! searched in.

mod support;

use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

use cadence_core::Move;
use cadence_engine::score::{INFINITE, Score};
use cadence_engine::search::Limits;
use support::{Engine, table};

/// Middlegames whose best move changes between iterations, and opening traps whose bait is refuted
/// only deeper, so some stops land after a change and some inside the fail low a refutation brings.
const FENS: [&str; 11] = [
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    "rnbqkb1r/1p2pppp/p2p1n2/8/3NP3/2N5/PPP2PPP/R1BQKB1R w KQkq - 0 6",
    "r1bqk1nr/pppp1ppp/2n5/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4",
    "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14",
    "r1bq1rk1/pp2bppp/2n1pn2/3p4/2PP4/2N1PN2/PP3PPP/R1BQKB1R w KQ - 0 8",
    "rnbqkb1r/ppp2ppp/4pn2/3p4/2PP4/2N5/PP2PPPP/R1BQKBNR w KQkq - 0 4",
    "r1b2rk1/2q1bppp/p2p1n2/1p2p3/3nP3/1BN1BN2/PPP2PPP/R2QR1K1 w - - 0 13",
    // Taking on d5 wins a pawn until the bishop check that costs a piece is seen.
    "r1bqkb1r/pppn1ppp/5n2/3p2B1/3P4/2N5/PP2PPPP/R2QKBNR w KQkq - 0 6",
    // Taking on d4 wins a pawn until the pawns trap the bishop.
    "r1bqkbnr/2p2ppp/p2p4/1p6/3pP3/1B6/PPP2PPP/RNBQK2R w KQkq - 0 8",
    // Taking on e5 wins a pawn until the queen reaches g2.
    "r1bqkbnr/pppp1ppp/8/4p3/2BnP3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4",
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

/// The second half: a stop the rule leaves alone keeps the completed iteration's move and its line,
/// and some stops fall inside a fail low, where a move finished at or under the floor.
#[test]
fn a_stopped_search_keeps_the_finished_move_where_the_unfinished_one_did_not_beat_the_floor() {
    let refused: Vec<&Stopped> = stopped().iter().filter(|run| !run.proven()).collect();
    assert!(
        refused.iter().any(|run| run.unproven()),
        "no stop of {} fell in a fail low",
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

/// Every third limit of the ladder, so the binary meets stops at the same depths in fewer searches.
const UCI_STEP: usize = 3;

/// Short enough to stop mid-iteration at every depth a debug build reaches, long enough that one
/// iteration completes first.
const MOVETIMES: [u64; 3] = [60, 150, 300];

/// What an `info depth` line with a `pv` says.
struct Info {
    depth: u32,
    lower: bool,
    first: String,
}

fn infos(lines: &[String]) -> Vec<Info> {
    lines
        .iter()
        .filter(|l| l.starts_with("info depth "))
        .filter_map(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            let pv = toks.iter().position(|t| *t == "pv")?;
            Some(Info {
                depth: toks[2].parse().expect("a depth"),
                lower: toks.contains(&"lowerbound"),
                first: (*toks.get(pv + 1)?).to_string(),
            })
        })
        .collect()
}

/// One search from a fresh table, as `stop_at` runs in process: every line printed and the move
/// played.
fn search_through_binary(e: &mut Engine, fen: &str, go: &str) -> (Vec<Info>, String) {
    e.send("ucinewgame");
    e.send(&format!("position fen {fen}"));
    e.send(go);
    let lines = e.read_until("bestmove ");
    let played = support::bestmove(&lines.join("\n"));
    (infos(&lines), played)
}

/// Through the binary, the last line printed names the move played, and where a stopped iteration
/// replaced the completed move that line keeps the completed depth and calls its score a lower
/// bound.
#[test]
fn the_last_info_line_names_the_move_played_after_a_node_limit() {
    let mut e = Engine::spawn();
    let mut replaced = 0;
    for fen in FENS {
        let mut nodes = 2_000u64;
        for i in 0..LIMITS {
            if i % UCI_STEP == 0 {
                let (infos, played) =
                    search_through_binary(&mut e, fen, &format!("go nodes {nodes}"));
                if let Some(last) = infos.last() {
                    assert_eq!(last.first, played, "{fen} at {nodes} nodes");
                    if last.lower {
                        let completed = infos.iter().rev().nth(1).expect("a completed line");
                        assert_eq!(last.depth, completed.depth, "{fen} at {nodes} nodes");
                        replaced += usize::from(completed.first != last.first);
                    }
                }
            }
            nodes += nodes / 10;
        }
    }
    e.quit();
    assert!(
        replaced > 0,
        "no stop replaced the completed move with a different one"
    );
}

/// Under `movetime` a search ends at its hard deadline inside an iteration, the path a clock takes.
/// Timing decides where, so this asserts only that the last line names the move played.
#[test]
fn the_last_info_line_names_the_move_played_after_a_movetime() {
    let mut e = Engine::spawn();
    for fen in FENS {
        for ms in MOVETIMES {
            let (infos, played) = search_through_binary(&mut e, fen, &format!("go movetime {ms}"));
            let last = infos.last().expect("a completed iteration's line");
            assert_eq!(last.first, played, "{fen} at {ms} ms");
        }
    }
    e.quit();
}
