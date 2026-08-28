// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence bench`: the deterministic regression detector.
//!
//! A fixed-depth search over a fixed, checked-in position set, single
//! thread, printing one line per position and a summary whose last line is
//! `<nodes> nodes <nps> nps`. The node count is a function of the code
//! alone, and every part of that is deliberate:
//!
//! - The position list (`bench_positions.txt`, compiled in) and the depth
//!   are in the repository, not on the command line. There are no
//!   arguments.
//! - One thread: the search runs on the thread that calls it.
//! - Fresh search state per position, and **the transposition table is
//!   cleared between positions**. Nothing carries from one position to the
//!   next, so the total does not depend on the order the positions are
//!   run in or on what ran before them.
//! - A fixed table size, [`HASH_MB`], compiled in rather than passed on
//!   the command line: the node count is a function of it, so it is
//!   recorded in the repository like the depth and the position list.
//! - No time budget: the limits are a depth and nothing else, so the
//!   search never reads the clock on a decision path (`time::budget` is
//!   `None` for a depth limit, and `tests/time.rs` pins that). The clock
//!   is read once, around the whole run, for the nps figure, which is
//!   reported and decides nothing.
//! - No hash map, no float, anywhere on the path (the search's contract).
//!
//! `bench.txt` at the repository root holds the expected count. The
//! `Bench: <n>` trailer on any commit that changes it is enforced by
//! `.githooks/commit-msg`, CI diffs the last line against the file, and
//! `tests/bench.rs` does the same locally.

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
/// Seven. It was three under the new quiescence search, five once the
/// capture ordering made three too short to time, six once the ordering of
/// the check evasions had taken depth five to about 200 ms on the M5 Max,
/// and seven when refusing losing captures took depth six to 7,691,290
/// nodes and about 690 ms, the same list at seven reading 33,848,419 and
/// about 2,400 ms. The depth goes up again as the search gets cheaper per
/// ply, each time with a `Bench:` trailer.
///
/// **Every count in this comment is a reading at the commit that made the
/// change it describes, and none of them is the count today**, which is
/// whatever `bench.txt` holds: each promotion since has moved it, which is
/// what the detector is for. They are kept because what they justify is the
/// choice of depth, and that argument is about the readings that were in
/// hand when the choice was made. The table below is the same kind of
/// figure and is read the same way.
///
/// **What decides it is a measurement, and it is not a measurement of the
/// search.** The SPRT harness scales the time control by the speed it
/// measures from this run, so the
/// depth chooses the length of the window that speed is read over, and a
/// short window does not only read noisily, it reads low: the run ends
/// before the machine has settled. This binary, three runs at each depth,
/// on an otherwise idle M5 Max, against the binary it was match-tested
/// against, which is the previously promoted version at its own
/// compiled-in depth of six:
///
/// | binary | depth | wall clock | reported |
/// |---|---|---|---|
/// | this | 6 | 680-696 ms | 11.05 to 11.31 Mnps |
/// | this | 7 | 2,344-2,386 ms | 14.19 to 14.44 Mnps |
/// | previous | 6 | 1,105-1,116 ms | 13.81 to 13.95 Mnps |
/// | previous | 7 | 3,247-3,270 ms | 15.58 to 15.69 Mnps |
///
/// **The lower two rows are not a reading of this tree**, which is worth
/// saying beside them: they are the other binary's, kept as a binary, and
/// nothing built from this source reproduces them. The upper two are this
/// tree's own. The four are quoted together because the comparison is the
/// argument and neither pair makes it alone.
///
/// Read at depth six this binary is 20% slower than the previous one; read
/// at depth seven, like for like, it is 8.4% slower, and the rest of the
/// 20% is the length of the run. Under `scale_method = BOTH` each side's
/// clock is the nominal one scaled by the reference over its own reading,
/// so a side that reads low is handed a longer clock: at six this side
/// would have played about a quarter longer for a difference that is
/// mostly the window. At seven the two readings are within 4%.
/// That is why seven, and why the mismatch in run length that six would
/// have avoided (2.4 s against 1.1 s) is the lesser evil: `BOTH` accounts
/// for a length mismatch, while nothing corrects a biased reading.
///
/// The gate in `tests/bench.rs` runs this six times and pays for the depth
/// as well: about 4.6 s at six, about 9 s at seven in the test profile.
pub const DEPTH: u32 = 7;

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
/// thread, fresh search state per position, `info` output discarded.
///
/// # Panics
///
/// If a position in the checked-in list does not parse, which
/// `tests/bench.rs` rules out before this can be reached; or if a table of
/// [`HASH_MB`] mebibytes cannot be allocated, which is sixteen mebibytes
/// and a machine that cannot find them cannot run the engine either.
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
