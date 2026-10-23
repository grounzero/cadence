// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cadence_core::{Move, MoveList, generate_legal, parse_uci, to_uci};

use super::say::say;
use super::{Running, Session};
use crate::history::History;
use crate::level;
use crate::position::Position;
use crate::search::{Limits, Search};
use crate::tt::Table;
use crate::tune::Tunables;

/// One frame per ply to `MAX_PLY`, each holding a `MoveList`; 16 MiB is far above that in any
/// profile.
const SEARCH_STACK_BYTES: usize = 16 << 20;

impl Session {
    /// A search still running is stopped first.
    pub(super) fn go<'a>(&mut self, tokens: impl Iterator<Item = &'a str>) {
        self.stop_search();
        let tokens: Vec<&str> = tokens.collect();
        let mut limits = Limits::parse(tokens.iter().copied());
        limits.move_overhead = Some(self.move_overhead);
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
        let root_filter = search_moves(&tokens, &generate_legal(&board));
        let chess960 = self.chess960;
        let level = self.level();
        let multipv = level.map_or(self.multipv, |policy| policy.candidates);
        let ponder = self.ponder;
        let threads = self.threads;
        let tunables = self.tunables;
        let history = std::mem::take(&mut self.history);
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
                    let (best, pv, history) = if threads == 1 {
                        let mut out = std::io::stdout();
                        let mut search = Search::new(&stop, &tt);
                        search.set_limits(limits);
                        search.set_ponder_hit(&ponder_hit);
                        search.set_chess960(chess960);
                        search.set_multipv(multipv);
                        search.set_tunables(tunables);
                        search.set_root_filter(&root_filter);
                        search.set_history(history);
                        let mut best = search.run(&mut pos, &mut out);
                        if let Some(policy) = level {
                            best = search.sample(policy, &pos);
                        }
                        (best, search.pv().to_vec(), search.take_history())
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
                            root_filter: &root_filter,
                            history,
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
                    history
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
    pub(super) fn ponderhit(&mut self) {
        if let Some(running) = &self.search {
            running.ponder_hit.store(true, Ordering::Relaxed);
        }
    }

    /// The search thread prints its `bestmove` on the way out.
    pub(super) fn stop_search(&mut self) {
        if let Some(running) = self.search.take() {
            running.stop.store(true, Ordering::Relaxed);
            // The panic hook has already reported a panic in the search thread, and its history is
            // lost with it.
            self.history = running.thread.join().unwrap_or_default();
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
    root_filter: &'a [Move],
    /// Every worker starts from it; only the primary's is kept, since helpers report nothing.
    history: History,
}

/// The generation advances once here, not per worker; the shared node slots keep `nodes` and `nps`
/// about the group.
fn parallel_search(go: ParallelGo<'_>) -> (Move, Vec<Move>, History) {
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
        root_filter,
        history,
    } = go;
    tt.new_search();
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
        let helper_filter = root_filter.to_vec();
        let helper_history = history.clone();
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
                search.set_root_filter(&helper_filter);
                search.set_history(helper_history);
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
    search.set_root_filter(root_filter);
    search.set_history(history);
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
    (best, pv, search.take_history())
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

/// The moves after `searchmoves`; one not legal here is named and left out, and none legal leaves
/// the root unfiltered.
fn search_moves(tokens: &[&str], legal: &MoveList) -> Vec<Move> {
    let Some(at) = tokens.iter().position(|&t| t == "searchmoves") else {
        return Vec::new();
    };
    let mut moves = Vec::new();
    for &token in tokens[at + 1..]
        .iter()
        .take_while(|t| !GO_KEYWORDS.contains(t))
    {
        match parse_uci(legal, token) {
            Some(m) => moves.push(m),
            None => say(format_args!(
                "info string go: searchmoves {token} is not a legal move here; left out"
            )),
        }
    }
    moves
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
