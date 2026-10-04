// SPDX-License-Identifier: GPL-3.0-or-later

//! The search, and what bounds it. `Limits` is the parsed `go` command.

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::Instant;

use cadence_core::position::Board;
use cadence_core::{MAX_PLY, Move, MoveList};

use crate::corrhist::CorrectionHistory;
use crate::history::History;
use crate::score::{DRAW, Score};
use crate::time::{self, Budget};
use crate::tt::{Bound, Table};
use crate::tune::Tunables;

mod counters;
mod depth;
mod limits;
mod node;
mod pruning;
mod pv;
mod quiesce;
mod report;
mod root;

pub use depth::{
    REDUCTION_INDEX, extension, history_reduction, lmr_reduction, null_reduction, reduction,
};
pub use limits::Limits;
pub(crate) use pruning::{LMP_MULTIPLIER, REVERSE_FUTILITY_MARGIN};
pub use pruning::{
    PROBCUT_REDUCTION, futile_node, futility_margin, futility_skips, has_non_pawn_material,
    improving, lmp_count, lmp_index, lmp_skips, probcut_bound, reverse_futile,
    reverse_futility_margin,
};
use pv::PvTable;

/// How often the clock is read, in nodes. A power of two.
const CLOCK_INTERVAL: u64 = 1024;

/// How often a parallel worker publishes its local node count, in nodes. Each worker owns one
/// slot and nothing else writes it, so a publication is a plain store; a parallel `go nodes`
/// may overshoot by fewer than this many nodes per other worker.
const NODE_PUBLISH_INTERVAL: u64 = 64;

/// How long a search runs before it starts naming the root move it is on, in milliseconds. The
/// conventional second: below it a game at any club control finishes the move without ever
/// saying `currmove`, so nothing is written down a pipe that a watcher could not have read
/// anyway.
pub const CURRMOVE_AFTER_MS: u64 = 1000;

/// The deepest iteration: `MAX_PLY`, which is the state stack's bound. It is no longer the
/// deepest ply either search reaches -- a check extension gives a ply back, and [`extension`]
/// states the bound that replaces it -- so both searches stop at `MAX_PLY` whatever depth is
/// left and stand on the evaluation there.
const MAX_DEPTH: u32 = MAX_PLY as u32;
const _: () = assert!(MAX_DEPTH as usize == MAX_PLY);

/// Which bound a node's fail-soft value carries: a lower one where it reached beta, an exact one
/// where it beat the alpha the node started with, an upper one otherwise. Out of `negamax`
/// because the line-count limit says so, and pinned as arithmetic in `tests/probcut.rs`.
#[must_use]
pub fn bound_for(best: Score, original_alpha: Score, beta: Score) -> Bound {
    if best >= beta {
        Bound::Lower
    } else if best > original_alpha {
        Bound::Exact
    } else {
        Bound::Upper
    }
}

/// Move `first` to the head of `list`, keeping the rest in the order they were generated in.
/// Returns whether `list` held it at all.
#[must_use]
pub fn order_first(list: &mut MoveList, first: Move) -> bool {
    if first.is_null() {
        return false;
    }
    let moves = list.as_mut_slice();
    let Some(i) = moves.iter().position(|&m| m == first) else {
        return false;
    };
    moves[..=i].rotate_right(1);
    true
}

/// Remember `m` as a killer of the ply `killers` belongs to: the quiet move that caused a beta
/// cutoff at a sibling of this node, newest first, with no duplicates.
pub fn remember_killer(killers: &mut [Move; 2], m: Move) {
    if m.is_noisy() || killers[0] == m {
        return;
    }
    killers[1] = killers[0];
    killers[0] = m;
}

/// One search. All state is here, per thread; nothing is global.
pub struct Search<'a> {
    limits: Limits,
    stop: &'a AtomicBool,
    /// The flag a `ponderhit` raises, read at the iteration boundary and inside
    /// [`Search::out_of_time`]'s node interval. It is the stop flag's shape rather than a
    /// replaced `Limits`, because the search that has to hear about it is already running.
    ponder_hit: Option<&'a AtomicBool>,
    /// The transposition table. Shared with whoever else searches: the UCI session keeps one
    /// across the whole game, `bench` clears one between positions.
    tt: &'a Table,
    /// Spells castling moves in `info` lines the way the GUI expects.
    chess960: bool,
    /// The values the pruning rules read for the constants a tune may move. Compiled-in unless
    /// the UCI session set others, which is why `bench`, building its own searches, never sees
    /// a setting.
    tunables: Tunables,
    nodes: u64,
    /// Every worker's slot, and this worker's index into it, while a group is searching.
    /// `None` on the single-thread path, which is where `bench` runs and where the exact
    /// node-count contract lives.
    shared_nodes: Option<(&'a [AtomicU64], usize)>,
    /// Zero for the primary. A helper rotates its root list left by this much before the first
    /// iteration, which is the group's only explicit divergence: measured at 8 to 12 percent of
    /// time to depth at 4 to 16 threads, all of it in the ordering of the tail, because every
    /// worker promotes its own best move to the front once an iteration completes.
    worker_index: usize,
    start: Instant,
    /// The time budget, when anything in `limits` constrains the time. `None` means the clock
    /// is never read.
    budget: Option<Budget>,
    /// Whether this search is still pondering, which is `limits.ponder` until a hit is
    /// absorbed. A field rather than the limit itself, because the limit records what the `go`
    /// asked for and this records what the search is now doing.
    pondering: bool,
    /// The budget a `ponderhit` installs, from the clock the `go ponder` carried. Derived at
    /// the head of the run because the hit is absorbed on a path with no board to read the side
    /// to move from.
    budget_on_hit: Option<Budget>,
    /// Set when a limit or the stop flag ends the search mid-iteration; the iteration's partial
    /// result is discarded, unless it is the first iteration's, which is all there is.
    aborted: bool,
    completed_depth: u32,
    best: Move,
    score: Score,
    /// The principal variation of the last completed iteration.
    pv: Vec<Move>,
    /// The deepest ply either search reached in the iteration in progress, which is what
    /// `seldepth` reports. Written at every node and read by nothing that decides anything, so
    /// two searches that differ only in this field visit the same nodes in the same order.
    seldepth: usize,
    /// The triangular table the iteration in progress writes.
    table: PvTable,
    /// Two quiet moves per ply: the ones that caused a beta cutoff at a node of that ply, tried
    /// at its other nodes ahead of the quiet moves that have refuted nothing. Indexed by ply,
    /// cleared when a search starts, and a kibibyte inline like `PvTable`'s row lengths.
    killers: [[Move; 2]; MAX_PLY],
    /// What each quiet move has been worth across this whole search: the butterfly table
    /// `picker` ranks the quiet band by and the reduction reads through [`history_reduction`].
    /// Cleared beside the killers, so the two have one lifetime; thirty-two kibibytes, on the
    /// heap for `PvTable`'s reason rather than inline for `killers`'.
    history: History,
    /// What the evaluation has been wrong by, per pawn structure and side, across this whole
    /// search. Cleared beside the killers and the history, so the three have one lifetime.
    corrhist: CorrectionHistory,
    /// How many observations were folded into the correction table, and at how many nodes a
    /// non-zero correction was read back. Written where the rule runs and read on no decision
    /// path.
    corrhist_updates: u64,
    corrhist_applied: u64,
    /// The depth of the iteration in progress, which is what the check extension's ply cap is a
    /// multiple of. Set at the head of each iteration; a field rather than a sixth argument to
    /// `negamax` because it does not change inside one.
    root_depth: u32,
    /// The static evaluation at each ply of the line being searched: written at every interior
    /// node of the main search that survives the table probe, `None` where the side to move is
    /// in check, because a position under attack has no quiet reading worth comparing. Two
    /// kibibytes inline, like `killers`.
    evals: [Option<Score>; MAX_PLY],
    /// How often the search tried a null move, cut on one, and refused one for material alone.
    /// Written wherever the rule runs and read on no decision path, so a depth-limited search
    /// stays a function of the code alone; what reads them is `tests/pruning.rs`, whose gates
    /// need to see that the pruning happened, not only that the count moved.
    null_attempts: u64,
    null_cutoffs: u64,
    /// Every other condition admitted the null move and the side to move had nothing but pawns
    /// beside the king. The zugzwang gate asserts this is the reason a pawn endgame never tried
    /// one, rather than the question never coming up.
    null_refused_material: u64,
    /// How often a late move's first search ran at reduced depth, and how often that reduced
    /// search beat alpha and was re-run at full depth before being believed. The first is how a
    /// gate sees that later moves were searched shallower, the second that a reduced fail-high
    /// was verified rather than trusted.
    lmr_reductions: u64,
    lmr_researches: u64,
    /// How often a history score shortened a reduction the index had already decided on, and
    /// how often it lengthened one. Two rather than one because the malus is the half that
    /// produces a negative score, so a gate that sees only the first cannot tell a table that
    /// credits from a table that also debits.
    history_reduced_less: u64,
    history_reduced_more: u64,
    /// How often the margin admitted a node, how many quiet moves it skipped there, and how
    /// often it would have skipped one and did not because the move gives check. The third is
    /// the shape [`Search::null_refused_by_material`] has: a gate asserting that an exempt move
    /// was searched needs to see the exemption *decide*, not merely see that no counterexample
    /// turned up, and a rule that never met a checking move at a futile node would pass a gate
    /// written the other way.
    futility_nodes: u64,
    futility_skipped: u64,
    futility_kept_check: u64,
    /// How often a node was one this rule could act at, how many quiet moves it gave up there,
    /// and how often it would have given one up and did not because the move gives check. **The
    /// first two are not comparable with the margin's, and the reason is the order in the
    /// loop.** The margin is asked first and keeps the moves it was already taking, so these
    /// count what this rule *adds*.
    lmp_nodes: u64,
    lmp_skipped: u64,
    lmp_kept_check: u64,
    /// How often the margin returned a node without searching it, and how often it would have
    /// and did not because the node had the full window. The second is the shape
    /// [`Search::null_refused_by_material`] has and it is here for the same reason: a gate
    /// asserting that a full-window node was searched needs to see the refusal *decide*, and a
    /// tree in which no full-window node ever cleared the margin would pass a gate written the
    /// other way.
    reverse_futility_cutoffs: u64,
    reverse_futility_refused_window: u64,
    /// How often the capture probe ran at a node, how many captures it searched at reduced depth
    /// once the quiescence screen passed them, and how often one of those cut the node. The
    /// fourth is how often it would have run and did not because the node had the full window.
    probcut_attempts: u64,
    probcut_searches: u64,
    probcut_cutoffs: u64,
    probcut_refused_window: u64,
    /// How many check evasion lists the quiescence search prepared, and how many of those the
    /// sort moved a new move to the head of. The first is the shape [`Search::futility_nodes`]
    /// has and it is here for the same reason: what the ordering is worth is no longer visible
    /// in a node count, so the gate reads the decision, and a gate that saw only the second
    /// would pass a search that had stopped reaching an in-check horizon at all.
    evasion_lists: u64,
    evasion_lists_reordered: u64,
    /// Elapsed milliseconds at the end of each completed iteration, in order, and empty where
    /// there is no budget. Written from the reading the soft-budget test already takes, so it
    /// adds no clock read anywhere, and under a depth or node limit it adds no entry either:
    /// `bench` leaves this empty and reads no clock, which is the contract `tests/time.rs`
    /// pins.
    iterations: Vec<u64>,
    /// The root move and the score at the end of each completed iteration, in order, which is
    /// the state a rule spending on how long the root move has stood reads. Written where the
    /// iteration is accepted and read by nothing that decides anything, and unlike `iterations`
    /// it costs no clock read, so it is kept under a depth limit too.
    roots: Vec<(Move, Score)>,
    /// How many principal variations the root reports, from `MultiPV`. One
    /// is the default and the only value a test or a rating list plays.
    multipv: usize,
    /// The lines the iteration in progress found, best first once it is
    /// accepted. One entry at `MultiPV` 1, which is the pv `report` prints.
    lines: Vec<RootLine>,
    /// The lines of the last iteration that was accepted, which is what a level
    /// samples among. Separate from `lines` because an aborted iteration leaves
    /// that one holding however much of itself it finished, and every limit but
    /// a fixed depth aborts.
    accepted: Vec<RootLine>,
}

/// One reported principal variation: the root move, its score, and the line the iteration
/// ended on. The pv is owned rather than read back off [`PvTable`], because the table holds one
/// root line and a second search of the root overwrites it.
struct RootLine {
    mv: Move,
    score: Score,
    pv: Vec<Move>,
}

impl<'a> Search<'a> {
    #[must_use]
    pub fn new(stop: &'a AtomicBool, tt: &'a Table) -> Search<'a> {
        Search {
            limits: Limits::default(),
            stop,
            ponder_hit: None,
            tt,
            chess960: false,
            tunables: Tunables::DEFAULT,
            nodes: 0,
            shared_nodes: None,
            worker_index: 0,
            start: Instant::now(),
            budget: None,
            pondering: false,
            budget_on_hit: None,
            aborted: false,
            completed_depth: 0,
            best: Move::NULL,
            score: DRAW,
            pv: Vec::with_capacity(MAX_PLY),
            seldepth: 0,
            table: PvTable::new(),
            killers: [[Move::NULL; 2]; MAX_PLY],
            history: History::new(),
            corrhist: CorrectionHistory::new(),
            corrhist_updates: 0,
            corrhist_applied: 0,
            root_depth: 0,
            evals: [None; MAX_PLY],
            null_attempts: 0,
            null_cutoffs: 0,
            null_refused_material: 0,
            lmr_reductions: 0,
            lmr_researches: 0,
            history_reduced_less: 0,
            history_reduced_more: 0,
            futility_nodes: 0,
            futility_skipped: 0,
            futility_kept_check: 0,
            lmp_nodes: 0,
            lmp_skipped: 0,
            lmp_kept_check: 0,
            reverse_futility_cutoffs: 0,
            reverse_futility_refused_window: 0,
            probcut_attempts: 0,
            probcut_searches: 0,
            probcut_cutoffs: 0,
            probcut_refused_window: 0,
            evasion_lists: 0,
            evasion_lists_reordered: 0,
            iterations: Vec::new(),
            roots: Vec::new(),
            multipv: 1,
            lines: Vec::new(),
            accepted: Vec::new(),
        }
    }

    /// What the `go` asked for, which `begin` reads once at the head of each run. A setter
    /// rather than a constructor argument, so that one search can serve more than one `go`.
    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    /// The flag to watch for a `ponderhit`. A search built without one still refuses to answer
    /// a `go ponder` before `stop`, which is what a session that never sends the hit gets.
    pub fn set_ponder_hit(&mut self, flag: &'a AtomicBool) {
        self.ponder_hit = Some(flag);
    }

    /// Spell castling moves in `info` lines per `UCI_Chess960`.
    pub fn set_chess960(&mut self, on: bool) {
        self.chess960 = on;
    }

    /// Search with `tunables` in place of the compiled-in values, from the next run on.
    pub fn set_tunables(&mut self, tunables: Tunables) {
        self.tunables = tunables;
    }

    /// Report `n` principal variations, clamped to at least one. A root with fewer moves than
    /// this reports the moves it has, and at one it reports exactly what it did before this
    /// option existed.
    pub fn set_multipv(&mut self, n: usize) {
        self.multipv = n.max(1);
    }

    /// Clear everything one run owns and derive its budgets, so a `Search` reused for a second
    /// `go` starts where a fresh one would. The clock origin is set here and moved again only
    /// by a `ponderhit`.
    fn begin(&mut self, board: &Board) {
        self.start = Instant::now();
        self.nodes = 0;
        self.aborted = false;
        self.completed_depth = 0;
        self.best = Move::NULL;
        self.pv.clear();
        self.seldepth = 0;
        self.killers = [[Move::NULL; 2]; MAX_PLY];
        self.history.clear();
        self.evals = [None; MAX_PLY];
        self.null_attempts = 0;
        self.null_cutoffs = 0;
        self.null_refused_material = 0;
        self.lmr_reductions = 0;
        self.lmr_researches = 0;
        self.history_reduced_less = 0;
        self.history_reduced_more = 0;
        self.corrhist.clear();
        self.corrhist_updates = 0;
        self.corrhist_applied = 0;
        self.futility_nodes = 0;
        self.futility_skipped = 0;
        self.futility_kept_check = 0;
        self.lmp_nodes = 0;
        self.lmp_skipped = 0;
        self.lmp_kept_check = 0;
        self.reverse_futility_cutoffs = 0;
        self.reverse_futility_refused_window = 0;
        self.probcut_attempts = 0;
        self.probcut_searches = 0;
        self.probcut_cutoffs = 0;
        self.probcut_refused_window = 0;
        self.evasion_lists = 0;
        self.evasion_lists_reordered = 0;
        self.iterations.clear();
        self.roots.clear();
        self.lines.clear();
        self.accepted.clear();
        self.pondering = self.limits.ponder;
        self.budget = if self.limits.infinite {
            None
        } else {
            time::budget(&self.limits, board.side_to_move())
        };
        // What the clock the `go ponder` carried is worth once a hit makes the time ours. A
        // ponder has no budget of its own, so this is held rather than derived where it lands.
        self.budget_on_hit = if self.pondering {
            let mut clocked = self.limits;
            clocked.ponder = false;
            time::budget(&clocked, board.side_to_move())
        } else {
            None
        };
    }

    /// Join a group as worker `worker_index`, publishing this worker's count into `nodes`.
    /// Every worker keeps its own history, killers and principal variation; the table, the stop
    /// flag and these slots are all that a group shares.
    pub(crate) fn set_parallel(&mut self, worker_index: usize, nodes: &'a [AtomicU64]) {
        self.worker_index = worker_index;
        self.shared_nodes = Some((nodes, worker_index));
    }
}
