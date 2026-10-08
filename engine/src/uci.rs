// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use cadence_core::Move;
use cadence_core::position::Board;
use cadence_core::{MAX_MOVES, START_FEN, generate_legal, parse_uci, to_uci};

use crate::level;
use crate::position::Position;
use crate::search::{Limits, Search};
use crate::tt::{self, Table};
use crate::tune::{self, Param, Tunables};

/// The version half is `version::VERSION`, not the package version, so a build off a release tag
/// names its commit.
const ENGINE_NAME: &str = "Cadence";
const ENGINE_VERSION: &str = crate::version::VERSION;
const ENGINE_AUTHOR: &str = "Michael Grounds";

/// One frame per ply to `MAX_PLY`, each holding a `MoveList`; 16 MiB is far above that in any
/// profile.
const SEARCH_STACK_BYTES: usize = 16 << 20;

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
                crate::corrhist_shadow::new_game();
            }
            "shadow" => {
                self.stop_search();
                crate::corrhist_shadow::report(|name, value| say(format_args!("{name} {value}")));
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

    // --- setoption ----------------------------------------------------------

    /// `name` and `value` delimit names and values, which may contain spaces.
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
        } else if name.eq_ignore_ascii_case("UCI_LimitStrength") {
            self.set_limit_strength(&value);
        } else if name.eq_ignore_ascii_case("UCI_Elo") {
            self.set_elo(&value);
        } else if name.eq_ignore_ascii_case("Ponder") {
            if value.eq_ignore_ascii_case("true") {
                self.ponder = true;
            } else if value.eq_ignore_ascii_case("false") {
                self.ponder = false;
            }
        } else if name.eq_ignore_ascii_case("Threads") {
            self.set_threads(&value);
        } else if let Some(param) = tune::find(&name) {
            self.set_tunable(param, &value);
        }
        // A GUI sends whatever it was told to.
    }

    /// Clamped, not refused, for [`Session::set_hash`]'s reason. A standing level holds it at one:
    /// a level's move is reproducible only where the search is.
    fn set_threads(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption Threads: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        let asked = asked.clamp(1, MAX_THREADS);
        if asked > 1 && self.limit_strength {
            say(format_args!(
                "info string setoption Threads: a level is reproducible only on one thread, \
                 so Threads stays at 1 while UCI_LimitStrength is on"
            ));
            return;
        }
        self.threads = asked;
    }

    /// A value that is not a number of the parameter's kind is ignored and the old value kept; it
    /// never reaches the search or ends the session.
    fn set_tunable(&mut self, param: &Param, value: &str) {
        match param.parse(value) {
            Some(stored) => param.set(&mut self.tunables, stored),
            None => say(format_args!(
                "info string setoption {}: `{value}` is not a number, keeping {}",
                param.name,
                param.spell(self.tunables.get(param.tunable))
            )),
        }
    }

    /// Clamped, not refused, for [`Session::set_hash`]'s reason.
    fn set_multipv(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption MultiPV: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        let asked = asked.clamp(1, MAX_MOVES);
        if asked > 1 && self.limit_strength {
            say(format_args!(
                "info string setoption MultiPV: a level owns the line count, so MultiPV stays \
                 at 1 while UCI_LimitStrength is on"
            ));
            return;
        }
        self.multipv = asked;
    }

    /// Refuses to engage beside `MultiPV` or `Threads` above one: the level owns the line count,
    /// and its move is reproducible only on one thread.
    fn set_limit_strength(&mut self, value: &str) {
        if value.eq_ignore_ascii_case("true") {
            if self.multipv > 1 {
                say(format_args!(
                    "info string setoption UCI_LimitStrength: UCI_Elo needs MultiPV at 1, so \
                     the level is not engaged"
                ));
                return;
            }
            if self.threads > 1 {
                say(format_args!(
                    "info string setoption UCI_LimitStrength: UCI_Elo needs Threads at 1, so \
                     the level is not engaged"
                ));
                return;
            }
            self.limit_strength = true;
        } else if value.eq_ignore_ascii_case("false") {
            self.limit_strength = false;
        }
    }

    /// Clamped into the ladder's range, and inert while `UCI_LimitStrength` is false.
    fn set_elo(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<u32>() else {
            say(format_args!(
                "info string setoption UCI_Elo: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        self.elo = asked.clamp(level::MIN_ELO, level::MAX_ELO);
    }

    /// The only place the two options are read together.
    fn level(&self) -> Option<level::Policy> {
        self.limit_strength.then(|| level::policy(self.elo))
    }

    /// Out-of-range values are clamped rather than refused: a GUI that sends one is not going to
    /// send another.
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

    // --- go / stop ----------------------------------------------------------

    /// A search still running is stopped first.
    fn go<'a>(&mut self, tokens: impl Iterator<Item = &'a str>) {
        self.stop_search();
        let limits = Limits::parse(tokens);
        // A clock not ours reads as zero, which returns the first iteration and looks like a broken
        // engine from the pipe, so say which it is.
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
        let board = self.board.duplicate();
        let chess960 = self.chess960;
        let level = self.level();
        let multipv = level.map_or(self.multipv, |policy| policy.candidates);
        let ponder = self.ponder;
        let threads = self.threads;
        let tunables = self.tunables;
        let thread = {
            let stop = Arc::clone(&stop);
            let ponder_hit = Arc::clone(&ponder_hit);
            // Its own handle, so a `setoption name Hash` mid-search replaces the session's table
            // without pulling this one from the thread.
            let tt = Arc::clone(&self.tt);
            // The default stack for a spawned thread is not something to rely on across platforms
            // and profiles.
            std::thread::Builder::new()
                .name("search".to_string())
                .stack_size(SEARCH_STACK_BYTES)
                .spawn(move || {
                    let legal = generate_legal(&board);
                    let mut pos = Position::new(board);
                    // So the `bestmove` is spelled in one place whatever `Threads` is.
                    let (best, pv) = if threads == 1 {
                        let mut out = std::io::stdout();
                        let mut search = Search::new(&stop, &tt);
                        search.set_limits(limits);
                        search.set_ponder_hit(&ponder_hit);
                        search.set_chess960(chess960);
                        search.set_multipv(multipv);
                        search.set_tunables(tunables);
                        let mut best = search.run(&mut pos, &mut out);
                        if let Some(policy) = level {
                            best = search.sample(policy, &pos);
                        }
                        (best, search.pv().to_vec())
                    } else {
                        parallel_search(ParallelGo {
                            board: &mut pos,
                            limits,
                            stop: &stop,
                            ponder_hit: &ponder_hit,
                            tt: &tt,
                            chess960,
                            threads,
                            multipv,
                            level,
                            tunables,
                        })
                    };
                    let spelled = to_uci(best, &legal, chess960);
                    // Only when the GUI said it ponders; otherwise the line is the one it always
                    // read.
                    match ponder
                        .then(|| ponder_move(&mut pos, best, &pv, chess960))
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

    /// The search keeps its tree and spends the clock from here. A flag, because the search that
    /// must hear it is already running.
    fn ponderhit(&mut self) {
        if let Some(running) = &self.search {
            running.ponder_hit.store(true, Ordering::Relaxed);
        }
    }

    /// The search thread prints its `bestmove` on the way out.
    fn stop_search(&mut self) {
        if let Some(running) = self.search.take() {
            running.stop.store(true, Ordering::Relaxed);
            // The panic hook has already reported a panic in the search thread.
            let _ = running.thread.join();
        }
    }
}

/// Only the primary reports and chooses; helpers start from rotated root orders and reach each
/// other only through the lockless table.
struct ParallelGo<'a> {
    board: &'a mut Position,
    limits: Limits,
    stop: &'a Arc<AtomicBool>,
    /// Given to every worker: a helper without it ponders all search long, and a primary without it
    /// never answers a `ponderhit`.
    ponder_hit: &'a Arc<AtomicBool>,
    tt: &'a Arc<Table>,
    chess960: bool,
    threads: usize,
    multipv: usize,
    /// Helpers never carry it: they report nothing, so a level on one buys a worse tree and no
    /// different move.
    level: Option<level::Policy>,
    tunables: Tunables,
}

/// The generation advances once here, not per worker; the shared node slots keep `nodes` and `nps`
/// about the group.
fn parallel_search(go: ParallelGo<'_>) -> (Move, Vec<Move>) {
    let ParallelGo {
        board,
        limits,
        stop,
        ponder_hit,
        tt,
        chess960,
        threads,
        multipv,
        level,
        tunables,
    } = go;
    tt.new_search();
    crate::corrhist_shadow::new_search();
    let nodes: Arc<[AtomicU64]> = (0..threads)
        .map(|_| AtomicU64::new(0))
        .collect::<Vec<_>>()
        .into();
    let mut helpers = Vec::with_capacity(threads.saturating_sub(1));
    for worker_index in 1..threads {
        let mut helper_board = Position::new(board.duplicate());
        let helper_stop = Arc::clone(stop);
        let helper_ponder_hit = Arc::clone(ponder_hit);
        let helper_tt = Arc::clone(tt);
        let helper_nodes = Arc::clone(&nodes);
        let result = std::thread::Builder::new()
            .name(format!("search-helper-{worker_index}"))
            .stack_size(SEARCH_STACK_BYTES)
            .spawn(move || {
                let mut sink = std::io::sink();
                let mut search = Search::new(&helper_stop, &helper_tt);
                search.set_limits(limits);
                search.set_ponder_hit(&helper_ponder_hit);
                search.set_chess960(chess960);
                search.set_multipv(multipv);
                search.set_tunables(tunables);
                search.set_parallel(worker_index, &helper_nodes);
                search.run_in_current_generation(&mut helper_board, &mut sink)
            });
        match result {
            Ok(helper) => helpers.push(helper),
            Err(e) => say(format_args!(
                "info string go: could not start helper {worker_index}: {e}"
            )),
        }
    }

    let mut out = std::io::stdout();
    let mut search = Search::new(stop, tt);
    search.set_limits(limits);
    search.set_ponder_hit(ponder_hit);
    search.set_chess960(chess960);
    search.set_multipv(multipv);
    search.set_tunables(tunables);
    search.set_parallel(0, &nodes);
    let mut best = search.run_in_current_generation(board, &mut out);
    if let Some(policy) = level {
        best = search.sample(policy, board);
    }
    let pv = search.pv().to_vec();

    // Helpers check the flag at every node, so this is the whole shutdown.
    stop.store(true, Ordering::Relaxed);
    for helper in helpers {
        let _ = helper.join();
    }
    (best, pv)
}

/// Spelled in the position it is played in, not at the root. `None` where an aborted first
/// iteration left no line.
fn ponder_move(board: &mut Position, best: Move, pv: &[Move], chess960: bool) -> Option<String> {
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

/// `START_FEN` is pinned by the corpus, so this cannot fail.
fn start_position() -> Board {
    #[allow(clippy::expect_used, reason = "a constant FEN")]
    Board::from_fen(START_FEN).expect("the start position parses")
}

/// Flushed: a GUI that sent `isready` blocks until it sees `readyok`, so a buffered reply is a
/// hang.
fn say(line: std::fmt::Arguments<'_>) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
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
