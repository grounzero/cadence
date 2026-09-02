// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence bench`: the deterministic regression detector.
//!
//! A fixed-depth search over a checked-in position list whose node count
//! must be a function of the code alone: single thread, fixed depth, a fixed
//! table cleared between positions, the list and the depth compiled in
//! rather than passed on a command line, and no clock on any decision path.
//!
//! **A commit that changes the count declares it**, and the declared figure
//! and `bench.txt` are diffed against each other on both architectures.

use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use cadence_core::Move;
use cadence_core::position::Board;

use crate::search::{Limits, Search};
use crate::tt::Table;

/// The transposition table size the bench runs with, in mebibytes.
///
/// Sixteen, which is what the STC preset passes and what the engine
/// defaults to today. It is written here rather than read from
/// `tt::DEFAULT_HASH_MB` on purpose: retuning the default a GUI gets is a
/// change to how the engine plays, and it must not silently move the
/// regression detector as a side effect. If this number changes, the
/// commit that changes it declares a new `Bench:` count like any other.
pub const HASH_MB: usize = 16;

/// The fixed depth every position is searched to.
///
/// **What sets it is not a property of the search.** The SPRT harness scales
/// every workload's time control by the speed it measures from this run, so
/// this depth chooses the length of the window that speed is read over, and
/// a short window does not only read noisily, it reads low: the run ends
/// before the machine has settled, and a side that reads low is handed a
/// longer clock. Under a couple of seconds the reading is not usable, and
/// lowering this to make a test suite faster would bias every clock the
/// harness delivers with nothing saying so.
///
/// **Changing it is changing the detector.** Allowed, and never a side
/// effect of retuning something else; the commit that changes it declares a
/// new `Bench:` count like any other.
pub const DEPTH: u32 = 13;

/// The checked-in position list, one FEN per line, `#` for comments.
pub const POSITIONS: &str = include_str!("../bench_positions.txt");

/// One position's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub fen: String,
    pub best: Move,
    pub nodes: u64,
}

/// The whole run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<Line>,
    pub nodes: u64,
    pub millis: u64,
}

/// The FENs of [`POSITIONS`], comments and blank lines dropped.
#[must_use]
pub fn positions() -> Vec<&'static str> {
    POSITIONS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// Run the bench: every position in [`POSITIONS`] to [`DEPTH`], single
/// thread, from a table of [`HASH_MB`] cleared between positions.
///
/// # Panics
///
/// If a checked-in position does not parse, or if a table of [`HASH_MB`]
/// mebibytes cannot be allocated.
#[must_use]
pub fn bench() -> Report {
    let start = Instant::now();
    let stop = AtomicBool::new(false);
    #[expect(clippy::expect_used, reason = "sixteen mebibytes")]
    let tt = Table::new(HASH_MB).expect("a bench-sized transposition table");
    let mut lines = Vec::new();
    let mut nodes = 0;
    for fen in positions() {
        // The seam: nothing an earlier position learned reaches this one.
        tt.clear();
        let mut board =
            Board::from_fen(fen).unwrap_or_else(|e| panic!("bench position {fen}: {e:?}"));
        let mut search = Search::new(Limits::depth(DEPTH), &stop, &tt);
        let best = search.run(&mut board, &mut std::io::sink());
        nodes += search.nodes();
        lines.push(Line {
            fen: fen.to_string(),
            best,
            nodes: search.nodes(),
        });
    }
    let millis = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    Report {
        lines,
        nodes,
        millis,
    }
}

/// The subcommand: run, print one line per position and the summary, whose
/// last line is `<nodes> nodes <nps> nps`. Takes no arguments, by design.
#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!(
            "cadence bench takes no arguments: the position set and the depth are fixed in the repository"
        );
        return ExitCode::from(2);
    }
    let report = bench();
    let out = std::io::stdout();
    let mut out = out.lock();
    for (i, line) in report.lines.iter().enumerate() {
        let _ = writeln!(
            out,
            "{:>2} {:>10} nodes  {:<6} {}",
            i + 1,
            line.nodes,
            line.best.to_uci_chess960(),
            line.fen
        );
    }
    let nps = report.nodes * 1000 / report.millis.max(1);
    let _ = writeln!(
        out,
        "depth {DEPTH}, hash {HASH_MB} MB, {} positions, {} ms",
        report.lines.len(),
        report.millis
    );
    let _ = writeln!(out, "{} nodes {nps} nps", report.nodes);
    let _ = out.flush();
    ExitCode::SUCCESS
}
