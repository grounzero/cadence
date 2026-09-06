// SPDX-License-Identifier: GPL-3.0-or-later

//! The UCI command loop. One `Session` per process.

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use cadence_core::Move;
use cadence_core::position::Board;
use cadence_core::{MAX_MOVES, START_FEN, generate_legal, parse_uci, to_uci};

use crate::search::{Limits, Search};
use crate::tt::{self, Table};

/// The public identity: what appears on rating lists and in the header of every game a GUI
/// records. The version half is [`crate::version::VERSION`] rather than the package version, so
/// a build that is not at a release tag says so and names the commit it came from.
const ENGINE_NAME: &str = "Cadence";
const ENGINE_VERSION: &str = crate::version::VERSION;
const ENGINE_AUTHOR: &str = "Michael Grounds";

/// Stack for the search thread. The search recurses one frame per ply to `MAX_PLY` at most,
/// each frame holding a `MoveList` and a little more; 16 MiB is far above that in any profile.
const SEARCH_STACK_BYTES: usize = 16 << 20;

/// The state one UCI session carries between commands.
pub struct Session {
    /// The current position, at ply zero, with the game history the `position` command replayed
    /// into it.
    board: Board,
    /// `UCI_Chess960`. Governs how castling moves are *spelled* on output; both spellings are
    /// always accepted on input.
    chess960: bool,
    /// The transposition table, kept across the whole game and shared with the search thread.
    /// `Hash` replaces it; `ucinewgame` clears it.
    tt: Arc<Table>,
    /// `MultiPV`: how many principal variations a search reports. One is the default, and at
    /// one the engine reports what it did without it.
    multipv: usize,
    /// `Ponder`: whether the GUI intends to think on our move, which is what decides whether a
    /// `bestmove` offers a move to ponder on. Off by default, and off is what every rating list
    /// and every SPRT plays.
    ponder: bool,
    /// The search thread started by the last `go`, until `stop`, the next `go`, or shutdown
    /// joins it. It may already have finished.
    search: Option<Running>,
}

/// A search in flight: the flag that ends it, the flag a `ponderhit` raises, and the thread to
/// wait for.
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
    /// A session at the start position, `UCI_Chess960` off, a table of `tt::DEFAULT_HASH_MB`,
    /// no search.
    ///
    /// # Panics
    ///
    /// If the default table cannot be allocated. A GUI-supplied size that cannot be is reported
    /// and refused (`set_option`); the default is sixteen mebibytes, and a machine without them
    /// cannot run a search.
    #[must_use]
    pub fn new() -> Session {
        #[allow(clippy::expect_used, reason = "the default table is sixteen mebibytes")]
        let tt = Table::new(tt::DEFAULT_HASH_MB).expect("the default transposition table");
        Session {
            board: start_position(),
            chess960: false,
            tt: Arc::new(tt),
            multipv: 1,
            ponder: false,
            search: None,
        }
    }

    /// The current position.
    #[must_use]
    pub fn board(&self) -> &Board {
        &self.board
    }

    /// The `UCI_Chess960` option.
    #[must_use]
    pub fn chess960(&self) -> bool {
        self.chess960
    }

    /// The `MultiPV` option.
    #[must_use]
    pub fn multipv(&self) -> usize {
        self.multipv
    }

    /// The `Ponder` option.
    #[must_use]
    pub fn ponder(&self) -> bool {
        self.ponder
    }

    /// The transposition table this session is playing with.
    #[must_use]
    pub fn tt(&self) -> &Table {
        &self.tt
    }

    /// Handle one line of input. Returns `false` when the session is over (`quit`), `true`
    /// otherwise.
    pub fn handle_line(&mut self, line: &str) -> bool {
        // A GUI may send trailing whitespace, and `position ... moves ...` arrives with
        // arbitrary internal spacing. Split on whitespace rather than trusting the shape of the
        // line.
        let mut tokens = line.split_whitespace();
        let Some(command) = tokens.next() else {
            return true;
        };
        match command {
            "uci" => {
                say(format_args!("id name {ENGINE_NAME} {ENGINE_VERSION}"));
                say(format_args!("id author {ENGINE_AUTHOR}"));
                // Every option the engine understands, before uciok. A GUI offers a Chess960
                // game only to an engine that declares this one.
                say(format_args!(
                    "option name UCI_Chess960 type check default false"
                ));
                // `Hash` is honoured: the value is the table's size in mebibytes and setting it
                // replaces the table. The runners pass it -- the OpenBench presets say Hash=16
                // at STC and Hash=64 at LTC -- and an advertised option that did nothing would
                // make both sides of a test play with whatever the engine defaults to while the
                // preset said otherwise.
                say(format_args!(
                    "option name Hash type spin default {} min {} max {}",
                    tt::DEFAULT_HASH_MB,
                    tt::MIN_HASH_MB,
                    tt::MAX_HASH_MB
                ));
                // `Threads` is declared with a maximum of one, which is the truthful
                // declaration: the search is single-threaded and does not implement Lazy SMP.
                // Declaring it stops runners warning that the engine lacks an option they were
                // told to set.
                say(format_args!(
                    "option name Threads type spin default 1 min 1 max 1"
                ));
                // `MultiPV` above one searches the second-best root move and beyond, so it
                // costs nodes by construction. The maximum is the longest move list the
                // generator can return, because a root asked for more lines reports the moves
                // it has.
                say(format_args!(
                    "option name MultiPV type spin default 1 min 1 max {MAX_MOVES}"
                ));
                // `Ponder` is what a GUI reads to decide whether to think on our move at all,
                // and it is what puts the move to ponder on into the `bestmove` line. Off by
                // default: pondering doubles the thinking one side gets, which is why every
                // rating list disables it.
                say(format_args!("option name Ponder type check default false"));
                say(format_args!("uciok"));
            }
            "isready" => say(format_args!("readyok")),
            "setoption" => self.set_option(tokens),
            "position" => self.set_position(tokens),
            "go" => self.go(tokens),
            // A search left running across either is stopped, as it would be by the `position`
            // and `go` that follow. `ucinewgame` then empties the table: the next game's tree
            // has nothing to do with this one's, and an entry that survives is a score for a
            // position reached by a different route.
            "stop" => self.stop_search(),
            "ponderhit" => self.ponderhit(),
            "ucinewgame" => {
                self.stop_search();
                self.tt.clear();
            }
            "quit" => return false,
            // `debug`, `register` and anything unknown are ignored, per the protocol.
            _ => {}
        }
        true
    }

    /// Stop any running search and wait for its `bestmove`. Called on `quit` and at end of
    /// input.
    pub fn shutdown(&mut self) {
        self.stop_search();
    }

    // --- setoption ----------------------------------------------------------

    /// `setoption name <name> [value <value>]`. Names and values may contain spaces; the
    /// keywords `name` and `value` delimit them.
    fn set_option<'a>(&mut self, tokens: impl Iterator<Item = &'a str>) {
        let mut name = Vec::new();
        let mut value = Vec::new();
        let mut into_value = false;
        let mut seen_name = false;
        for tok in tokens {
            match tok {
                "name" if !seen_name => seen_name = true,
                "value" if seen_name && !into_value => into_value = true,
                _ if into_value => value.push(tok),
                _ if seen_name => name.push(tok),
                _ => {}
            }
        }
        let name = name.join(" ");
        let value = value.join(" ");
        if name.eq_ignore_ascii_case("UCI_Chess960") {
            if value.eq_ignore_ascii_case("true") {
                self.chess960 = true;
            } else if value.eq_ignore_ascii_case("false") {
                self.chess960 = false;
            }
        } else if name.eq_ignore_ascii_case("Hash") {
            self.set_hash(&value);
        } else if name.eq_ignore_ascii_case("MultiPV") {
            self.set_multipv(&value);
        } else if name.eq_ignore_ascii_case("Ponder") {
            if value.eq_ignore_ascii_case("true") {
                self.ponder = true;
            } else if value.eq_ignore_ascii_case("false") {
                self.ponder = false;
            }
        }
        // `Threads` is declared with a maximum of one and there is nothing to set: the value is
        // accepted and ignored. Unknown options are ignored too; a GUI sends whatever it was
        // told to.
    }

    /// `setoption name MultiPV value <n>`: how many lines a search reports. Clamped rather than
    /// refused, for [`Session::set_hash`]'s reason: a GUI that sends an out-of-range value is
    /// not going to send another.
    fn set_multipv(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption MultiPV: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        self.multipv = asked.clamp(1, MAX_MOVES);
    }

    /// `setoption name Hash value <mebibytes>`: a new table of that size. Out-of-range values
    /// are clamped rather than refused, because a GUI that sends one is not going to send
    /// another.
    fn set_hash(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption Hash: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        let mb = asked.clamp(tt::MIN_HASH_MB, tt::MAX_HASH_MB);
        match Table::new(mb) {
            Some(table) => self.tt = Arc::new(table),
            None => say(format_args!(
                "info string setoption Hash: {mb} MB could not be allocated, keeping {} MB",
                self.tt.bytes() >> 20
            )),
        }
    }

    // --- position -----------------------------------------------------------

    /// `position [startpos | fen <fen>] [moves <m>...]`. Rebuilt from scratch every time, never
    /// appended to.
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
        // After `startpos` the `moves` keyword is still ahead; after `fen` the take_while
        // consumed it. Either way, whatever is left is moves.
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
        // Accepted, then named. The position is one no legal play can reach -- the side to move
        // could take a king -- and it is set anyway, because refusing it is the worse failure
        // of the two: refusing leaves the *previous* position in place, the `go` that follows
        // searches something else, and the move that comes back is illegal in the position the
        // GUI believes it set.
        if board.opponent_in_check() {
            say(format_args!(
                "info string position: the side not to move is in check; \
                 no legal play reaches this position, searching it anyway"
            ));
        }
        self.board = board;
    }

    // --- go / stop ----------------------------------------------------------

    /// Start the search on its own thread with a copy of the board. A search still running from
    /// a previous `go` is stopped first.
    fn go<'a>(&mut self, tokens: impl Iterator<Item = &'a str>) {
        self.stop_search();
        let limits = Limits::parse(tokens);
        // A `go` that spoke about the clock without naming ours. The search treats a clock it
        // was not told as zero and returns its first iteration, which is safe and looks exactly
        // like a broken engine from the other end of the pipe, so say which it is.
        if !limits.infinite
            && limits.movetime.is_none()
            && limits.is_clocked()
            && limits.clock(self.board.side_to_move()).is_none()
        {
            say(format_args!(
                "info string go: no clock for the side to move; treating it \
                 as none left rather than as unlimited"
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let ponder_hit = Arc::new(AtomicBool::new(false));
        let mut board = self.board.duplicate();
        let chess960 = self.chess960;
        let multipv = self.multipv;
        let ponder = self.ponder;
        let thread = {
            let stop = Arc::clone(&stop);
            let ponder_hit = Arc::clone(&ponder_hit);
            // A handle of its own, so that a `setoption name Hash` during the search replaces
            // the session's table without pulling this one out from under the thread reading
            // it.
            let tt = Arc::clone(&self.tt);
            // An explicit stack: the search recurses to MAX_PLY at most, with a move list in
            // every frame, and the default for a spawned thread is not something to rely on
            // across platforms and profiles.
            std::thread::Builder::new()
                .name("search".to_string())
                .stack_size(SEARCH_STACK_BYTES)
                .spawn(move || {
                    let legal = generate_legal(&board);
                    let mut out = std::io::stdout();
                    let mut search = Search::new(limits, &stop, &tt);
                    search.set_ponder_hit(&ponder_hit);
                    search.set_chess960(chess960);
                    search.set_multipv(multipv);
                    let best = search.run(&mut board, &mut out);
                    let spelled = to_uci(best, &legal, chess960);
                    // Only when the GUI said it ponders. Off is the default and what every
                    // rating list plays, and the line it reads there is the line it always read.
                    match ponder
                        .then(|| ponder_move(&mut board, best, search.pv(), chess960))
                        .flatten()
                    {
                        Some(reply) => say(format_args!("bestmove {spelled} ponder {reply}")),
                        None => say(format_args!("bestmove {spelled}")),
                    }
                })
        };
        match thread {
            Ok(thread) => {
                self.search = Some(Running {
                    stop,
                    ponder_hit,
                    thread,
                });
            }
            Err(e) => say(format_args!(
                "info string go: could not start the search thread: {e}"
            )),
        }
    }

    /// `ponderhit`: the opponent played the move being pondered on, so the search keeps the tree
    /// it has built and starts spending the clock from here. A flag rather than a new `Limits`,
    /// because the search that has to hear about it is already running.
    fn ponderhit(&mut self) {
        if let Some(running) = &self.search {
            running.ponder_hit.store(true, Ordering::Relaxed);
        }
    }

    /// Raise the stop flag and wait for the search thread, which prints its `bestmove` on the
    /// way out. Nothing to do if no search is running.
    fn stop_search(&mut self) {
        if let Some(running) = self.search.take() {
            running.stop.store(true, Ordering::Relaxed);
            // A panic in the search thread has already been reported by the panic hook; there
            // is nothing further to do with it here.
            let _ = running.thread.join();
        }
    }
}

/// The move to offer to ponder on: the second move of the principal variation, spelled in the
/// position it is played in rather than at the root. `None` when the search left no line to
/// speak of, which is what an aborted first iteration leaves.
fn ponder_move(board: &mut Board, best: Move, pv: &[Move], chess960: bool) -> Option<String> {
    if pv.first() != Some(&best) {
        return None;
    }
    let reply = *pv.get(1)?;
    board.make_move(best);
    let legal = generate_legal(board);
    let spelled = legal
        .contains(reply)
        .then(|| to_uci(reply, &legal, chess960));
    board.unmake_move(best);
    spelled
}

/// The start position. `START_FEN` is a constant the corpus pins, so this cannot fail; `expect`
/// rather than `?` because there is nothing sensible for a UCI session to do without a board.
fn start_position() -> Board {
    #[allow(clippy::expect_used, reason = "a constant FEN")]
    Board::from_fen(START_FEN).expect("the start position parses")
}

/// Print one line to stdout, atomically with respect to the other thread, and flush: a GUI that
/// has sent `isready` is blocked until it sees `readyok`, so a buffered reply is a hang.
fn say(line: std::fmt::Arguments<'_>) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The loop: stdin to `Session::handle_line`, until `quit` or end of input.
#[must_use]
pub fn run() -> ExitCode {
    // The startup banner, before any input is read: running `cadence` with stdin at end-of-file
    // prints the identity and exits, which is the cheapest smoke test there is.
    say(format_args!(
        "{ENGINE_NAME} {ENGINE_VERSION} by {ENGINE_AUTHOR}"
    ));

    let mut session = Session::new();
    let mut stdin = std::io::stdin().lock();
    let mut raw = Vec::new();
    loop {
        // Bytes, not `lines()`. `BufRead::lines` yields `Err` for a line that is not valid
        // UTF-8, and treating that as end of input ends the session silently -- exit 0, no
        // message, a GUI reporting a crash with nothing to attribute it to.
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
