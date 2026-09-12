// SPDX-License-Identifier: GPL-3.0-or-later

//! The search, and what bounds it. `Limits` is the parsed `go` command.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{Colour, MAX_PLY, Move, MoveList, generate_legal, generate_noisy, to_uci};

use crate::corrhist::CorrectionHistory;
use crate::eval;
use crate::history::{self, History};
use crate::picker;
use crate::position::Position;
use crate::score::{self, DRAW, INFINITE, Score, mated_in};
use crate::see;
use crate::time::{self, Budget};
use crate::tt::{Bound, Table};
use crate::tune::Tunables;

mod counters;
mod depth;
mod limits;
mod pruning;
mod pv;

pub use depth::{
    REDUCTION_INDEX, extension, history_reduction, lmr_reduction, null_reduction, reduction,
};
pub use limits::Limits;
pub(crate) use pruning::{LMP_MULTIPLIER, REVERSE_FUTILITY_MARGIN};
pub use pruning::{
    futile_node, futility_margin, futility_skips, has_non_pawn_material, improving, lmp_count,
    lmp_index, lmp_skips, reverse_futile, reverse_futility_margin,
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
            evasion_lists: 0,
            evasion_lists_reordered: 0,
            iterations: Vec::new(),
            roots: Vec::new(),
            multipv: 1,
            lines: Vec::new(),
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
        self.evasion_lists = 0;
        self.evasion_lists_reordered = 0;
        self.iterations.clear();
        self.roots.clear();
        self.lines.clear();
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

    /// The best move in `board`, or `Move::NULL` when there is none. Returns when the limits
    /// are met or `stop` is raised; under `infinite` and under a ponder nobody has hit, only
    /// when `stop` is raised.
    pub fn run(&mut self, board: &mut Position, out: &mut dyn Write) -> Move {
        // Here and not in `begin`, which is per worker rather than per search. A group of
        // workers advances the generation once between them, so the bump belongs to whoever
        // starts the group.
        self.tt.new_search();
        self.run_in_current_generation(board, out)
    }

    /// The same run for a worker whose caller has already advanced the generation once for the
    /// whole group. Everything else that separates a worker from a lone search arrived through
    /// `set_parallel`.
    pub(crate) fn run_in_current_generation(
        &mut self,
        board: &mut Position,
        out: &mut dyn Write,
    ) -> Move {
        self.begin(board);

        let legal = generate_legal(board);
        if legal.is_empty() {
            self.score = if board.in_check() { mated_in(0) } else { DRAW };
            self.wait_if_open_ended();
            self.publish_nodes();
            return Move::NULL;
        }
        let mut root_moves: Vec<Move> = legal.iter().collect();
        // The list is never empty here, so the remainder is safe; a helper starting on a
        // different root move is the whole of the group's explicit divergence.
        let root_count = root_moves.len();
        root_moves.rotate_left(self.worker_index % root_count);

        let max_depth = self.limits.depth.unwrap_or(u32::MAX).clamp(1, MAX_DEPTH);
        for depth in 1..=max_depth {
            self.root_depth = depth;
            // Per iteration, so that the figure printed beside a depth belongs to it rather
            // than to the deepest line of any iteration before it.
            self.seldepth = 0;
            // One search of the root per line asked for, each skipping the moves the lines
            // before it took. A root with fewer moves than this reports the moves it has.
            let wanted = self.multipv.min(root_moves.len());
            self.lines.clear();
            let mut partial = (root_moves[0], -INFINITE);
            for _ in 0..wanted {
                let (best, score) = self.search_root(board, &legal, &root_moves, depth, out);
                if self.aborted {
                    partial = (best, score);
                    break;
                }
                self.keep_line(best, score);
            }
            if self.aborted {
                // The last completed iteration stands. If there is none, the best root move
                // fully searched so far does -- the first root move, if not even one was -- so
                // that there is always a move.
                if self.completed_depth == 0 {
                    // A line that finished outranks the one the abort cut short, and at
                    // `MultiPV` 1 there is never one to prefer.
                    let (best, score) = match self.lines.first() {
                        Some(line) => (line.mv, line.score),
                        None => partial,
                    };
                    self.best = best;
                    self.score = if score == -INFINITE { DRAW } else { score };
                    self.pv.clear();
                    self.pv.push(best);
                }
                break;
            }
            self.completed_depth = depth;
            // Descending, and the sort is stable, so lines that scored
            // equally stay in the order the root found them.
            self.lines.sort_by_key(|line| std::cmp::Reverse(line.score));
            let (best, score) = (self.lines[0].mv, self.lines[0].score);
            self.best = best;
            self.score = score;
            // The iteration is accepted, so the pair it ended on joins the ones before it. An
            // aborted iteration breaks out above and leaves nothing, which is what makes a run
            // of equal moves here a run of completed ones.
            self.roots.push((best, score));
            self.pv.clear();
            self.pv.extend_from_slice(&self.lines[0].pv);
            for number in 1..=self.lines.len() {
                self.report(board, number, out);
            }
            // The best move first next time: what makes an aborted iteration's fallback -- the
            // previous iteration -- a good one, and the only ordering there is.
            if let Some(i) = root_moves.iter().position(|&m| m == best) {
                root_moves[..=i].rotate_right(1);
            }
            // A hit may have arrived while this iteration ran, and the ladder below is read
            // against the origin it moves. Absorbed before the budget is consulted, so the
            // first clocked decision of the search is made on the new clock.
            self.absorb_ponder_hit();
            if let Some(b) = self.budget {
                let elapsed = self.elapsed_ms();
                self.iterations.push(elapsed);
                // Two reasons not to start another, and they are different reasons: the soft
                // budget says this move has had its share, and the prediction says the next
                // iteration would be abandoned unfinished at the hard budget and buy nothing.
                if elapsed >= b.soft || !time::another_iteration_fits(&self.iterations, b) {
                    break;
                }
            }
        }
        self.wait_if_open_ended();
        self.publish_nodes();
        self.best
    }

    /// Count a node, at the ply it sits at, and publish the count where a group is watching.
    /// The deepest ply is what `seldepth` reports and nothing here reads it, so a search that
    /// keeps it visits the same nodes in the same order as one that does not.
    #[inline]
    fn visit(&mut self, ply: usize) {
        self.nodes += 1;
        self.seldepth = self.seldepth.max(ply);
        if self.nodes & (NODE_PUBLISH_INTERVAL - 1) == 0 {
            self.publish_nodes();
        }
    }

    /// The root: every move a line before this one has not taken, the first in the full window
    /// and the rest in a null one, returning the best of them and its score. `MultiPV` 1 leaves
    /// nothing to skip, so the loop runs the whole list as it always has.
    fn search_root(
        &mut self,
        board: &mut Position,
        legal: &MoveList,
        moves: &[Move],
        depth: u32,
        out: &mut dyn Write,
    ) -> (Move, Score) {
        self.visit(0);
        self.table.clear(0);
        let mut alpha = -INFINITE;
        let beta = INFINITE;
        let mut best = Move::NULL;
        let mut best_score = -INFINITE;
        // The moves this root has searched, which is what the null window is conditioned on.
        // It is the index in the list only when no earlier line took anything.
        let mut searched = 0;
        for (i, &m) in moves.iter().enumerate() {
            if self.lines.iter().any(|line| line.mv == m) {
                continue;
            }
            if searched == 0 {
                best = m;
            }
            self.name_current(m, i + 1, legal, out);
            board.make_move(m);
            // A root move that gives check is extended like any other: the rule is about the
            // move, and the root has no claim on being the exception.
            let ext = extension(board.in_check(), 1, depth);
            let child = depth - 1 + ext;
            // The same rule as the interior nodes, and the root is where it is most nearly
            // free: the move in hand is the one the last iteration chose, and a root move that
            // beats it is what an iteration is looking for rather than what it expects. `beta`
            // here is `INFINITE`, so the second condition on the re-search is true whenever the
            // first is; it is written out because the rule is one rule.
            let mut score = if searched == 0 {
                -self.negamax(board, child, 1, -beta, -alpha)
            } else {
                -self.negamax(board, child, 1, -alpha - 1, -alpha)
            };
            if searched > 0 && !self.aborted && score > alpha && score < beta {
                score = -self.negamax(board, child, 1, -beta, -alpha);
            }
            board.unmake_move(m);
            searched += 1;
            if self.aborted {
                break;
            }
            if score > best_score {
                best_score = score;
                best = m;
                if score > alpha {
                    alpha = score;
                    self.table.update(0, m);
                }
            }
        }
        (best, best_score)
    }

    /// One interior node of the main search, at the `ply` and `depth` given, searched with the
    /// full window.
    #[must_use]
    pub fn node(&mut self, board: &mut Position, depth: u32, ply: usize) -> Score {
        self.node_window(board, depth, ply, -INFINITE, INFINITE)
    }

    /// The same node, searched with the window given instead of the full one. The seam a null
    /// window is reached through.
    #[must_use]
    pub fn node_window(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Score {
        // As though `depth` were the iteration's, so that the extension's ply cap means here
        // what it means in a search rather than reading whatever the last iteration left
        // behind.
        self.root_depth = depth;
        self.negamax(board, depth, ply, alpha, beta)
    }

    /// Negamax with alpha-beta, fail-soft. The value returned after an abort is meaningless and
    /// is discarded by every caller.
    fn negamax(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        mut alpha: Score,
        beta: Score,
    ) -> Score {
        // The horizon: the quiescence search takes over, and counts the node.
        if depth == 0 {
            return self.quiesce(board, ply, alpha, beta);
        }
        self.visit(ply);
        self.table.clear(ply);
        // At every node, so that `nodes N` stops at N and not at N plus a subtree.
        if self.out_of_time() {
            return DRAW;
        }

        // A repeated position is a draw wherever it is met: twofold inside the tree, threefold
        // against the game history.
        if board.is_repetition() {
            return DRAW;
        }

        // The ply bound, above the sort because that is where the first read past the arrays
        // would be. In release a read past their end is a bounds check rather than the
        // assertion a debug build gets.
        if ply >= MAX_PLY {
            return eval::evaluate(board);
        }

        // The table, before the moves are generated: that saving is most of what it is for.
        let key = board.key();
        let (tt_move, cutoff) = self.probe(board, key, depth, ply, alpha, beta);
        if let Some(score) = cutoff {
            return score;
        }

        // The static evaluation, written whether or not anything below reads it at this node:
        // the stack is how a rule at ply `p` compares its own reading against the one at `p -
        // 2`, so a hole here is a wrong answer there, not a saving. A node in check writes
        // `None` rather than a number, because what the evaluation measures is a position
        // nobody is about to win material in, and a check is exactly that claim being
        // contested.
        let in_check = board.in_check();
        let pawn_key = board.pawn_key();
        let us_eval = board.side_to_move();
        self.evals[ply] = self.corrected_eval(board, in_check, pawn_key, us_eval);

        // The margin, above the null move because the node's preamble runs the margin tests
        // there and because below it the rule would be nothing but its own error case: see
        // [`Search::reverse_futility`].
        if let Some(bound) = self.reverse_futility(board, depth, ply, alpha, beta) {
            return bound;
        }

        if let Some(score) = self.null_move(board, depth, ply, alpha, beta) {
            return score;
        }

        let mut legal = generate_legal(board);
        if legal.is_empty() {
            return if in_check { mated_in(ply) } else { DRAW };
        }
        // After the mate check: a mate delivered on the hundredth half-move is a mate, not a
        // draw.
        if board.halfmove_clock() >= 100 {
            return DRAW;
        }
        // The history row is the side to move's, read here and again per move below, so `us` is
        // taken once at the node rather than after a move has changed it.
        let us = board.side_to_move();
        let killers = self.order(board, &mut legal, tt_move, us, ply);

        // The margin, read once with the node's own alpha. The interior node's preamble runs
        // the margin tests beside the static evaluation and above the null move, and this sits
        // below it instead: nothing between the two writes what the test reads, and this rule
        // returns no score of its own, so the sequence is unaffected and what the placement
        // saves is the test at every node the null move cuts.
        let futile = futile_node(self.evals[ply], depth, alpha);
        self.futility_nodes += u64::from(futile);

        // The count, read once against the list this node actually holds, because the rule is
        // off at a node the count already admits whole. It sits beside the margin and not above
        // it: the two act on one population and overlap over 43% of it, and whichever is asked
        // first keeps the moves it takes.
        let give_up = lmp_index(&self.tunables, in_check, depth, legal.len());
        self.lmp_nodes += u64::from(give_up.is_some());

        let original_alpha = alpha;
        let mut best = -INFINITE;
        let mut best_move = Move::NULL;
        for (i, m) in legal.iter().enumerate() {
            if self.futile(board, futile, m, i) {
                continue;
            }
            // Before the move is made, which is the whole of what the rule buys: a move given
            // up here costs the node its exemption tests and nothing else. It is also above the
            // reduction rather than beside it, and the two are not alternatives at a node where
            // both would fire -- the move is gone and [`reduction`] never sees it.
            if self.given_up(board, give_up, m, killers, i) {
                continue;
            }
            board.make_move(m);
            // The check extension, on the child's own state: `make_move` has just recomputed
            // the checkers, so asking costs nothing that was not already spent. The same read
            // is the reduction's check exemption below, so it is taken once and named.
            let gives_check = board.in_check();
            let ext = extension(gives_check, ply + 1, self.root_depth);
            let child = depth - 1 + ext;
            // The first move gets the window this node was given and every move behind it the
            // narrower question. The module doc has the window, what it buys and what it costs.
            let mut score = if i == 0 {
                -self.negamax(board, child, ply + 1, -beta, -alpha)
            } else {
                // The index decides whether this move is reduced at all and the score by how
                // much; [`history_reduction`] has why those are not the same reading.
                let base = reduction(in_check, gives_check, m, killers, depth, i);
                self.late_move(board, child, base, self.history.get(us, m), ply, alpha)
            };
            // No re-search at or above beta, which is already the bound this node returns, and
            // none at a node whose own window is a null one, where there is no room for a score
            // to ask for one.
            if i > 0 && !self.aborted && score > alpha && score < beta {
                score = -self.negamax(board, child, ply + 1, -beta, -alpha);
            }
            board.unmake_move(m);
            if self.aborted {
                return DRAW;
            }
            if score > best {
                best = score;
                best_move = m;
                if score > alpha {
                    alpha = score;
                    self.table.update(ply, m);
                    if alpha >= beta {
                        remember_killer(&mut self.killers[ply], m);
                        // The moves it beat are the ones before it in the sorted list, and the
                        // margin and the count above both skip moves inside it, so a quiet
                        // cutoff debits moves that failed to cut and moves that were never
                        // searched alike. Measured over the bench positions at 0.4.8 that is
                        // 29.2% of the debits, and separating the two changes what the table
                        // holds, which is a change with its own test.
                        self.remember_history(us, &legal.as_slice()[..i], m, depth);
                        break;
                    }
                }
            }
        }
        // Fail-soft, so the bound follows the value and not the window it was found in. Nothing
        // an aborted search computed is stored: the loop above returns before this line.
        let bound = if best >= beta {
            Bound::Lower
        } else if best > original_alpha {
            Bound::Exact
        } else {
            Bound::Upper
        };
        self.tt.store(
            key,
            best_move,
            score::to_tt(best, ply),
            depth.min(u32::from(u8::MAX)) as u8,
            bound,
        );
        self.remember_correction(pawn_key, us_eval, ply, best, best_move, bound, depth);
        best
    }

    /// The static evaluation this node's rules read, corrected by what the evaluation has been
    /// wrong by on this pawn structure. The correction is read before the node folds anything
    /// in, so nothing it offers has seen the score it will be scored against.
    fn corrected_eval(
        &mut self,
        board: &Board,
        in_check: bool,
        pawn_key: u64,
        side: Colour,
    ) -> Option<Score> {
        if in_check {
            return None;
        }
        let correction = self.corrhist.correction(pawn_key, side);
        self.corrhist_applied += u64::from(correction != 0);
        Some(eval::evaluate(board) + correction)
    }

    /// Fold this node's disagreement between the static evaluation and the score it returned
    /// into the correction table. Four things disqualify a node: no static reading, a mate
    /// score, a best move that is noisy or absent, and a bound pointing the other way from the
    /// difference.
    #[allow(clippy::too_many_arguments)]
    fn remember_correction(
        &mut self,
        pawn_key: u64,
        side: Colour,
        ply: usize,
        best: Score,
        best_move: Move,
        bound: Bound,
        depth: u32,
    ) {
        let Some(eval) = self.evals[ply] else {
            return;
        };
        if score::is_mate(best) || best_move == Move::NULL || best_move.is_noisy() {
            return;
        }
        let usable = match bound {
            Bound::Exact => true,
            Bound::Lower => best > eval,
            Bound::Upper => best < eval,
        };
        if usable {
            self.corrhist.update(pawn_key, side, best - eval, depth);
            self.corrhist_updates += 1;
        }
    }

    /// Put the node's move list in the order it will be searched, and hand back the killers the
    /// caller needs again below. Three stages and one sort.
    fn order(
        &self,
        board: &Board,
        legal: &mut MoveList,
        tt_move: Move,
        us: Colour,
        ply: usize,
    ) -> [Move; 2] {
        let ordered = usize::from(order_first(legal, tt_move));
        let killers = self.killers[ply];
        picker::sort_from(board, legal, ordered, killers, self.history.side(us));
        killers
    }

    /// The transposition table at an interior node: the move a hit named, and the score to
    /// return where the stored bound answers this node's question outright. The move comes back
    /// whatever the depth says, which is why the two halves come back separately.
    fn probe(
        &self,
        board: &Board,
        key: u64,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> (Move, Option<Score>) {
        let Some(hit) = self.tt.probe(key) else {
            return (Move::NULL, None);
        };
        if u32::from(hit.depth) < depth || board.halfmove_clock() >= 100 {
            return (hit.mv, None);
        }
        let score = score::from_tt(hit.score, ply);
        let cutoff = match hit.bound {
            Bound::Exact => true,
            Bound::Lower => score >= beta,
            Bound::Upper => score <= alpha,
        };
        (hit.mv, cutoff.then_some(score))
    }

    /// The null-window search of one move behind a node's first, reduced by as many plies as
    /// [`reduction`] and [`history_reduction`] allow. A reduced search that beats alpha is
    /// re-run at the full child depth before its answer is believed.
    fn late_move(
        &mut self,
        board: &mut Position,
        child: u32,
        base: u32,
        history: i32,
        ply: usize,
        alpha: Score,
    ) -> Score {
        let r = history_reduction(base, history);
        self.history_reduced_less += u64::from(r < base);
        self.history_reduced_more += u64::from(r > base);
        if r == 0 {
            return -self.negamax(board, child, ply + 1, -alpha - 1, -alpha);
        }
        self.lmr_reductions += 1;
        let reduced = child.saturating_sub(r).max(1);
        let score = -self.negamax(board, reduced, ply + 1, -alpha - 1, -alpha);
        if score > alpha && !self.aborted {
            self.lmr_researches += 1;
            return -self.negamax(board, child, ply + 1, -alpha - 1, -alpha);
        }
        score
    }

    /// Whether the move at `index` of a node the margin admitted is skipped without being
    /// searched. `gives_check` is asked last, because it is the only expensive question here
    /// and only a move that would otherwise be skipped has to answer it.
    fn futile(&mut self, board: &Board, futile: bool, m: Move, index: usize) -> bool {
        if !futility_skips(futile, m, index) {
            return false;
        }
        if board.gives_check(m) {
            self.futility_kept_check += 1;
            return false;
        }
        self.futility_skipped += 1;
        true
    }

    /// Whether the move at `index` of a node [`lmp_index`] admitted is given up without being
    /// searched. `gives_check` is asked last, for [`Search::futile`]'s reason, and it is the
    /// whole cost of the rule at a move it does give up.
    fn given_up(
        &mut self,
        board: &Board,
        from: Option<usize>,
        m: Move,
        killers: [Move; 2],
        index: usize,
    ) -> bool {
        if !lmp_skips(from, m, killers, index) {
            return false;
        }
        if board.gives_check(m) {
            self.lmp_kept_check += 1;
            return false;
        }
        self.lmp_skipped += 1;
        true
    }

    /// Record what this node's cutoff says about its quiet moves: credit `cut`, and debit every
    /// quiet move tried ahead of it at this node.
    fn remember_history(&mut self, us: Colour, tried: &[Move], cut: Move, depth: u32) {
        if cut.is_noisy() {
            return;
        }
        let bonus = history::bonus(depth);
        self.history.update(us, cut, bonus);
        for &beaten in tried.iter().filter(|q| !q.is_noisy()) {
            self.history.update(us, beaten, -bonus);
        }
    }

    /// Reverse futility at one node: where the static evaluation stands
    /// [`reverse_futility_margin`] above beta, the node is returned at the bound its own
    /// arithmetic established, without generating a move. `Some` is that bound; `None` means
    /// search the node.
    fn reverse_futility(
        &mut self,
        board: &Board,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Option<Score> {
        let bound = reverse_futile(&self.tunables, self.evals[ply], depth, beta)?;
        if board.halfmove_clock() >= 100 {
            return None;
        }
        if beta != alpha + 1 {
            self.reverse_futility_refused_window += 1;
            return None;
        }
        self.reverse_futility_cutoffs += 1;
        Some(bound)
    }

    /// Null-move pruning at one node: `Some` is the cutoff, `None` means search the node.
    /// Refused in check, at a full window, on a mate-scale beta, below beta, at a position the
    /// null move itself reached, on a halfmove clock at the limit, and where the side to move
    /// has nothing but pawns beside the king ([`has_non_pawn_material`]).
    fn null_move(
        &mut self,
        board: &mut Position,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Option<Score> {
        let admitted = self.evals[ply].is_some_and(|eval| eval >= beta)
            && beta == alpha + 1
            && !score::is_mate(beta)
            && board.plies_from_null() != 0
            && board.halfmove_clock() < 100;
        if !admitted {
            return None;
        }
        if !has_non_pawn_material(board) {
            self.null_refused_material += 1;
            return None;
        }
        self.null_attempts += 1;
        let reduced = depth.saturating_sub(1 + null_reduction(depth));
        let _ = board.make_null_move();
        let score = -self.negamax(board, reduced, ply + 1, -beta, -beta + 1);
        board.unmake_null_move();
        if self.aborted {
            return Some(DRAW);
        }
        if score >= beta {
            self.null_cutoffs += 1;
            return Some(if score::is_mate(score) { beta } else { score });
        }
        None
    }

    /// The quiescence search. Out of check the side to move may stand pat, then every noisy
    /// move is tried most valuable victim first, except those whose static exchange loses
    /// material.
    fn quiesce(
        &mut self,
        board: &mut Position,
        ply: usize,
        mut alpha: Score,
        beta: Score,
    ) -> Score {
        self.visit(ply);
        self.table.clear(ply);
        if self.out_of_time() {
            return DRAW;
        }
        if board.is_repetition() {
            return DRAW;
        }
        if ply >= MAX_PLY {
            return eval::evaluate(board);
        }

        let in_check = board.in_check();
        let (moves, mut best) = if in_check {
            let mut evasions = generate_legal(board);
            if evasions.is_empty() {
                return mated_in(ply);
            }
            // After the mate check: a mate on the hundredth half-move is a mate.
            if board.halfmove_clock() >= 100 {
                return DRAW;
            }
            // Noisy evasions first, by victim; the quiet ones keep the order the generator
            // emitted them in, behind all of those. No killers and no history: whether either
            // ranks the quiet evasions usefully is unmeasured, and a second change.
            let generated_first = evasions.as_slice()[0];
            picker::sort_from(board, &mut evasions, 0, [Move::NULL; 2], &[]);
            // The head is the move a cutoff here is bought with, so it is the element the
            // counter reads. Both are written on no decision path.
            self.evasion_lists += 1;
            if evasions.as_slice()[0] != generated_first {
                self.evasion_lists_reordered += 1;
            }
            (evasions, -INFINITE)
        } else {
            if board.halfmove_clock() >= 100 {
                return DRAW;
            }
            let stand_pat = eval::evaluate(board);
            if stand_pat >= beta {
                return stand_pat;
            }
            if stand_pat > alpha {
                alpha = stand_pat;
            }
            let mut noisy = generate_noisy(board);
            picker::sort_noisy(board, &mut noisy);
            (noisy, stand_pat)
        };

        for m in moves.iter() {
            // A losing capture is refused, out of check only: in check the list is the legal
            // list and every entry answers the check.
            if !in_check && see::see(board, m) < 0 {
                continue;
            }
            board.make_move(m);
            let score = -self.quiesce(board, ply + 1, -beta, -alpha);
            board.unmake_move(m);
            if self.aborted {
                return DRAW;
            }
            if score > best {
                best = score;
                if score > alpha {
                    alpha = score;
                    if alpha >= beta {
                        break;
                    }
                }
            }
        }
        best
    }

    /// Whether a limit or the stop flag ends the search here, at any node from the first; the
    /// clock only every `CLOCK_INTERVAL` nodes, and only when there is a budget.
    fn out_of_time(&mut self) -> bool {
        if self.aborted {
            return true;
        }
        if self.stop.load(Ordering::Relaxed) {
            self.aborted = true;
            return true;
        }
        if self.limits.infinite {
            return false;
        }
        if let Some(n) = self.limits.nodes
            && self.reported_nodes() >= n
        {
            self.aborted = true;
            return true;
        }
        if self.nodes & (CLOCK_INTERVAL - 1) == 0 {
            // On the interval that already exists rather than on one of its own: a hit that
            // waited for the end of a deep iteration would spend the clock it just took.
            self.absorb_ponder_hit();
            if let Some(b) = self.budget
                && self.elapsed_ms() >= b.hard
            {
                self.aborted = true;
                return true;
            }
        }
        false
    }

    /// Take a `ponderhit` if one has arrived: the search stops pondering, the clock runs from
    /// this moment, and the iteration ladder starts again. The ladder is cleared because an
    /// entry measured from the ponder's origin, read against the new one, gives a branching
    /// factor below one and starts an iteration on it.
    fn absorb_ponder_hit(&mut self) {
        if !self.pondering || !self.ponder_hit.is_some_and(|f| f.load(Ordering::Relaxed)) {
            return;
        }
        self.pondering = false;
        self.start = Instant::now();
        self.iterations.clear();
        self.budget = self.budget_on_hit;
    }

    /// Hold the finished search until `stop`, in the two states that say so. `go infinite` and
    /// a ponder nobody has hit both mean "do not answer until told", and a ponder that returned
    /// early would be answering a question the opponent has not yet asked.
    fn wait_if_open_ended(&self) {
        if self.limits.infinite || self.pondering {
            while !self.stop_requested() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Name the root move about to be searched, and its place in the root list, once the search
    /// has been running for [`CURRMOVE_AFTER_MS`]. Nothing is written and no clock is read under
    /// a depth or node limit, which is the shape `bench` runs in.
    fn name_current(&self, m: Move, number: usize, legal: &MoveList, out: &mut dyn Write) {
        if self.budget.is_none() && !self.limits.infinite {
            return;
        }
        if self.elapsed_ms() < CURRMOVE_AFTER_MS {
            return;
        }
        // No `depth` on this line, deliberately. A harness that picks iteration lines out of the
        // stream by their `info depth ` prefix would otherwise collect one of these per root
        // move.
        let _ = writeln!(
            out,
            "info currmove {} currmovenumber {number}",
            to_uci(m, legal, self.chess960)
        );
        let _ = out.flush();
    }

    /// Keep the line the root just returned, with the pv it ended on. Taken off [`PvTable`]
    /// here because the next line's search of the root clears row zero and writes its own.
    fn keep_line(&mut self, mv: Move, score: Score) {
        let pv = self.table.line(0).to_vec();
        self.lines.push(RootLine { mv, score, pv });
    }

    /// This search's nodes, or the group's where one is watching. A worker reads its own count
    /// live and its siblings' from the slots they publish into, so a reported figure is about
    /// the whole search rather than about one thread.
    #[inline]
    fn reported_nodes(&self) -> u64 {
        let Some((nodes, worker_index)) = self.shared_nodes else {
            return self.nodes;
        };
        nodes.iter().enumerate().fold(0, |total, (index, nodes)| {
            total.saturating_add(if index == worker_index {
                self.nodes
            } else {
                nodes.load(Ordering::Relaxed)
            })
        })
    }

    /// Store this worker's count in the slot it owns. Nothing else writes that slot, so no
    /// ordering beyond `Relaxed` is needed; the slots do share cache lines, which was measured
    /// at under 1 percent of node throughput up to 18 threads and about 3 percent at 64.
    #[inline]
    fn publish_nodes(&self) {
        if let Some((nodes, worker_index)) = self.shared_nodes {
            nodes[worker_index].store(self.nodes, Ordering::Relaxed);
        }
    }

    /// One `info` line for line `number` of the iteration just completed, its pv spelled by
    /// walking it on the board so castling reads per the option. `multipv` is absent where only
    /// one line was asked for, which is the line every rating list and every test reads.
    fn report(&self, board: &mut Position, number: usize, out: &mut dyn Write) {
        let reported = &self.lines[number - 1];
        let ms = self.elapsed_ms();
        let nps = self.reported_nodes() * 1000 / ms.max(1);
        let numbered = if self.multipv > 1 {
            format!(" multipv {number}")
        } else {
            String::new()
        };
        let mut line = format!(
            "info depth {} seldepth {}{numbered} score {} nodes {} nps {nps} hashfull {} time {ms} pv",
            self.completed_depth,
            self.seldepth,
            score::uci(reported.score),
            self.reported_nodes(),
            self.tt.hashfull()
        );
        let mut made = 0;
        for &m in &reported.pv {
            let legal = generate_legal(board);
            if !legal.contains(m) {
                break;
            }
            line.push(' ');
            line.push_str(&to_uci(m, &legal, self.chess960));
            board.make_move(m);
            made += 1;
        }
        for &m in reported.pv[..made].iter().rev() {
            board.unmake_move(m);
        }
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use super::Search;
    use crate::tt::Table;

    /// `reported_nodes` answers for the group and not for the worker that asks. This is the one
    /// externally visible thing `Threads` above one changes, and it is asserted here rather than
    /// through a search because a search only shows it when the helpers get scheduled.
    #[test]
    fn reported_nodes_sums_the_group_and_reads_its_own_count_live() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let slots: Vec<AtomicU64> = (0..4).map(|_| AtomicU64::new(0)).collect();
        slots[1].store(100, Ordering::Relaxed);
        slots[2].store(20, Ordering::Relaxed);
        slots[3].store(3, Ordering::Relaxed);

        let mut search = Search::new(&stop, &tt);
        search.nodes = 7;
        assert_eq!(
            search.reported_nodes(),
            7,
            "a search outside a group answers for itself"
        );

        search.set_parallel(0, &slots);
        assert_eq!(
            search.reported_nodes(),
            130,
            "own count live plus every sibling's slot"
        );

        // The worker's own slot is stale until it publishes, which is why the live count is read
        // from the field and never from the slot.
        assert_eq!(slots[0].load(Ordering::Relaxed), 0);
        search.publish_nodes();
        assert_eq!(slots[0].load(Ordering::Relaxed), 7);
        assert_eq!(search.reported_nodes(), 130, "publishing changes no total");
    }
}
