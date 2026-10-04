// SPDX-License-Identifier: GPL-3.0-or-later

//! The node count must be a function of the code alone: single thread, fixed depth, a fixed table
//! cleared between positions, the list and depth compiled in, and no clock on a decision path.

use std::io::Write;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use cadence_core::Move;
use cadence_core::position::Board;

use crate::position::Position;
use crate::search::{Limits, Search};
use crate::tt::Table;
use crate::tune::Tunables;

/// What the STC preset passes.
pub const HASH_MB: usize = 16;

/// The SPRT harness scales every time control by the speed it measures from this run, so a short
/// window reads low and hands the other side a longer clock. Changing this changes the detector.
pub const DEPTH: u32 = 13;

/// One FEN per line, `#` for comments.
pub const POSITIONS: &str = include_str!("../bench_positions.txt");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub fen: String,
    pub best: Move,
    pub nodes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<Line>,
    pub nodes: u64,
    pub millis: u64,
}

#[must_use]
pub fn positions() -> Vec<&'static str> {
    POSITIONS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// # Panics
///
/// If a checked-in position does not parse, or a table of [`HASH_MB`] mebibytes cannot be
/// allocated.
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
        let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("bench position {fen}: {e:?}"));
        let mut pos = Position::new(board);
        let mut search = Search::new(&stop, &tt);
        search.set_limits(Limits::depth(DEPTH));
        // Never a setting: a tunable that reached here would make the count depend on an option.
        search.set_tunables(Tunables::DEFAULT);
        let best = search.run(&mut pos, &mut std::io::sink());
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

/// The summary's last line is `<nodes> nodes <nps> nps`. Takes no arguments, by design.
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
