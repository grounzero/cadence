// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{Colour, Move, MoveList, generate_legal, parse_uci, to_uci};
use cadence_nnue::AnyNetwork;

use super::say::say;
use super::{Played, Running, Session, Thinking};
use crate::level;
use crate::position::Position;
use crate::search::{Learned, Limits, Search};
use crate::syzygy::{Rank, Tables};
use crate::time;
use crate::tt::Table;
use crate::tune::Tunables;

/// One frame per ply to `MAX_PLY`, each holding a `MoveList`; 16 MiB is far above that in any
/// profile, for every thread that searches, datagen's workers included.
pub(crate) const SEARCH_STACK_BYTES: usize = 16 << 20;

impl Session {
    /// After a failed `position` a search still running is stopped and nothing is searched: the
    /// answer is `bestmove 0000`, saying why.
    pub(super) fn go_unless_refused<'a>(&mut self, tokens: impl Iterator<Item = &'a str>) {
        let Some(why) = self.refused.clone() else {
            self.go(tokens);
            return;
        };
        self.stop_search();
        say(format_args!(
            "info string go: the last position command failed ({why}); searching nothing"
        ));
        say(format_args!("bestmove 0000"));
    }

    /// A search still running is stopped first.
    pub(super) fn go<'a>(&mut self, tokens: impl Iterator<Item = &'a str>) {
        // The GUI's clock for this move started when it sent the line.
        let received = Instant::now();
        self.stop_search();
        let tokens: Vec<&str> = tokens.collect();
        let (limits, thinking) = self.limits_for(&tokens, received);
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
        let mut board = self.board.duplicate();
        let Some(root_filter) = search_moves(&tokens, &generate_legal(&board)) else {
            say(format_args!(
                "info string go: searchmoves names no legal move here; searching nothing"
            ));
            say(format_args!("bestmove 0000"));
            return;
        };
        // Owed whatever happens next, so spelled before anything can fail to start.
        let fallback = first_answer(&board, &root_filter, self.chess960);
        let (tables, net) = (self.tables.clone(), self.active_net());
        let chess960 = self.chess960;
        let level = self.level();
        let (threads, multipv) = self.threads_and_lines(level.is_some());
        let ponder = self.ponder;
        let tunables = self.tunables;
        // Lent to the thread and returned by it, padded or cut to one set per thread the option
        // names, so a level keeps the helpers' sets.
        let mut learned = std::mem::take(&mut self.learned);
        learned.resize_with(self.threads, Learned::new);
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
                    let hard = time::budget(&limits, board.side_to_move())
                        .map(|b| Duration::from_millis(b.hard));
                    let go_on = || probing_allowed(&stop, received, hard);
                    let (root_filter, decided, rank) =
                        tablebase_root(tables.as_deref(), &mut board, root_filter, &go_on);
                    let mut pos = position_for(board, net);
                    // So the `bestmove` is spelled in one place whatever `Threads` is.
                    let go = ParallelGo {
                        board: &mut pos,
                        limits,
                        received,
                        stop: &stop,
                        ponder_hit: &ponder_hit,
                        tt: &tt,
                        chess960,
                        threads,
                        multipv,
                        level,
                        tunables,
                        root_filter: &root_filter,
                        tables: tables.as_ref(),
                        decided,
                        rank,
                        learned: &mut learned,
                    };
                    let (best, pv) = if threads == 1 {
                        single_search(go)
                    } else {
                        parallel_search(go)
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
                    (learned, Instant::now())
                })
        };
        match thread {
            Ok(thread) => {
                self.search = Some(Running {
                    stop,
                    ponder_hit,
                    thread,
                    thinking,
                });
            }
            Err(e) => not_started(&e, &fallback),
        }
    }

    /// The threads and lines a `go` searches with: a level owns the line count, and its move is
    /// reproducible only on one thread, so it takes one of each and says so.
    fn threads_and_lines(&self, level: bool) -> (usize, usize) {
        if !level {
            return (self.threads, self.multipv);
        }
        if self.threads > 1 || self.multipv > 1 {
            say(format_args!(
                "info string go: UCI_LimitStrength is on, so this search runs on one thread \
                 and shows one line"
            ));
        }
        (1, 1)
    }

    /// The search keeps its tree and spends the clock from here. A flag, because the search that
    /// must hear it is already running.
    pub(super) fn ponderhit(&mut self) {
        if let Some(running) = &mut self.search {
            running.ponder_hit.store(true, Ordering::Relaxed);
            // The move is ours from here, and so is the clock.
            if let Some(thinking) = &mut running.thinking {
                thinking.from = Instant::now();
                thinking.pondering = false;
            }
        }
    }

    /// The limits a `go` searches with, the session's overhead, lag and move number among them,
    /// and the clock the move is answered on where it has one of its own.
    fn limits_for(&mut self, tokens: &[&str], received: Instant) -> (Limits, Option<Thinking>) {
        let mut limits = Limits::parse(tokens.iter().copied());
        limits.move_overhead = Some(self.move_overhead);
        let us = self.board.side_to_move();
        self.read_lag(&limits, us);
        limits.lag = self.lag;
        limits.move_number = Some(u32::from(self.board.fullmove_number()));
        let thinking = if limits.infinite || limits.movetime.is_some() {
            None
        } else {
            limits.clock(us).map(|(clock, inc)| Thinking {
                side: us,
                clock,
                inc,
                from: received,
                pondering: limits.ponder,
            })
        };
        (limits, thinking)
    }

    /// What our last move lost beyond its thinking: the clock it left, less the clock this `go`
    /// gives. Only our own clock in one game says anything of our move.
    fn read_lag(&mut self, limits: &Limits, us: Colour) {
        let (Some(played), Some((clock, _))) = (self.played.take(), limits.clock(us)) else {
            return;
        };
        if played.side == us {
            let left = played
                .clock
                .saturating_add(played.inc)
                .saturating_sub(played.spent);
            self.lag = Some(time::fold_lag(self.lag, left.saturating_sub(clock)));
        }
    }

    /// The search thread prints its `bestmove` on the way out and returns the histories it was
    /// lent. One that failed to start or panicked took them with it, so the next `go` starts empty.
    pub(super) fn stop_search(&mut self) {
        if let Some(running) = self.search.take() {
            running.stop.store(true, Ordering::Relaxed);
            // The panic hook has already reported a panic in the search thread.
            if let Ok((learned, answered)) = running.thread.join() {
                self.learned = learned;
                // A ponder never hit was not a move played.
                self.played = running.thinking.filter(|t| !t.pondering).map(|t| Played {
                    side: t.side,
                    clock: t.clock,
                    inc: t.inc,
                    spent: u64::try_from(answered.saturating_duration_since(t.from).as_millis())
                        .unwrap_or(u64::MAX),
                });
            }
        }
    }

    /// After `stop_search`, which has the search's histories back.
    pub(super) fn clear_learned(&mut self) {
        for learned in &mut self.learned {
            learned.clear();
        }
    }
}

/// The first `searchmoves` move, else the first legal move, else none, spelled as `bestmove`
/// spells it.
fn first_answer(board: &Board, filter: &[Move], chess960: bool) -> String {
    let legal = generate_legal(board);
    match filter.first().or(legal.as_slice().first()) {
        Some(&m) => to_uci(m, &legal, chess960),
        None => "0000".to_string(),
    }
}

/// A GUI waits for `bestmove` after `go`, so a search that cannot start still answers.
fn not_started(e: &std::io::Error, fallback: &str) {
    say(format_args!(
        "info string go: could not start the search thread: {e}"
    ));
    say(format_args!("bestmove {fallback}"));
}

/// Only the primary reports and chooses; helpers start from rotated root orders and reach each
/// other only through the lockless table.
struct ParallelGo<'a> {
    board: &'a mut Position,
    limits: Limits,
    received: Instant,
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
    root_filter: &'a [Move],
    tables: Option<&'a Arc<Tables>>,
    /// The root's ranking settled the result, so no thread probes below it.
    decided: bool,
    /// Only the primary reports, so only it is given what the tables read at the root.
    rank: Option<Rank>,
    /// One set per thread, the primary's first; each worker searches with its own, moved to it and
    /// back without a replacement being built.
    learned: &'a mut Vec<Learned>,
}

/// One thread, which reports and samples a level's move itself.
fn single_search(go: ParallelGo<'_>) -> (Move, Vec<Move>) {
    let ParallelGo {
        board,
        limits,
        received,
        stop,
        ponder_hit,
        tt,
        chess960,
        multipv,
        level,
        tunables,
        root_filter,
        tables,
        decided,
        rank,
        learned,
        ..
    } = go;
    let mut out = std::io::stdout();
    let mut search = Search::new(stop, tt);
    search.swap_learned(&mut learned[0]);
    search.set_limits(limits);
    search.set_clock_origin(received);
    search.set_ponder_hit(ponder_hit);
    search.set_chess960(chess960);
    search.set_lines(multipv, level);
    search.set_tunables(tunables);
    search.set_root_filter(root_filter);
    if let Some(tables) = tables {
        search.set_tables(tables, decided);
    }
    search.set_root_rank(rank);
    let mut best = search.run(board, &mut out);
    if let Some(policy) = level {
        best = search.sample(policy, board, &mut out);
    }
    search.swap_learned(&mut learned[0]);
    (best, search.pv().to_vec())
}

/// The generation advances once here, not per worker; the shared node slots keep `nodes` and `nps`
/// about the group.
fn parallel_search(go: ParallelGo<'_>) -> (Move, Vec<Move>) {
    let ParallelGo {
        board,
        limits,
        received,
        stop,
        ponder_hit,
        tt,
        chess960,
        threads,
        multipv,
        level,
        tunables,
        root_filter,
        tables,
        decided,
        rank,
        learned,
    } = go;
    tt.new_search();
    let nodes: Arc<[AtomicU64]> = (0..threads)
        .map(|_| AtomicU64::new(0))
        .collect::<Vec<_>>()
        .into();
    let mut helpers = Vec::with_capacity(threads.saturating_sub(1));
    // Moved out by value, the vector keeping its buffer, so the sets are pushed back without a
    // reallocation.
    for (worker_index, mut helper_learned) in (1..threads).zip(learned.drain(1..)) {
        let mut helper_board = board.fork();
        let helper_stop = Arc::clone(stop);
        let helper_ponder_hit = Arc::clone(ponder_hit);
        let helper_tt = Arc::clone(tt);
        let helper_nodes = Arc::clone(&nodes);
        let helper_filter = root_filter.to_vec();
        let helper_tables = tables.cloned();
        let result = std::thread::Builder::new()
            .name(format!("search-helper-{worker_index}"))
            .stack_size(SEARCH_STACK_BYTES)
            .spawn(move || {
                let mut sink = std::io::sink();
                let mut search = Search::new(&helper_stop, &helper_tt);
                search.swap_learned(&mut helper_learned);
                search.set_limits(limits);
                search.set_clock_origin(received);
                search.set_ponder_hit(&helper_ponder_hit);
                search.set_chess960(chess960);
                search.set_lines(multipv, level);
                search.set_tunables(tunables);
                search.set_parallel(worker_index, &helper_nodes);
                search.set_root_filter(&helper_filter);
                if let Some(tables) = &helper_tables {
                    search.set_tables(tables, decided);
                }
                #[cfg(feature = "counters")]
                report_given(worker_index, &search);
                search.run_in_current_generation(&mut helper_board, &mut sink);
                search.swap_learned(&mut helper_learned);
                helper_learned
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
    search.swap_learned(&mut learned[0]);
    search.set_limits(limits);
    search.set_clock_origin(received);
    search.set_ponder_hit(ponder_hit);
    search.set_chess960(chess960);
    search.set_lines(multipv, level);
    search.set_tunables(tunables);
    search.set_parallel(0, &nodes);
    search.set_root_filter(root_filter);
    if let Some(tables) = tables {
        search.set_tables(tables, decided);
    }
    search.set_root_rank(rank);
    #[cfg(feature = "counters")]
    report_given(0, &search);
    let mut best = search.run_in_current_generation(board, &mut out);
    if let Some(policy) = level {
        best = search.sample(policy, board, &mut out);
    }
    let pv = search.pv().to_vec();
    search.swap_learned(&mut learned[0]);

    // Helpers check the flag at every node, so this is the whole shutdown.
    stop.store(true, Ordering::Relaxed);
    // A helper that failed to start or panicked took its set with it: the sets after it move down a
    // thread, and the next `go` pads the last with an empty one.
    for helper in helpers {
        if let Ok(returned) = helper.join() {
            learned.push(returned);
        }
    }
    (best, pv)
}

/// Test builds only: what a thread's search holds as it starts, so a gate can hold every helper to
/// the primary. The learned tables go on a line of their own, which differs by thread.
#[cfg(feature = "counters")]
fn report_given(worker_index: usize, search: &Search<'_>) {
    say(format_args!(
        "info string search thread {worker_index} given {:?} {:?} filter {:?} probes {}",
        search.limits(),
        search.given_tunables(),
        search.root_filter(),
        search.probes()
    ));
    say(format_args!(
        "info string search learned {worker_index} {:?}",
        search.learned_written()
    ));
}

/// Asked before each of the root's probes, so a stop or a spent budget waits for one probe at most.
/// A read the operating system has blocked finishes before the next question is asked.
fn probing_allowed(stop: &AtomicBool, received: Instant, hard: Option<Duration>) -> bool {
    !stop.load(Ordering::Relaxed) && hard.is_none_or(|hard| received.elapsed() < hard)
}

/// The root's ranking narrows `filter`, the `searchmoves` list or empty for every move, and says
/// whether it settled the result and what it reads. Untouched where no table covers the root, or
/// where `go_on` stops it before it ends.
fn tablebase_root(
    tables: Option<&Tables>,
    board: &mut Board,
    filter: Vec<Move>,
    go_on: &dyn Fn() -> bool,
) -> (Vec<Move>, bool, Option<Rank>) {
    let Some(tables) = tables else {
        return (filter, false, None);
    };
    tables.reset_hits();
    match tables.rank_root_while(board, &filter, go_on) {
        Some(ranked) => (ranked.moves, ranked.decided, Some(ranked.verdict)),
        None => (filter, false, None),
    }
}

/// The words that end a `searchmoves` list, which UCI lets any of them follow.
const GO_KEYWORDS: [&str; 12] = [
    "searchmoves",
    "ponder",
    "wtime",
    "btime",
    "winc",
    "binc",
    "movestogo",
    "depth",
    "nodes",
    "mate",
    "movetime",
    "infinite",
];

/// The moves after `searchmoves`, empty where the word is absent; one not legal here is named and
/// left out. `None` is a list naming no legal move, an empty one included, so nothing is searched.
fn search_moves(tokens: &[&str], legal: &MoveList) -> Option<Vec<Move>> {
    let Some(at) = tokens.iter().position(|&t| t == "searchmoves") else {
        return Some(Vec::new());
    };
    let mut moves = Vec::new();
    let named = tokens[at + 1..]
        .iter()
        .take_while(|t| !GO_KEYWORDS.contains(t));
    for &token in named {
        match parse_uci(legal, token) {
            Some(m) => moves.push(m),
            None => say(format_args!(
                "info string go: searchmoves {token} is not a legal move here; left out"
            )),
        }
    }
    (!moves.is_empty()).then_some(moves)
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

/// Scored by the net where one is active, by the handcrafted evaluation otherwise.
fn position_for(board: Board, net: Option<AnyNetwork>) -> Position {
    match net {
        Some(net) => Position::with_net(board, net),
        None => Position::new(board),
    }
}
