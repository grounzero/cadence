// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence perft [--divide] [--threads N] <fen | startpos> <depth>` The corpus from the
//! command line. Root-parallel: the root's legal moves are dealt out to worker threads, each
//! with its own `Board` and nothing shared, and the totals are summed.

use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Instant;

use cadence_core::position::Board;
use cadence_core::{generate_legal, perft};

pub const USAGE: &str = "usage: cadence perft [--divide] [--threads N] <fen | startpos> <depth>";

const STARTPOS: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Parsed arguments. The FEN is whatever lies between the flags and the final depth token,
/// joined with spaces, so it needs no quoting.
struct Args {
    divide: bool,
    threads: usize,
    fen: String,
    depth: u32,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut divide = false;
    let mut threads = std::thread::available_parallelism().map_or(1, usize::from);
    let mut rest: Vec<&str> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--divide" => divide = true,
            "--threads" => {
                let n = it.next().ok_or("--threads needs a number")?;
                threads = n
                    .parse()
                    .map_err(|_| format!("--threads: `{n}` is not a number"))?;
                if threads == 0 {
                    return Err("--threads must be at least 1".to_string());
                }
            }
            other => rest.push(other),
        }
    }
    let (depth, fen) = rest.split_last().ok_or("missing <depth>")?;
    let depth: u32 = depth
        .parse()
        .map_err(|_| format!("`{depth}` is not a depth"))?;
    if fen.is_empty() {
        return Err("missing <fen>".to_string());
    }
    let fen = if fen == ["startpos"] {
        STARTPOS.to_string()
    } else {
        fen.join(" ")
    };
    Ok(Args {
        divide,
        threads,
        fen,
        depth,
    })
}

#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    let args = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("cadence perft: {e}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let root = match Board::from_fen(&args.fen) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cadence perft: FEN rejected ({e:?}): {}", args.fen);
            return ExitCode::from(2);
        }
    };

    // Counted, not refused. The position is one no legal play can reach and its perft is not a
    // standard quantity, but it is well defined -- a king is never a target, so the recursion
    // terminates on the same rule as any other position -- and refusing a FEN a user typed on
    // purpose helps nobody.
    if root.opponent_in_check() {
        eprintln!(
            "cadence perft: the side not to move is in check; no legal play reaches this position"
        );
    }

    let start = Instant::now();
    let mut rows = root_split(&root, &args.fen, args.depth, args.threads);
    let elapsed = start.elapsed();

    if args.divide {
        rows.sort();
        for (mv, n) in &rows {
            println!("{mv}: {n}");
        }
    }
    let nodes: u64 = rows.iter().map(|(_, n)| n).sum();
    let millis = elapsed.as_millis();
    #[allow(clippy::cast_precision_loss, reason = "display only")]
    let nps = if millis == 0 {
        0
    } else {
        (nodes as f64 / elapsed.as_secs_f64()) as u64
    };
    println!("nodes {nodes}");
    println!("time {millis} ms");
    println!("nps {nps}");
    ExitCode::SUCCESS
}

/// Perft split at the root across `threads` workers. Depth 0 is one node and no moves; depth 1
/// is the root move list, counted in bulk.
fn root_split(root: &Board, fen: &str, depth: u32, threads: usize) -> Vec<(String, u64)> {
    if depth == 0 {
        return vec![("-".to_string(), 1)];
    }
    let moves: Vec<_> = generate_legal(root).iter().collect();
    let threads = threads.min(moves.len()).max(1);

    // Each worker takes the next unclaimed root move until none are left: cheap dynamic
    // balancing, since root subtrees vary widely in size.
    let next = Mutex::new(0usize);
    let results: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::with_capacity(moves.len()));
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut board = Board::from_fen(fen).expect("the root FEN parsed once already");
                loop {
                    let i = {
                        let mut n = next.lock().expect("claim lock");
                        let i = *n;
                        *n += 1;
                        i
                    };
                    let Some(&m) = moves.get(i) else { break };
                    board.make_move(m);
                    let n = perft(&mut board, depth - 1);
                    board.unmake_move(m);
                    results
                        .lock()
                        .expect("results lock")
                        .push((m.to_uci_chess960(), n));
                }
            });
        }
    });
    results.into_inner().expect("results lock")
}
