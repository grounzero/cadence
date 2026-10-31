// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::Instant;

use cadence_core::position::Board;
use cadence_core::{MAX_MOVES, MAX_PLY, Move, MoveList};

use crate::conthist::{ContinuationHistory, Frame};
use crate::corrhist::CorrectionHistory;
use crate::history::History;
use crate::picker::Slot;
use crate::score::{ABORTED, Score};
use crate::time::{self, Budget};
use crate::tt::{Bound, Table};
use crate::tune::Tunables;

mod counters;
mod depth;
mod expected;
mod limits;
mod node;
mod pruning;
mod pv;
mod quiesce;
mod report;
mod root;

pub use depth::{
    LMR_CAP, REDUCTION_INDEX, cut_node_reduction, extension, history_reduction, iir_reduction,
    lmr_reduction, losing_capture, move_history, null_reduction, pv_reduction, reduction,
    reduction_cap,
};
#[cfg(debug_assertions)]
pub use expected::Call;
pub use expected::{Expected, Via};
pub use limits::Limits;
pub(crate) use pruning::{LMP_MULTIPLIER, REVERSE_FUTILITY_MARGIN};
pub use pruning::{
    PROBCUT_REDUCTION, futile_node, futility_margin, futility_skips, has_non_pawn_material,
    improving, lmp_count, lmp_index, lmp_skips, probcut_bound, reverse_futile,
    reverse_futility_margin,
};
use pv::PvTable;

/// Nodes between clock reads; a power of two.
const CLOCK_INTERVAL: u64 = 1024;

/// A parallel `go nodes` may overshoot by up to this many nodes per other worker.
const NODE_PUBLISH_INTERVAL: u64 = 64;

/// The conventional second: faster moves never print `currmove`.
pub const CURRMOVE_AFTER_MS: u64 = 1000;

/// Not the deepest ply: extensions reach past it, so both searches stop at `MAX_PLY` and stand on
/// the evaluation.
const MAX_DEPTH: u32 = MAX_PLY as u32;
const _: () = assert!(MAX_DEPTH as usize == MAX_PLY);

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

/// The rest keep their order.
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

pub fn remember_killer(killers: &mut [Move; 2], m: Move) {
    if m.is_noisy() || killers[0] == m {
        return;
    }
    killers[1] = killers[0];
    killers[0] = m;
}

/// Per thread; nothing is global.
pub struct Search<'a> {
    limits: Limits,
    stop: &'a AtomicBool,
    /// A flag rather than new limits, because the search that must hear it is already running.
    ponder_hit: Option<&'a AtomicBool>,
    tt: &'a Table,
    /// Spelling in `info` lines only.
    chess960: bool,
    /// `bench` builds its own searches, so it never sees a setting.
    tunables: Tunables,
    /// Zero only where a gate compares against a search with no table, every node of which lacks a
    /// table move.
    iir_plies: u32,
    nodes: u64,
    /// `None` on the single-thread path, where `bench` runs and node counts are exact.
    shared_nodes: Option<(&'a [AtomicU64], usize)>,
    /// A helper rotates its root list by this much, the group's only explicit divergence: measured
    /// at 8 to 12 percent of time to depth at 4 to 16 threads.
    worker_index: usize,
    start: Instant,
    /// `None`: the clock is never read.
    budget: Option<Budget>,
    /// What the search is doing now; `limits.ponder` records what the `go` asked.
    pondering: bool,
    /// Derived up front: the hit is absorbed where no board is at hand.
    budget_on_hit: Option<Budget>,
    /// The iteration's partial result is discarded unless it is the first.
    aborted: bool,
    completed_depth: u32,
    best: Move,
    score: Score,
    /// Of the last completed iteration.
    pv: Vec<Move>,
    /// Read by nothing that decides anything.
    seldepth: usize,
    table: PvTable,
    killers: [[Move; 2]; MAX_PLY],
    /// The picker's buffer, one row per ply, allocated once so that no node clears one.
    slots: Box<[[Slot; MAX_MOVES]]>,
    /// On the heap: `Search` is returned by value.
    history: History,
    conthist: ContinuationHistory,
    /// The move made at each ply of the current path, which the continuation history reads back.
    frames: [Frame; MAX_PLY],
    corrhist: CorrectionHistory,
    /// Read on no decision path.
    corrhist_updates: u64,
    corrhist_applied: u64,
    /// The check extension's ply cap is a multiple of it.
    root_depth: u32,
    /// `None` in check: a position under attack has no quiet reading worth comparing.
    evals: [Option<Score>; MAX_PLY],
    /// Each node's ttPv flag, written once its probe has answered, so a child reads its parent's.
    tt_pv: [bool; MAX_PLY],
    /// Whether each node is on the previous iteration's line. One longer than the plies, since a
    /// move at the last ply writes its child's.
    follow_pv: [bool; MAX_PLY + 1],
    /// The move made at each ply of the current path, which each call's entry takes.
    #[cfg(debug_assertions)]
    moved: [Move; MAX_PLY],
    /// Every call into the main search since a gate asked for the record, in a debug build only.
    #[cfg(debug_assertions)]
    calls: Option<Vec<Call>>,
    /// How the next call's kind was derived, which that call's entry takes.
    #[cfg(debug_assertions)]
    asked: Option<(Expected, Via)>,
    /// These counters are read on no decision path, so a depth-limited search stays a function of
    /// the code.
    null_attempts: u64,
    null_cutoffs: u64,
    /// Lets the zugzwang gate see the refusal decide, not merely no null move.
    null_refused_material: u64,
    lmr_reductions: u64,
    lmr_researches: u64,
    lmr_pv_lessened: u64,
    lmr_cut_reduced_more: u64,
    lmr_capped: u64,
    lmr_losing_captures: u64,
    /// Two, so a gate can tell a table that only credits from one that also debits.
    history_reduced_less: u64,
    history_reduced_more: u64,
    history_debits: u64,
    /// As the butterfly's, and the reads that found no frame, so a gate sees the root and the null
    /// move skipped.
    continuation_credits: u64,
    continuation_debits: u64,
    continuation_absent: u64,
    /// The check-exemption count lets a gate see the exemption decide.
    futility_nodes: u64,
    futility_skipped: u64,
    futility_kept_check: u64,
    /// What this rule adds over the margin, which is asked first.
    lmp_nodes: u64,
    lmp_skipped: u64,
    lmp_kept_check: u64,
    /// The refusal count lets a gate see the full-window refusal decide.
    reverse_futility_cutoffs: u64,
    reverse_futility_refused_window: u64,
    probcut_attempts: u64,
    probcut_searches: u64,
    probcut_cutoffs: u64,
    probcut_refused_window: u64,
    iir_nodes: u64,
    /// The first shows the in-check horizon is reached, without which the second proves nothing.
    evasion_lists: u64,
    evasion_lists_reordered: u64,
    /// Empty without a budget, and taken from the soft-budget reading, so it adds no clock read.
    iterations: Vec<u64>,
    /// Kept under every limit: it costs no clock read and decides nothing.
    roots: Vec<(Move, Score)>,
    multipv: usize,
    /// `go searchmoves`: the root searches only these, and every move where it is empty.
    root_filter: Vec<Move>,
    /// Best first once the iteration is accepted.
    lines: Vec<RootLine>,
    /// What a level samples among; separate from `lines`, which an aborted iteration leaves
    /// partial.
    accepted: Vec<RootLine>,
}

/// Owns its pv: a second search of the root overwrites the table's.
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
            iir_plies: depth::IIR_REDUCTION,
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
            score: ABORTED,
            pv: Vec::with_capacity(MAX_PLY),
            seldepth: 0,
            table: PvTable::new(),
            killers: [[Move::NULL; 2]; MAX_PLY],
            slots: vec![[Slot::default(); MAX_MOVES]; MAX_PLY].into_boxed_slice(),
            history: History::new(),
            conthist: ContinuationHistory::new(),
            frames: [None; MAX_PLY],
            corrhist: CorrectionHistory::new(),
            corrhist_updates: 0,
            corrhist_applied: 0,
            root_depth: 0,
            evals: [None; MAX_PLY],
            tt_pv: [false; MAX_PLY],
            follow_pv: [false; MAX_PLY + 1],
            #[cfg(debug_assertions)]
            moved: [Move::NULL; MAX_PLY],
            #[cfg(debug_assertions)]
            calls: None,
            #[cfg(debug_assertions)]
            asked: None,
            null_attempts: 0,
            null_cutoffs: 0,
            null_refused_material: 0,
            lmr_reductions: 0,
            lmr_researches: 0,
            lmr_pv_lessened: 0,
            lmr_cut_reduced_more: 0,
            lmr_capped: 0,
            lmr_losing_captures: 0,
            history_reduced_less: 0,
            history_reduced_more: 0,
            history_debits: 0,
            continuation_credits: 0,
            continuation_debits: 0,
            continuation_absent: 0,
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
            iir_nodes: 0,
            evasion_lists: 0,
            evasion_lists_reordered: 0,
            iterations: Vec::new(),
            roots: Vec::new(),
            multipv: 1,
            root_filter: Vec::new(),
            lines: Vec::new(),
            accepted: Vec::new(),
        }
    }

    pub fn set_limits(&mut self, limits: Limits) {
        self.limits = limits;
    }

    /// Without one, a `go ponder` still waits for `stop`.
    pub fn set_ponder_hit(&mut self, flag: &'a AtomicBool) {
        self.ponder_hit = Some(flag);
    }

    pub fn set_chess960(&mut self, on: bool) {
        self.chess960 = on;
    }

    /// From the next run on.
    pub fn set_tunables(&mut self, tunables: Tunables) {
        self.tunables = tunables;
    }

    pub fn set_multipv(&mut self, n: usize) {
        self.multipv = n.max(1);
    }

    /// For the gates alone: no UCI option and no `bench` path reaches it.
    pub fn set_iir_reduction(&mut self, plies: u32) {
        self.iir_plies = plies;
    }

    /// For the gates alone, in a debug build: every call into the main search from here on is
    /// recorded.
    #[cfg(debug_assertions)]
    pub fn trace_calls(&mut self) {
        self.calls = Some(Vec::new());
    }

    /// In the order the calls were made.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn calls(&self) -> &[Call] {
        self.calls.as_deref().unwrap_or_default()
    }

    /// From the next run on; a move not legal at the root is never searched.
    pub fn set_root_filter(&mut self, moves: &[Move]) {
        self.root_filter = moves.to_vec();
    }

    /// The clock origin is set here and moved only by a `ponderhit`.
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
        self.conthist.clear();
        self.frames = [None; MAX_PLY];
        self.evals = [None; MAX_PLY];
        self.null_attempts = 0;
        self.null_cutoffs = 0;
        self.null_refused_material = 0;
        self.lmr_reductions = 0;
        self.lmr_researches = 0;
        self.lmr_pv_lessened = 0;
        self.lmr_cut_reduced_more = 0;
        self.lmr_capped = 0;
        self.lmr_losing_captures = 0;
        self.history_reduced_less = 0;
        self.history_reduced_more = 0;
        self.history_debits = 0;
        self.continuation_credits = 0;
        self.continuation_debits = 0;
        self.continuation_absent = 0;
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
        self.iir_nodes = 0;
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
        self.budget_on_hit = if self.pondering {
            let mut clocked = self.limits;
            clocked.ponder = false;
            time::budget(&clocked, board.side_to_move())
        } else {
            None
        };
    }

    /// The table, the stop flag and the node slots are all a group shares.
    pub(crate) fn set_parallel(&mut self, worker_index: usize, nodes: &'a [AtomicU64]) {
        self.worker_index = worker_index;
        self.shared_nodes = Some((nodes, worker_index));
    }
}
