// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared by the engine's integration tests.
//!
//! The corpus fixture is read here with a small parser of its own, because the
//! engine crate does not see `core`'s test support. As there, nothing
//! transcribes a FEN or a node count into Rust: the fixture is the only
//! source of expected values, and a test names a row and reads it.

// Each test binary uses a different subset of this module.
#![allow(dead_code)]

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const FIXTURE: &str = include_str!("../../../tests/fixtures/perft-corpus.txt");

/// Pawn-and-king positions where one side stands far enough ahead that the
/// evaluation sits above beta at null-window nodes throughout the tree.
/// Two gates read them for opposite halves of one fact -- null-move pruning
/// refuses these positions, so a search of them tries no null move
/// (`tests/pruning.rs`) and remains a function of the position and the
/// depth alone, independent of the window it is asked in
/// (`tests/search.rs`). Pawns start on their own second and third ranks, so
/// no promotion is reachable inside six plies of the main search and every
/// node of a depth-six subtree is pawn-and-king only.
pub const PAWN_ENDGAMES: [&str; 2] = [
    "4k3/pppp4/8/8/8/8/PPPPPPPP/4K3 w - - 0 1",
    "4k3/pppppppp/8/8/8/8/4PPPP/4K3 b - - 0 1",
];

/// The rows of the fenced TSV block named `name`.
pub fn tsv(name: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut inside = false;
    for line in FIXTURE.lines() {
        if inside {
            if line.trim_start().starts_with("```") {
                break;
            }
            if !line.trim().is_empty() {
                rows.push(line.split('\t').map(|f| f.trim().to_string()).collect());
            }
        } else if line.trim() == format!("```tsv {name}") {
            inside = true;
        }
    }
    assert!(!rows.is_empty(), "no block named `{name}`");
    rows
}

/// The `| n | name | `fen` |` table of section 1.
pub fn standard_fen(name: &str) -> String {
    for line in FIXTURE.lines() {
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() == 3 && cells[1].eq_ignore_ascii_case(name) {
            return cells[2].trim_matches('`').to_string();
        }
    }
    panic!("no section 1 position named {name}");
}

/// Every FEN in the section 1 table, startpos first.
pub fn standard_fens() -> Vec<String> {
    let mut out = Vec::new();
    for line in FIXTURE.lines() {
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() == 3
            && cells[0].parse::<u32>().is_ok()
            && let Some(fen) = cells[2].strip_prefix('`').and_then(|f| f.strip_suffix('`'))
        {
            out.push(fen.to_string());
        }
    }
    assert!(!out.is_empty(), "no section 1 table");
    out
}

/// The twenty DFRC start arrays, as `(wid, bid, fen)`.
pub fn dfrc_arrays() -> Vec<(u32, u32, String)> {
    tsv("dfrc-arrays")
        .into_iter()
        .map(|r| {
            (
                r[0].parse().expect("wid"),
                r[1].parse().expect("bid"),
                r[2].clone(),
            )
        })
        .collect()
}

/// The castling-legality positions of section 3.
pub fn castling_fens() -> Vec<String> {
    tsv("castling-legality")
        .into_iter()
        .map(|r| r[2].clone())
        .collect()
}

/// The edge-case positions of section 4.
pub fn edge_case_fens() -> Vec<String> {
    tsv("edge-cases")
        .into_iter()
        .map(|r| r[0].clone())
        .collect()
}

/// Every position the corpus names, for "over every corpus position" tests:
/// the standard suite, the DFRC arrays, the castling-legality set and the
/// edge cases.
pub fn corpus_fens() -> Vec<String> {
    let mut out = standard_fens();
    out.extend(dfrc_arrays().into_iter().map(|(_, _, fen)| fen));
    out.extend(castling_fens());
    out.extend(edge_case_fens());
    out
}

// ---------------------------------------------------------------------------
// The transposition table
// ---------------------------------------------------------------------------

/// A table for one search, at the size the engine defaults to.
///
/// Every gate that ran before the table existed gets a fresh one per
/// search, which is what those gates were written against: a search whose
/// node count depends on nothing but the position and the depth. A gate
/// that means to test the table across searches builds its own and keeps
/// it (`tests/tt.rs`).
///
/// # Panics
///
/// If the table cannot be allocated.
#[must_use]
pub fn table() -> cadence_engine::tt::Table {
    cadence_engine::tt::Table::new(cadence_engine::tt::DEFAULT_HASH_MB)
        .expect("a default-sized transposition table")
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// splitmix64. Seedable, so a failing walk is reproducible from its seed.
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        usize::try_from(self.next_u64() % n as u64).expect("fits")
    }
}

// ---------------------------------------------------------------------------
// The binary as a subprocess
// ---------------------------------------------------------------------------

/// How long a subprocess may take before the test calls it hung. Generous,
/// because CI runners are slow and debug builds slower; a real hang is
/// seconds of nothing, not a close call.
pub const SUBPROCESS_TIMEOUT: Duration = Duration::from_mins(1);

/// Feed `input` to `cadence` on stdin, all at once, and return its stdout.
///
/// The child is killed and the test fails if it has not exited within
/// [`SUBPROCESS_TIMEOUT`]: a `go infinite` that does not come back on `stop`
/// or `quit` is a hang, and a hung test is worse than a failed one.
pub fn talk(input: &str) -> String {
    talk_bytes(input.as_bytes())
}

/// The same, for input that is deliberately not valid UTF-8.
pub fn talk_bytes(input: &[u8]) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn cadence");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input)
        .expect("write to cadence");
    // Drop the stdin handle so the child sees end of input once it has read
    // everything; `wait_with_output` below reads stdout to the end.
    let start = Instant::now();
    loop {
        match child.try_wait().expect("poll cadence") {
            Some(_) => break,
            None if start.elapsed() > SUBPROCESS_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("cadence did not exit within {SUBPROCESS_TIMEOUT:?} on input {input:?}");
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let out = child.wait_with_output().expect("wait for cadence");
    assert!(out.status.success(), "exited with {:?}", out.status);
    String::from_utf8(out.stdout).expect("stdout is UTF-8")
}

/// The move named on the first `bestmove` line, or a panic naming the output.
pub fn bestmove(out: &str) -> String {
    bestmoves(out)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no bestmove line in {out:?}"))
}

/// Every `bestmove` line's move, in order.
pub fn bestmoves(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|l| l.strip_prefix("bestmove "))
        .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Colour mirror
// ---------------------------------------------------------------------------

/// The position with the colours swapped: ranks reversed, piece case swapped,
/// side to move flipped, castling rights swapped, the en-passant square on
/// the mirrored rank. Clocks unchanged. A legal position mirrors to a legal
/// position, and mirroring twice is the identity -- both asserted in
/// `tests/eval.rs`, because every coverage count there rests on this
/// function being right.
///
/// Operates on the FEN text, which is the representation in which the
/// transform is obviously correct. Shredder style, so the castling field is
/// always file letters and never needs the `K`/`Q` resolution redone.
pub fn mirror_fen(fen: &str) -> String {
    let fields: Vec<&str> = fen.split_whitespace().collect();
    assert!(fields.len() >= 4, "short FEN: {fen}");

    let placement: Vec<String> = fields[0]
        .split('/')
        .rev()
        .map(|rank| rank.chars().map(swap_case).collect())
        .collect();
    let stm = match fields[1] {
        "w" => "b",
        "b" => "w",
        other => panic!("side to move `{other}` in {fen}"),
    };
    let castling = if fields[2] == "-" {
        "-".to_string()
    } else {
        // Swap the case of every letter, then put White's first again so the
        // field reads the way the board writes it.
        let swapped: Vec<char> = fields[2].chars().map(swap_case).collect();
        let mut out: String = swapped.iter().filter(|c| c.is_ascii_uppercase()).collect();
        out.extend(swapped.iter().filter(|c| c.is_ascii_lowercase()));
        out
    };
    let ep = if fields[3] == "-" {
        "-".to_string()
    } else {
        let mut chars = fields[3].chars();
        let file = chars.next().expect("ep file");
        let rank = match chars.next().expect("ep rank") {
            '3' => '6',
            '6' => '3',
            other => panic!("ep rank `{other}` in {fen}"),
        };
        format!("{file}{rank}")
    };
    let mut out = format!("{} {stm} {castling} {ep}", placement.join("/"));
    for f in &fields[4..] {
        out.push(' ');
        out.push_str(f);
    }
    out
}

fn swap_case(c: char) -> char {
    if c.is_ascii_uppercase() {
        c.to_ascii_lowercase()
    } else if c.is_ascii_lowercase() {
        c.to_ascii_uppercase()
    } else {
        c
    }
}

/// `board` with the colours swapped. See [`mirror_fen`].
pub fn mirror(board: &cadence_core::Board) -> cadence_core::Board {
    let fen = board.to_fen(cadence_core::FenStyle::Shredder);
    let mirrored = mirror_fen(&fen);
    cadence_core::Board::from_fen(&mirrored)
        .unwrap_or_else(|e| panic!("mirror of {fen} is {mirrored}, rejected: {e:?}"))
}

// ---------------------------------------------------------------------------
// Endgame seeds
// ---------------------------------------------------------------------------

/// Positions with little or nothing left but pawns and kings, as seeds for
/// walks that need the endgame end of the phase scale. Random walks from
/// the start position do not get there.
pub const ENDGAME_FENS: &[&str] = &[
    "8/8/8/4k3/8/8/4P3/4K3 w - - 0 1",     // KPK
    "8/5k2/8/8/8/8/1PP5/1K6 w - - 0 1",    // KPPK
    "8/pp3k2/8/8/8/8/PP3K2/8 w - - 0 1",   // pawn ending, 2 v 2
    "8/8/3k4/8/8/3K4/8/4R3 w - - 0 1",     // KRK
    "8/8/8/3k4/8/8/8/4KQ2 w - - 0 1",      // KQK
    "8/8/8/3k4/8/8/8/2BNK3 w - - 0 1",     // KBNK
    "8/3k4/8/8/8/8/3P4/3K3r w - - 0 1",    // KPK v R
    "6k1/5ppp/8/8/8/8/5PPP/6K1 w - - 0 1", // symmetrical pawns
    "8/1k6/8/8/8/8/6K1/8 w - - 0 1",       // bare kings
    "8/8/4k3/8/3p4/8/3P4/4K3 b - - 0 1",   // blocked pawn, black to move
    "r6k/8/8/8/8/8/8/R6K w - - 0 1",       // rook ending, rooks only
    "8/8/8/8/8/2k5/8/1nK5 w - - 0 1",      // KNK (minor alone), white to move
];

// ---------------------------------------------------------------------------
// The binary, interactively
// ---------------------------------------------------------------------------

/// A running `cadence` process spoken to a line at a time. For tests that
/// need to read a reply before deciding what to send next -- a game on a
/// clock, where each `go` carries the time the previous reply left.
pub struct Engine {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl Engine {
    pub fn spawn() -> Engine {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cadence"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn cadence");
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = std::io::BufReader::new(child.stdout.take().expect("stdout is piped"));
        Engine {
            child,
            stdin,
            stdout,
        }
    }

    pub fn send(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").expect("write to cadence");
        self.stdin.flush().expect("flush to cadence");
    }

    /// Read lines until one starts with `prefix`; return every line read,
    /// the matching one last. Panics, rather than hanging, if the process
    /// ends first.
    pub fn read_until(&mut self, prefix: &str) -> Vec<String> {
        use std::io::BufRead;
        let mut lines = Vec::new();
        loop {
            let mut line = String::new();
            let n = self.stdout.read_line(&mut line).expect("read from cadence");
            assert!(
                n > 0,
                "cadence ended before a line starting with {prefix:?}; saw {lines:?}"
            );
            let line = line.trim_end().to_string();
            let done = line.starts_with(prefix);
            lines.push(line);
            if done {
                return lines;
            }
        }
    }

    /// `isready` / `readyok`: wait until the engine is up and has consumed
    /// everything sent so far. A timing test that does not do this measures
    /// process start-up -- 150-250 ms on macOS for a freshly built binary,
    /// which was enough to make "the search used its movetime" pass against
    /// a placeholder that returned at once.
    pub fn sync(&mut self) -> Vec<String> {
        self.send("isready");
        self.read_until("readyok")
    }

    /// Send `setup` lines, then `go_line`, read up to and including the
    /// `bestmove`, quit, and return everything the engine printed, one line
    /// per entry. The way to drive a search that must run to its limit:
    /// `talk` pipes `quit` in with the rest, and `quit` -- correctly -- stops
    /// a search that is still running.
    pub fn go(setup: &[&str], go_line: &str) -> Vec<String> {
        let mut e = Engine::spawn();
        for line in setup {
            e.send(line);
        }
        e.send(go_line);
        let lines = e.read_until("bestmove ");
        e.quit();
        lines
    }

    /// [`Engine::go`], with a deadline, returning how long the search took
    /// and everything printed from the first setup line onward.
    ///
    /// A gate whose subject is "the engine comes back" must not hang when it
    /// does not, and `read_until` blocks: `panic = "abort"` is not in force
    /// in the test profile (Cargo.toml), so a search thread that panics
    /// unwinds and dies while the UCI loop goes on reading, and no
    /// `bestmove` is ever printed. The same shape covers a search that
    /// simply never stops. The child sees end of input when this process
    /// exits, so a thread left behind on the failing path does not outlive
    /// the run.
    pub fn go_within(setup: &[&str], go_line: &str, timeout: Duration) -> (Duration, Vec<String>) {
        let setup: Vec<String> = setup.iter().map(|s| (*s).to_string()).collect();
        let line = go_line.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut e = Engine::spawn();
            for l in &setup {
                e.send(l);
            }
            // Sync first, so the elapsed time is the search's and not the
            // process start-up's.
            // Its output is kept: anything the setup lines printed comes out
            // before `readyok` and would otherwise be read past and lost.
            let mut lines = e.sync();
            lines.pop();
            let start = Instant::now();
            e.send(&line);
            lines.extend(e.read_until("bestmove "));
            if tx.send((start.elapsed(), lines)).is_ok() {
                e.quit();
            }
        });
        rx.recv_timeout(timeout)
            .unwrap_or_else(|_| panic!("no bestmove within {timeout:?} for `{go_line}`"))
    }

    /// `quit`, and wait for the process to exit cleanly.
    pub fn quit(mut self) {
        self.send("quit");
        let status = self.child.wait().expect("wait for cadence");
        assert!(status.success(), "exited with {status:?}");
    }
}

// ---------------------------------------------------------------------------
// Games
// ---------------------------------------------------------------------------

use cadence_core::position::Board;
use cadence_core::{Colour, Move, PieceType, generate_legal};

/// How a game ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The named colour delivered mate.
    Mate(Colour),
    Stalemate,
    /// Threefold repetition, claimed at the root.
    Repetition,
    FiftyMoves,
    /// Neither side has mating material: bare kings, or a lone minor piece.
    InsufficientMaterial,
    /// The ply cap was reached with the game still going.
    Cap,
}

/// A player: given the board, returns the move to play. The harness checks
/// that it is legal, so a player that returns an illegal move fails the
/// test by name rather than corrupting the board.
pub type Player<'a> = dyn FnMut(&mut Board) -> Move + 'a;

/// Play `white` against `black` from `fen` until a rules-based end or `cap`
/// plies. Returns the outcome and the moves played.
pub fn play_game(
    fen: &str,
    white: &mut Player<'_>,
    black: &mut Player<'_>,
    cap: usize,
) -> (Outcome, Vec<Move>) {
    let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
    let mut moves = Vec::new();
    loop {
        let legal = generate_legal(&board);
        if legal.is_empty() {
            let outcome = if board.in_check() {
                Outcome::Mate(board.side_to_move().flip())
            } else {
                Outcome::Stalemate
            };
            return (outcome, moves);
        }
        if board.is_repetition() {
            return (Outcome::Repetition, moves);
        }
        if board.halfmove_clock() >= 100 {
            return (Outcome::FiftyMoves, moves);
        }
        if insufficient_material(&board) {
            return (Outcome::InsufficientMaterial, moves);
        }
        if moves.len() >= cap {
            return (Outcome::Cap, moves);
        }
        let m = match board.side_to_move() {
            Colour::White => white(&mut board),
            Colour::Black => black(&mut board),
        };
        assert!(
            legal.contains(m),
            "illegal move {m:?} after {} plies from {fen}; legal {:?}",
            moves.len(),
            legal.as_slice()
        );
        assert_eq!(board.ply(), 0, "the player left search moves on the stack");
        board.play(m);
        moves.push(m);
    }
}

/// Bare kings, or one minor piece against a bare king.
pub fn insufficient_material(board: &Board) -> bool {
    let pawns = board.by_type(PieceType::Pawn).count();
    let majors = board.by_type(PieceType::Rook).count() + board.by_type(PieceType::Queen).count();
    let minors =
        board.by_type(PieceType::Knight).count() + board.by_type(PieceType::Bishop).count();
    pawns == 0 && majors == 0 && minors <= 1
}

/// A player that picks a uniformly random legal move.
pub fn random_mover(rng: &mut Rng) -> impl FnMut(&mut Board) -> Move + '_ {
    move |board: &mut Board| {
        let legal = generate_legal(board);
        legal.as_slice()[rng.below(legal.len())]
    }
}
