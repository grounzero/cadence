// SPDX-License-Identifier: GPL-3.0-or-later

//! Seeded random command streams against the binary, started until a fixed wall-clock budget is
//! spent. Every `go` answers exactly once with a move legal where it searched, and every `isready`
//! is answered.

mod support;

use std::time::{Duration, Instant};

use cadence_core::{Move, generate_legal, parse_uci, to_uci};
use cadence_engine::position::Position;
use support::{Engine, Rng, SUBPROCESS_TIMEOUT};

/// No stream starts after this, and one already started runs to its end.
const BUDGET: Duration = Duration::from_secs(5);

/// A failing stream is reproduced from this and its index, which the failure names.
const SEED: u64 = 0x5EED_2026_1106;

const STEPS: usize = 40;

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

struct Stream {
    engine: Engine,
    rng: Rng,
    fens: Vec<String>,
    /// What the next `go` searches.
    current: Line,
    /// A search that runs until something ends it, and whether it ponders on a clock.
    running: Option<(Line, bool)>,
    log: Vec<String>,
    gos: usize,
    readies: usize,
}

impl Stream {
    fn send(&mut self, line: &str) {
        self.log.push(line.to_string());
        self.engine.send(line);
    }

    /// Reads to the next `bestmove`, which must be legal in `line`, or `0000` where nothing is.
    fn answer(&mut self, line: &Line) -> Result<(), String> {
        let lines = self.engine.read_until("bestmove ");
        let said = lines.last().map_or("", String::as_str);
        let mv = said
            .strip_prefix("bestmove ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or("");
        self.gos += 1;
        let legal = generate_legal(&line.board());
        if legal.is_empty() {
            return if mv == "0000" {
                Ok(())
            } else {
                Err(format!("bestmove {mv} where no move is legal"))
            };
        }
        match parse_uci(&legal, mv) {
            Some(m) if to_uci(m, &legal, false) == mv => Ok(()),
            _ => Err(format!(
                "bestmove {mv} is not legal after `{}`",
                line.command()
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

    /// The running search's answer, then an `isready` with nothing owed before it.
    fn finish(&mut self, searched: &Line) -> Result<(), String> {
        self.running = None;
        self.answer(searched)?;
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

    /// A `go` that ends by itself, soon.
    fn bounded_go(&mut self) -> String {
        match self.rng.below(6) {
            0 => format!("go depth {}", 1 + self.rng.below(3)),
            1 => format!("go nodes {}", 1 + self.rng.below(3000)),
            2 => format!("go movetime {}", 1 + self.rng.below(20)),
            3 => {
                let white = 50 + self.rng.below(1950);
                let black = 50 + self.rng.below(1950);
                let inc = self.rng.below(50);
                format!("go wtime {white} btime {black} winc {inc} binc {inc}")
            }
            4 => "go mate 1 depth 2".to_string(),
            _ => {
                let legal = generate_legal(&self.current.board());
                if legal.is_empty() {
                    return "go depth 2".to_string();
                }
                let m = legal.as_slice()[self.rng.below(legal.len())];
                format!("go depth 2 searchmoves {}", to_uci(m, &legal, false))
            }
        }
    }

    /// A `go` that runs until told: `infinite`, or a ponder whose hit brings a short clock.
    fn open_go(&mut self) -> (String, bool) {
        if self.rng.below(2) == 0 {
            return ("go infinite".to_string(), false);
        }
        let white = 50 + self.rng.below(450);
        let black = 50 + self.rng.below(450);
        (format!("go ponder wtime {white} btime {black}"), true)
    }

    fn option(&mut self) -> String {
        let choice = match self.rng.below(6) {
            0 => format!("Threads value {}", [1, 2, 4][self.rng.below(3)]),
            1 => format!("MultiPV value {}", 1 + self.rng.below(2)),
            2 => format!("Hash value {}", [1, 2, 16][self.rng.below(3)]),
            3 => "Clear Hash".to_string(),
            4 => format!("Move Overhead value {}", [0, 10, 20][self.rng.below(3)]),
            _ => format!("Ponder value {}", self.rng.below(2) == 1),
        };
        format!("setoption name {choice}")
    }

    fn step(&mut self) -> Result<(), String> {
        let Some((searched, ponder)) = self.running.clone() else {
            match self.rng.below(10) {
                0 | 1 => {
                    self.current = self.random_line();
                    let command = self.current.command();
                    self.send(&command);
                }
                2 => {
                    let command = self.option();
                    self.send(&command);
                }
                3 => self.send("ucinewgame"),
                4 => self.ready()?,
                5 | 6 => {
                    let go = self.bounded_go();
                    self.send(&go);
                    let line = self.current.clone();
                    self.answer(&line)?;
                    self.ready()?;
                }
                7 => {
                    let (go, ponder) = self.open_go();
                    self.send(&go);
                    self.running = Some((self.current.clone(), ponder));
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
                self.finish(&searched)?;
            }
            // A second `go` stops the first, which answers before the second starts.
            2 => {
                let go = self.bounded_go();
                self.send(&go);
                self.running = None;
                self.answer(&searched)?;
                let line = self.current.clone();
                self.answer(&line)?;
                self.ready()?;
            }
            3 => {
                self.send("ucinewgame");
                self.finish(&searched)?;
            }
            4 => self.ready()?,
            // A `position` stops the search, which answers before the board changes.
            5 => {
                self.current = self.random_line();
                let command = self.current.command();
                self.send(&command);
                self.finish(&searched)?;
            }
            6 => {
                let command = self.option();
                self.send(&command);
            }
            // Only a ponder takes the hit; an infinite search goes on until it is stopped.
            _ => {
                self.send("ponderhit");
                if ponder {
                    self.finish(&searched)?;
                }
            }
        }
        Ok(())
    }
}

/// One stream from a fresh process, returning its counts of `go` and `isready`, or the commands
/// sent and what went wrong.
fn stream(seed: u64) -> Result<(usize, usize), String> {
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
        log: Vec::new(),
        gos: 0,
        readies: 0,
    };
    let mut result = (0..STEPS).try_for_each(|_| s.step());
    if result.is_ok()
        && let Some((searched, _)) = s.running.clone()
    {
        s.send("stop");
        result = s.finish(&searched);
    }
    if result.is_ok() {
        result = s.ready();
    }
    match result {
        Ok(()) => {
            let counts = (s.gos, s.readies);
            s.engine.quit();
            Ok(counts)
        }
        Err(e) => Err(format!("{e}, after:\n{}", s.log.join("\n"))),
    }
}

#[test]
fn a_random_command_stream_answers_every_go_once_and_every_isready() {
    let start = Instant::now();
    let (mut streams, mut gos, mut readies) = (0u64, 0, 0);
    while streams == 0 || start.elapsed() < BUDGET {
        let seed = SEED ^ streams.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        match Engine::within(SUBPROCESS_TIMEOUT, move || stream(seed)) {
            Ok((g, r)) => {
                gos += g;
                readies += r;
            }
            Err(e) => panic!("stream {streams}, seed {seed:#x}: {e}"),
        }
        streams += 1;
    }
    println!("{streams} streams, {gos} go, {readies} isready");
    assert!(gos > 0 && readies > 0, "the streams asked nothing");
}
