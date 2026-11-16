// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeded random command streams against the binary, started until a wall-clock budget is spent.
//! Every `go` answers exactly once, in time, with a move legal where it searched, every `pv` and
//! ponder move it names is legal, and every `isready` is answered.

mod support;

use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cadence_core::{Colour, Move, generate_legal, parse_uci, to_uci};
use cadence_engine::position::Position;
use support::{Engine, Rng, SUBPROCESS_TIMEOUT};

/// No stream starts after this, and one already started runs to its end.
const BUDGET: Duration = Duration::from_secs(5);

/// A failing stream is reproduced from this and its index, which the failure names.
const SEED: u64 = 0x5EED_2026_1106;

/// The stress runs' own, so they do not replay the short run's streams.
const STRESS_SEED: u64 = 0x5EED_2026_1116;

const STEPS: usize = 40;

/// Past the clock a search was left, for start-up and scheduling on a loaded runner. A search that
/// ignores its clock, or a `stop`, runs far past it.
const SLACK: Duration = Duration::from_secs(2);

/// Lines the engine must ignore, an empty one among them.
const NOISE: [&str; 4] = ["", "debug on", "register later", "xyzzy 1 2 3"];

/// A position as `position` sends it: a FEN from the corpus and the moves played from it.
#[derive(Clone)]
struct Line {
    fen: String,
    moves: Vec<Move>,
}

impl Line {
    fn board(&self) -> Position {
        let mut board = support::position(&self.fen);
        for &m in &self.moves {
            board.play(m);
        }
        board
    }

    fn command(&self) -> String {
        let mut board = support::position(&self.fen);
        let mut out = format!("position fen {}", self.fen);
        if !self.moves.is_empty() {
            out.push_str(" moves");
        }
        for &m in &self.moves {
            let legal = generate_legal(&board);
            out.push(' ');
            out.push_str(&to_uci(m, &legal, false));
            board.play(m);
        }
        out
    }
}

/// A search the engine owes one `bestmove`.
#[derive(Clone)]
struct Searched {
    line: Line,
    /// `Ponder` was on at its `go`, so its answer may name a reply, and otherwise must not.
    offers: bool,
    /// The clock a `ponderhit` leaves a ponder, in milliseconds; `None` for `infinite`.
    hit_clock: Option<u64>,
}

struct Stream {
    engine: Engine,
    rng: Rng,
    fens: Vec<String>,
    /// What the next `go` searches.
    current: Line,
    running: Option<Searched>,
    /// `Threads` for the whole stream where set; otherwise the stream changes it.
    threads: Option<usize>,
    /// The `Ponder` option as last sent.
    ponder: bool,
    /// Shared with the runner, which prints it when the stream fails, hangs or panics.
    log: Arc<Mutex<Vec<String>>>,
    gos: usize,
    readies: usize,
}

/// When an answer is late for a command sent now that leaves the search `ms` milliseconds.
fn deadline(ms: u64) -> Option<Instant> {
    Some(Instant::now() + Duration::from_millis(ms) + SLACK)
}

/// The clock of the side to move, which a search may spend and no more.
fn ours(us: Colour, white: u64, black: u64) -> u64 {
    match us {
        Colour::White => white,
        Colour::Black => black,
    }
}

/// Every move an `info` line's `pv` names, played in turn from where the search began.
fn pv_legal(line: &Line, said: &str) -> Result<(), String> {
    let mut words = said.split_whitespace();
    if words.next() != Some("info") || said.starts_with("info string") {
        return Ok(());
    }
    if !words.by_ref().any(|w| w == "pv") {
        return Ok(());
    }
    let mut board = line.board();
    for word in words {
        let legal = generate_legal(&board);
        match parse_uci(&legal, word) {
            Some(m) if to_uci(m, &legal, false) == word => board.play(m),
            _ => return Err(format!("`{said}` names {word} where it is not legal")),
        }
    }
    Ok(())
}

impl Stream {
    fn send(&mut self, line: &str) {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.to_string());
        self.engine.send(line);
    }

    fn searched(&self, hit_clock: Option<u64>) -> Searched {
        Searched {
            line: self.current.clone(),
            offers: self.ponder,
            hit_clock,
        }
    }

    /// Reads to the next `bestmove`, which must come by `deadline` and be legal where `searched`
    /// began, or `0000` where nothing is. Any reply it offers, and every `pv` before it, is legal.
    fn answer(&mut self, searched: &Searched, deadline: Option<Instant>) -> Result<(), String> {
        let lines = self.engine.read_until("bestmove ");
        let now = Instant::now();
        let late = deadline.and_then(|d| now.checked_duration_since(d));
        self.gos += 1;
        let after = searched.line.command();
        if let Some(late) = late {
            return Err(format!(
                "bestmove {late:?} past its deadline after `{after}`"
            ));
        }
        let (said, info) = lines.split_last().expect("read_until ends on the match");
        for line in info {
            if let Err(e) = pv_legal(&searched.line, line) {
                return Err(format!("{e}, after `{after}`"));
            }
        }
        let mut words = said.split_whitespace().skip(1);
        let mv = words.next().unwrap_or("");
        let reply = match (words.next(), words.next(), words.next()) {
            (None, ..) => None,
            (Some("ponder"), Some(reply), None) => Some(reply),
            _ => return Err(format!("`{said}` is not a bestmove line")),
        };
        let mut board = searched.line.board();
        let legal = generate_legal(&board);
        if legal.is_empty() {
            return match (mv, reply) {
                ("0000", None) => Ok(()),
                _ => Err(format!("`{said}` where no move is legal")),
            };
        }
        let parsed = parse_uci(&legal, mv);
        let Some(m) = parsed.filter(|&m| to_uci(m, &legal, false) == mv) else {
            return Err(format!("bestmove {mv} is not legal after `{after}`"));
        };
        let Some(reply) = reply else {
            return Ok(());
        };
        if !searched.offers {
            return Err(format!("`{said}` offers a reply with Ponder off"));
        }
        board.play(m);
        let legal = generate_legal(&board);
        match parse_uci(&legal, reply) {
            Some(r) if to_uci(r, &legal, false) == reply => Ok(()),
            _ => Err(format!(
                "ponder {reply} is not legal after {mv}, after `{after}`"
            )),
        }
    }

    /// An `isready` must be answered, and no `bestmove` may come before it that no `go` was owed.
    fn ready(&mut self) -> Result<(), String> {
        self.send("isready");
        let lines = self.engine.read_until("readyok");
        self.readies += 1;
        match lines.iter().find(|l| l.starts_with("bestmove ")) {
            Some(extra) => Err(format!("`{extra}` answered no go")),
            None => Ok(()),
        }
    }

    /// The running search's answer by `deadline`, then an `isready` with nothing owed before it.
    fn finish(&mut self, searched: &Searched, deadline: Option<Instant>) -> Result<(), String> {
        self.running = None;
        self.answer(searched, deadline)?;
        self.ready()
    }

    fn random_line(&mut self) -> Line {
        let fen = self.fens[self.rng.below(self.fens.len())].clone();
        let mut board = support::position(&fen);
        let mut moves = Vec::new();
        for _ in 0..self.rng.below(12) {
            let legal = generate_legal(&board);
            if legal.is_empty() {
                break;
            }
            let m = legal.as_slice()[self.rng.below(legal.len())];
            board.play(m);
            moves.push(m);
        }
        Line { fen, moves }
    }

    /// Both clocks: tiny ones, down to none left as a game's last seconds send them, or short ones.
    fn clocks(&mut self) -> (u64, u64) {
        if self.rng.below(2) == 0 {
            (self.rng.next_u64() % 41, self.rng.next_u64() % 41)
        } else {
            (
                50 + self.rng.next_u64() % 1950,
                50 + self.rng.next_u64() % 1950,
            )
        }
    }

    /// A `go` that ends by itself, soon, and what a clock lets it spend, where one bounds it.
    fn bounded_go(&mut self) -> (String, Option<u64>) {
        let us = self.current.board().side_to_move();
        match self.rng.below(9) {
            0 => (format!("go depth {}", 1 + self.rng.below(3)), None),
            1 => (format!("go nodes {}", 1 + self.rng.below(3000)), None),
            2 => ("go nodes 1".to_string(), None),
            3 => {
                let ms = self.rng.next_u64() % 21;
                (format!("go movetime {ms}"), Some(ms))
            }
            4 => {
                let (white, black) = self.clocks();
                let inc = self.rng.below(50);
                (
                    format!("go wtime {white} btime {black} winc {inc} binc {inc}"),
                    Some(ours(us, white, black)),
                )
            }
            5 => {
                let (white, black) = self.clocks();
                (
                    format!("go wtime {white} btime {black}"),
                    Some(ours(us, white, black)),
                )
            }
            6 => {
                let (white, black) = self.clocks();
                let togo = 1 + self.rng.below(3);
                (
                    format!("go wtime {white} btime {black} movestogo {togo}"),
                    Some(ours(us, white, black)),
                )
            }
            7 => ("go mate 1 depth 2".to_string(), None),
            _ => {
                let legal = generate_legal(&self.current.board());
                if legal.is_empty() {
                    return ("go depth 2".to_string(), None);
                }
                let m = legal.as_slice()[self.rng.below(legal.len())];
                (
                    format!("go depth 2 searchmoves {}", to_uci(m, &legal, false)),
                    None,
                )
            }
        }
    }

    /// A `go` that runs until told: `infinite`, or a ponder whose hit leaves it the clock returned.
    fn open_go(&mut self) -> (String, Option<u64>) {
        if self.rng.below(2) == 0 {
            return ("go infinite".to_string(), None);
        }
        let us = self.current.board().side_to_move();
        let (white, black) = self.clocks();
        (
            format!("go ponder wtime {white} btime {black}"),
            Some(ours(us, white, black)),
        )
    }

    /// A `setoption` line, and whether it stops a running search, as a new or cleared table does.
    fn option(&mut self) -> (String, bool) {
        let (choice, stops) = match self.rng.below(6) {
            0 => {
                let threads = self.threads.unwrap_or_else(|| [1, 2, 4][self.rng.below(3)]);
                (format!("Threads value {threads}"), false)
            }
            1 => (format!("MultiPV value {}", 1 + self.rng.below(2)), false),
            2 => (
                format!("Hash value {}", [1, 2, 16, 32][self.rng.below(4)]),
                true,
            ),
            3 => ("Clear Hash".to_string(), true),
            4 => (
                format!("Move Overhead value {}", [0, 10, 20][self.rng.below(3)]),
                false,
            ),
            _ => {
                self.ponder = self.rng.below(2) == 1;
                (format!("Ponder value {}", self.ponder), false)
            }
        };
        (format!("setoption name {choice}"), stops)
    }

    fn step(&mut self) -> Result<(), String> {
        let Some(searched) = self.running.clone() else {
            match self.rng.below(10) {
                0 | 1 => {
                    self.current = self.random_line();
                    let command = self.current.command();
                    self.send(&command);
                }
                2 => {
                    let (command, _) = self.option();
                    self.send(&command);
                }
                3 => self.send("ucinewgame"),
                4 => self.ready()?,
                5 | 6 => {
                    let (go, clock) = self.bounded_go();
                    let next = self.searched(None);
                    self.send(&go);
                    self.answer(&next, clock.and_then(deadline))?;
                    self.ready()?;
                }
                7 => {
                    let (go, hit_clock) = self.open_go();
                    let next = self.searched(hit_clock);
                    self.send(&go);
                    self.running = Some(next);
                }
                8 => {
                    let command = ["stop", "ponderhit"][self.rng.below(2)];
                    self.send(command);
                    self.ready()?;
                }
                _ => {
                    let noise = NOISE[self.rng.below(NOISE.len())];
                    self.send(noise);
                }
            }
            return Ok(());
        };
        match self.rng.below(8) {
            0 | 1 => {
                self.send("stop");
                self.finish(&searched, deadline(0))?;
            }
            // A second `go` stops the first, which answers before the second starts.
            2 => {
                let (go, clock) = self.bounded_go();
                let next = self.searched(None);
                self.send(&go);
                let (stopped, then) = (deadline(0), clock.and_then(deadline));
                self.running = None;
                self.answer(&searched, stopped)?;
                self.answer(&next, then)?;
                self.ready()?;
            }
            3 => {
                self.send("ucinewgame");
                self.finish(&searched, deadline(0))?;
            }
            4 => self.ready()?,
            // A `position` stops the search, which answers before the board changes.
            5 => {
                self.current = self.random_line();
                let command = self.current.command();
                self.send(&command);
                self.finish(&searched, deadline(0))?;
            }
            6 => {
                let (command, stops) = self.option();
                self.send(&command);
                if stops {
                    self.finish(&searched, deadline(0))?;
                }
            }
            // Only a ponder takes the hit; an infinite search goes on until it is stopped.
            _ => {
                self.send("ponderhit");
                if let Some(ms) = searched.hit_clock {
                    self.finish(&searched, deadline(ms))?;
                }
            }
        }
        Ok(())
    }
}

/// One stream from a fresh process, returning its counts of `go` and `isready`.
fn stream(
    seed: u64,
    threads: Option<usize>,
    log: Arc<Mutex<Vec<String>>>,
) -> Result<(usize, usize), String> {
    let fens = support::standard_fens();
    let current = Line {
        fen: fens[0].clone(),
        moves: Vec::new(),
    };
    let mut s = Stream {
        engine: Engine::spawn(),
        rng: Rng::new(seed),
        fens,
        current,
        running: None,
        threads,
        ponder: false,
        log,
        gos: 0,
        readies: 0,
    };
    if let Some(threads) = threads {
        s.send(&format!("setoption name Threads value {threads}"));
    }
    (0..STEPS).try_for_each(|_| s.step())?;
    if let Some(searched) = s.running.clone() {
        // `quit` mid-search must end the process cleanly too, which `Engine::quit` asserts.
        if s.rng.below(2) == 0 {
            s.log
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push("quit".to_string());
            let counts = (s.gos, s.readies);
            s.engine.quit();
            return Ok(counts);
        }
        s.send("stop");
        s.finish(&searched, deadline(0))?;
    }
    s.ready()?;
    let counts = (s.gos, s.readies);
    s.engine.quit();
    Ok(counts)
}

/// A stream on a thread of its own, so a hang fails at the deadline and a panic as a failure. Any
/// failure names every command the stream sent.
fn run_stream(seed: u64, threads: Option<usize>) -> Result<(usize, usize), String> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = channel();
    let shared = Arc::clone(&log);
    std::thread::spawn(move || {
        let _ = tx.send(stream(seed, threads, shared));
    });
    let failure = match rx.recv_timeout(SUBPROCESS_TIMEOUT) {
        Ok(Ok(counts)) => return Ok(counts),
        Ok(Err(e)) => e,
        Err(RecvTimeoutError::Timeout) => format!("no end within {SUBPROCESS_TIMEOUT:?}"),
        Err(RecvTimeoutError::Disconnected) => {
            "the stream panicked, so the engine ended or answered out of turn".to_string()
        }
    };
    let sent = log
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .join("\n");
    Err(format!("{failure}, after:\n{sent}"))
}

/// Streams until `budget` is spent, with `Threads` held where `threads` names it.
fn run(seed: u64, threads: Option<usize>, budget: Duration) {
    let start = Instant::now();
    let (mut streams, mut gos, mut readies) = (0u64, 0, 0);
    while streams == 0 || start.elapsed() < budget {
        let seed = seed ^ streams.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        match run_stream(seed, threads) {
            Ok((g, r)) => {
                gos += g;
                readies += r;
            }
            Err(e) => panic!("{threads:?} threads, stream {streams}, seed {seed:#x}: {e}"),
        }
        streams += 1;
    }
    println!(
        "{streams} streams, {gos} go, {readies} isready, threads {threads:?}, in {:?}",
        start.elapsed()
    );
    assert!(gos > 0 && readies > 0, "the streams asked nothing");
}

/// `CADENCE_STRESS_SECS`, which CI sets, else `BUDGET`.
fn stress_budget() -> Duration {
    let Ok(secs) = std::env::var("CADENCE_STRESS_SECS") else {
        return BUDGET;
    };
    Duration::from_secs(secs.parse().expect("whole seconds"))
}

#[test]
fn a_random_command_stream_answers_every_go_once_and_every_isready() {
    run(SEED, None, BUDGET);
}

#[test]
#[ignore = "the stress run, which CI times by CADENCE_STRESS_SECS"]
fn a_stress_run_at_one_thread_answers_every_command_in_time() {
    run(STRESS_SEED, Some(1), stress_budget());
}

#[test]
#[ignore = "the stress run, which CI times by CADENCE_STRESS_SECS"]
fn a_stress_run_at_four_threads_answers_every_command_in_time() {
    run(STRESS_SEED ^ 4, Some(4), stress_budget());
}
