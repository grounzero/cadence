// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::BufRead;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

use cadence_core::position::Board;
use cadence_core::{MAX_MOVES, START_FEN, generate_legal, parse_uci};

use crate::level;
use crate::tt::{self, Table};
use crate::tune::{self, Tunables};

mod go;
mod options;
mod say;

use say::say;

/// The version half is `version::VERSION`, not the package version, so a build off a release tag
/// names its commit.
const ENGINE_NAME: &str = "Cadence";
const ENGINE_VERSION: &str = crate::version::VERSION;
const ENGINE_AUTHOR: &str = "Michael Grounds";

/// The only value for which a search is reproducible, and what `bench`, rating lists and every SPRT
/// play.
pub const DEFAULT_THREADS: usize = 1;

/// Each reserves `SEARCH_STACK_BYTES` of stack, so the ceiling is a real resource claim.
pub const MAX_THREADS: usize = 64;

pub struct Session {
    /// At ply zero, with the game history `position` replayed into it.
    board: Board,
    /// Output spelling only; both spellings are always accepted on input.
    chess960: bool,
    /// Kept across the game. `Hash` replaces it; `ucinewgame` clears it.
    tt: Arc<Table>,
    multipv: usize,
    /// A boolean, not a sentinel rating, so no arithmetic on a rating can turn the feature on.
    limit_strength: bool,
    /// Read only where `limit_strength` is set, so its value alone reaches nothing.
    elo: u32,
    /// Decides whether `bestmove` offers a ponder move. Off by default, as every rating list and
    /// SPRT plays.
    ponder: bool,
    threads: usize,
    /// Handed to every search a `go` starts and to nothing else.
    tunables: Tunables,
    /// Until `stop`, the next `go`, or shutdown joins it; it may already have finished.
    search: Option<Running>,
}

struct Running {
    stop: Arc<AtomicBool>,
    ponder_hit: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// # Panics
    ///
    /// If the default table cannot be allocated.
    #[must_use]
    pub fn new() -> Session {
        #[allow(clippy::expect_used, reason = "the default table is sixteen mebibytes")]
        let tt = Table::new(tt::DEFAULT_HASH_MB).expect("the default transposition table");
        Session {
            board: start_position(),
            chess960: false,
            tt: Arc::new(tt),
            multipv: 1,
            limit_strength: false,
            elo: level::MAX_ELO,
            ponder: false,
            threads: DEFAULT_THREADS,
            tunables: Tunables::DEFAULT,
            search: None,
        }
    }

    #[must_use]
    pub fn board(&self) -> &Board {
        &self.board
    }

    #[must_use]
    pub fn chess960(&self) -> bool {
        self.chess960
    }

    #[must_use]
    pub fn multipv(&self) -> usize {
        self.multipv
    }

    #[must_use]
    pub fn ponder(&self) -> bool {
        self.ponder
    }

    #[must_use]
    pub fn threads(&self) -> usize {
        self.threads
    }

    #[must_use]
    pub fn tunables(&self) -> &Tunables {
        &self.tunables
    }

    #[must_use]
    pub fn tt(&self) -> &Table {
        &self.tt
    }

    /// `false` on `quit`.
    pub fn handle_line(&mut self, line: &str) -> bool {
        // A GUI may send arbitrary spacing, so split on whitespace rather than trusting the line's
        // shape.
        let mut tokens = line.split_whitespace();
        let Some(command) = tokens.next() else {
            return true;
        };
        match command {
            "uci" => {
                say(format_args!("id name {ENGINE_NAME} {ENGINE_VERSION}"));
                say(format_args!("id author {ENGINE_AUTHOR}"));
                // A GUI offers a Chess960 game only to an engine that declares this one.
                say(format_args!(
                    "option name UCI_Chess960 type check default false"
                ));
                // Honoured, not only advertised: the OpenBench presets pass Hash=16 at STC and
                // Hash=64 at LTC, and an ignored option would have both sides play the default.
                say(format_args!(
                    "option name Hash type spin default {} min {} max {}",
                    tt::DEFAULT_HASH_MB,
                    tt::MIN_HASH_MB,
                    tt::MAX_HASH_MB
                ));
                // Helpers reach the primary only through the table. The default stays at one, which
                // every rating list and SPRT plays.
                say(format_args!(
                    "option name Threads type spin default {DEFAULT_THREADS} min 1 max {MAX_THREADS}"
                ));
                // The maximum is the longest move list the generator returns; a root with fewer
                // reports the moves it has.
                say(format_args!(
                    "option name MultiPV type spin default 1 min 1 max {MAX_MOVES}"
                ));
                // The number is inert without the boolean, so a GUI can send an opponent's rating
                // without changing how the engine plays.
                say(format_args!(
                    "option name UCI_LimitStrength type check default false"
                ));
                say(format_args!(
                    "option name UCI_Elo type spin default {} min {} max {}",
                    level::MAX_ELO,
                    level::MIN_ELO,
                    level::MAX_ELO
                ));
                // Off by default: pondering doubles one side's thinking, which is why every rating
                // list disables it.
                say(format_args!("option name Ponder type check default false"));
                // From the table `cadence spsa` prints, so a tuner's names are the names declared
                // here.
                for param in tune::PARAMS {
                    say(format_args!("{}", param.uci_option()));
                }
                say(format_args!("uciok"));
            }
            "isready" => say(format_args!("readyok")),
            "setoption" => self.set_option(tokens),
            "position" => self.set_position(tokens),
            "go" => self.go(tokens),
            // A running search is stopped either way. `ucinewgame` also empties the table: a
            // surviving entry is a score for a position reached by a different route.
            "stop" => self.stop_search(),
            "ponderhit" => self.ponderhit(),
            "ucinewgame" => {
                self.stop_search();
                self.tt.clear();
            }
            "quit" => return false,
            // Ignored, per the protocol.
            _ => {}
        }
        true
    }

    /// Called on `quit` and at end of input.
    pub fn shutdown(&mut self) {
        self.stop_search();
    }

    // --- position -----------------------------------------------------------

    /// Rebuilt from scratch every time, never appended to.
    fn set_position<'a>(&mut self, mut tokens: impl Iterator<Item = &'a str>) {
        let mut board = match tokens.next() {
            Some("startpos") => start_position(),
            Some("fen") => {
                let fen: Vec<&str> = tokens.by_ref().take_while(|t| *t != "moves").collect();
                match Board::from_fen(&fen.join(" ")) {
                    Ok(b) => b,
                    Err(e) => {
                        say(format_args!(
                            "info string position: FEN rejected ({e:?}): {}",
                            fen.join(" ")
                        ));
                        return;
                    }
                }
            }
            other => {
                say(format_args!(
                    "info string position: expected startpos or fen, got {}",
                    other.unwrap_or("nothing")
                ));
                return;
            }
        };
        // After `startpos` the `moves` keyword is still ahead; after `fen` the take_while consumed
        // it.
        let mut tokens = tokens.skip_while(|t| *t == "moves");
        for tok in tokens.by_ref() {
            let legal = generate_legal(&board);
            let Some(m) = parse_uci(&legal, tok) else {
                say(format_args!(
                    "info string position: `{tok}` is not a legal move here, ignoring it and the rest"
                ));
                break;
            };
            board.play(m);
        }
        // Accepted, then named: refusing leaves the previous position in place, and the `go` that
        // follows answers with a move illegal in the position the GUI believes it set.
        if board.opponent_in_check() {
            say(format_args!(
                "info string position: the side not to move is in check; \
                 no legal play reaches this position, searching it anyway"
            ));
        }
        self.board = board;
    }
}

/// `START_FEN` is pinned by the corpus, so this cannot fail.
fn start_position() -> Board {
    #[allow(clippy::expect_used, reason = "a constant FEN")]
    Board::from_fen(START_FEN).expect("the start position parses")
}

#[must_use]
pub fn run() -> ExitCode {
    // With stdin at end of file, `cadence` prints the identity and exits: the cheapest smoke test
    // there is.
    say(format_args!(
        "{ENGINE_NAME} {ENGINE_VERSION} by {ENGINE_AUTHOR}"
    ));

    let mut session = Session::new();
    let mut stdin = std::io::stdin().lock();
    let mut raw = Vec::new();
    loop {
        // Bytes, not `lines()`: a line that is not UTF-8 would end the session silently, exit 0,
        // leaving a GUI a crash with nothing to attribute it to.
        raw.clear();
        match stdin.read_until(b'\n', &mut raw) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
        let line = String::from_utf8_lossy(&raw);
        if !session.handle_line(&line) {
            break;
        }
    }
    session.shutdown();
    ExitCode::SUCCESS
}
