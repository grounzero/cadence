// SPDX-License-Identifier: GPL-3.0-or-later

//! The search, and what bounds it.
//!
//! `Limits` is the parsed `go` command. `Search` is one search: it holds the
//! per-thread state -- there are no globals -- and runs in whichever thread
//! calls it. The UCI layer gives it a board of its own and a stop flag, and
//! prints whatever it returns.
//!
//! **What the search is.** Negamax with alpha-beta, fail-soft, over the
//! legal move list, to a fixed depth; iterative deepening from depth one,
//! the previous iteration's best move searched first at the root; a
//! triangular table for the principal variation. At
//! the horizon, a quiescence search: the captures, en-passant captures and
//! promotions are resolved, and a side in check gets out of it, before the
//! static evaluation is consulted, so that what is evaluated is a position
//! nobody is about to win material in. Draws are scored as draws: a
//! repetition (twofold inside the tree, threefold against the game history
//! -- `Board::is_repetition`), the fifty-move rule, stalemate. Mate is
//! `MATE - ply`, so the shorter mate wins.
//!
//! **The transposition table** (`tt`) is probed at every interior node of
//! the main search, before the moves are generated, and stored at every
//! one the search completes. A hit searched at least as deep returns at
//! once when its bound allows: an exact score always, a lower bound at or
//! above beta, an upper bound at or below alpha. Mate scores are stored
//! relative to the node rather than to the root (`score::to_tt`), so an
//! entry read from another ply still names the right distance. The
//! quiescence search neither probes nor stores.
//!
//! **The ordering.** At an interior node, three stages and one sort. The
//! move a table hit named is tried first (`order_first`): a hit supplies a
//! move whatever its depth says, so the ordering fires far more often than
//! the cutoff above it does, and a move the position does not have is
//! refused rather than played. Behind it the rest of the list, in one pass
//! of `picker::sort_from`: the noisy moves whose exchange keeps material,
//! by most valuable victim; then this ply's two killers, the quiet moves
//! that caused a beta cutoff at a sibling of this node (`remember_killer`);
//! then the noisy moves whose exchange loses material, which keep their
//! victim order among themselves; then the quiet moves that have refuted
//! nothing, which all rank alike and so keep the order they were generated
//! in. A capture that gives material away is worth less than a quiet move
//! that has already caused a cutoff, and until the losing band existed
//! every capture was tried before every killer. At the root the previous
//! iteration's best move goes first, and that is unchanged. The quiescence
//! search orders its moves through the same sort and has no killers to
//! give it: out of check its list holds no quiet move and no losing one
//! either, because the rule below refuses those before they are searched,
//! so that list is sorted by victim alone (`picker::sort_noisy`); in check
//! it holds every evasion, the capture of the checking piece ahead of the
//! retreats the generator emits first and ahead of the captures that lose
//! material.
//!
//! **The one thing the quiescence search refuses.** Out of check, a noisy
//! move whose static exchange value is negative (`see::see`: the material
//! it loses once every recapture has been answered) is skipped without
//! being searched. The value is computed for each move as it comes up in
//! the sorted list, so a cutoff on the first capture costs one evaluation
//! and the moves behind it none. In check nothing is skipped: the list is
//! the legal list, every entry is an answer to the check, and skipping one
//! is skipping an answer.
//!
//! **What it is not, on purpose.** No history, so the quiet moves the
//! killers did not claim are not ranked against each other: that is a
//! separate change with its own SPRT, plugging into the same sort. No
//! extensions or reductions, and no other pruning, in the quiescence
//! search included: no delta pruning. The exchange value is read by the
//! rule above and by nothing else yet; ranking the losing captures by it
//! is a separate, independently match-tested change. Folded in together
//! the result would not identify which change helped or hurt.
//!
//! **Determinism.** The node count at a fixed depth is a function of the
//! position, the code, and the state of the table it is given: both move
//! lists are generated in a fixed order, the root order
//! depends only on the previous iteration, the table's index and
//! replacement are integer functions of the key and the generation, and
//! the clock is consulted only when there is a time budget -- under a
//! depth or node limit `Instant::now()` is never read on a decision path.
//! There is no hash map, no float, no thread. The killers are cleared when
//! a search starts, so they are state within one search and never across
//! two. `bench` supplies a table of a fixed size and clears it between
//! positions, which is what turns "and the state of the table" back into
//! "the code alone". Quiescence nodes are nodes: they count, and they
//! check the limits.
//!
//! **Stopping.** The stop flag, a node limit and the hard time budget end
//! the search mid-iteration, at any node from the first; the iteration's
//! partial result is discarded and the last completed one stands. Stopped
//! inside its first iteration, the search returns the best root move it
//! has fully searched -- the first root move, if not even one -- so there
//! is always a move. The soft budget only prevents starting another
//! iteration. Under `infinite` the search returns only when `stop` is
//! raised, as the protocol requires.

use std::io::Write;
use std::iter::Peekable;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{Colour, MAX_PLY, Move, MoveList, generate_legal, generate_noisy, to_uci};

use crate::eval;
use crate::picker;
use crate::score::{self, DRAW, INFINITE, Score, mated_in};
use crate::see;
use crate::time::{self, Budget};
use crate::tt::{Bound, Table};

/// What a `go` command asked for. A field that is `None` did not appear and
/// must not influence the search.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// `go depth N`: stop after completing iteration N.
    pub depth: Option<u32>,
    /// `go nodes N`: stop after N nodes.
    pub nodes: Option<u64>,
    /// `go movetime N`: search for exactly N milliseconds.
    pub movetime: Option<u64>,
    /// `go infinite`: search until `stop`, and do not return before it.
    pub infinite: bool,
    /// `wtime` / `btime`, in milliseconds, indexed by `Colour`.
    pub time: [Option<u64>; 2],
    /// `winc` / `binc`, in milliseconds, indexed by `Colour`.
    pub inc: [Option<u64>; 2],
    /// `movestogo N`: moves until the next time control.
    pub movestogo: Option<u32>,
}

impl Limits {
    /// Parse the tokens that follow `go`.
    ///
    /// Unknown tokens are skipped; a known token without a number, or with
    /// something that is not a number, is skipped too. `ponder`,
    /// `searchmoves` and `mate` are accepted and ignored. A negative clock
    /// -- which a GUI sends when a side has already overstepped -- reads as
    /// zero rather than as "no clock", so the search still hurries.
    #[must_use]
    pub fn parse<'a>(tokens: impl Iterator<Item = &'a str>) -> Limits {
        let mut limits = Limits::default();
        let mut it = tokens.peekable();
        while let Some(token) = it.next() {
            match token {
                "infinite" => limits.infinite = true,
                "depth" => limits.depth = small(&mut it),
                "nodes" => limits.nodes = large(&mut it),
                "movetime" => limits.movetime = large(&mut it),
                "wtime" => limits.time[Colour::White.index()] = large(&mut it),
                "btime" => limits.time[Colour::Black.index()] = large(&mut it),
                "winc" => limits.inc[Colour::White.index()] = large(&mut it),
                "binc" => limits.inc[Colour::Black.index()] = large(&mut it),
                "movestogo" => limits.movestogo = small(&mut it),
                // Accepted and ignored. `mate` takes a number, which is
                // consumed so it is not mistaken for anything else; the
                // moves after `searchmoves` are unknown tokens and fall
                // through to the arm below.
                "mate" => {
                    let _ = small(&mut it);
                }
                _ => {}
            }
        }
        limits
    }

    /// A fixed-depth search.
    #[must_use]
    pub fn depth(depth: u32) -> Limits {
        Limits {
            depth: Some(depth),
            ..Limits::default()
        }
    }

    /// Search until `stop`.
    #[must_use]
    pub fn infinite() -> Limits {
        Limits {
            infinite: true,
            ..Limits::default()
        }
    }

    /// The side to move's clock and increment, if a clock was given.
    #[must_use]
    pub fn clock(&self, us: Colour) -> Option<(u64, u64)> {
        self.time[us.index()].map(|t| (t, self.inc[us.index()].unwrap_or(0)))
    }

    /// Whether this `go` named a clock at all, for either side.
    ///
    /// The distinction [`clock`](Limits::clock) cannot make. A `go` that
    /// named no clock is unbounded *by design* -- `bench`, `go depth`, `go
    /// nodes`, `go infinite` -- and the search must never read the time,
    /// which is what keeps the node count a function of the code alone. A
    /// `go` that named a clock for the other side and
    /// not for ours is not unbounded, it is a `go` we know nothing about our
    /// own time from, and treating the two alike is how `go wtime 1000 winc
    /// 10` with Black to move came to search until `stop`.
    ///
    /// An increment or a `movestogo` on its own counts. A GUI does not send
    /// one without a clock, and if one did, the same reasoning applies: it
    /// spoke about the clock and did not tell us ours.
    #[must_use]
    pub fn is_clocked(&self) -> bool {
        self.time.iter().chain(self.inc.iter()).any(Option::is_some) || self.movestogo.is_some()
    }
}

/// The next token as a non-negative number, consumed only if it is one. A
/// token that does not parse is left for the main loop, which skips it --
/// it may be the next keyword.
fn large<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> Option<u64> {
    let n: i64 = it.peek()?.parse().ok()?;
    it.next();
    Some(u64::try_from(n.max(0)).expect("non-negative"))
}

fn small<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> Option<u32> {
    large(it).map(|n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// How often the clock is read, in nodes. A power of two.
const CLOCK_INTERVAL: u64 = 1024;

/// The deepest iteration: `MAX_PLY`, which is the state stack's bound. With
/// no extensions, depth is the deepest ply the main search reaches; the
/// quiescence search below it stops at `MAX_PLY` and stands on the
/// evaluation there.
const MAX_DEPTH: u32 = MAX_PLY as u32;
const _: () = assert!(MAX_DEPTH as usize == MAX_PLY);

/// Move `first` to the head of `list`, keeping the rest in the order they
/// were generated in. Returns whether `list` held it at all.
///
/// This is the whole of the interior nodes' move ordering: the move a
/// transposition-table hit named, tried before anything else. It is a free
/// function rather than a method for the same reason [`tt::verify`] is: the
/// thing worth testing is a total function of a list and a move, and a gate
/// that can call it directly does not have to reach it through a search.
///
/// **It rotates rather than swaps.** A swap would put whatever was at the
/// head into the middle of the list, and generation order is the only order
/// the rest of the list has. Rotating keeps it, which is what the root
/// already does with the previous iteration's best move, and what a later
/// stable sort over the remainder would rest on.
///
/// **It validates, and the validation costs nothing**, because finding the
/// move is the operation: a move the list does not hold has no index to
/// rotate to the front, and "not found" is the same answer as "not legal
/// here". A stale or colliding table entry therefore cannot make the search
/// play a move absent from the freshly generated legal list.
/// `Move::NULL` is `a1a1` quiet, which no generator emits, so the scan
/// would refuse it too; the early return says so without walking the list.
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

/// Remember `m` as a killer of the ply `killers` belongs to: the quiet move
/// that caused a beta cutoff there, to be tried at that ply's other nodes
/// ahead of the quiet moves that have refuted nothing.
///
/// A free function rather than a method for the same reason [`order_first`]
/// is: the thing worth testing is a total function of two slots and a move,
/// and a gate that can call it directly does not have to reach it through a
/// search.
///
/// **Only a quiet move is remembered.** A noisy move is already ordered by
/// what it captures, and a slot spent on one is a slot not spent on the move
/// the stage exists for.
///
/// **Slot zero shifts into slot one**, unless `m` is already slot zero,
/// where the shift would fill both slots with one move and leave the stage
/// one move wide. A move already in slot one is promoted, which swaps the
/// two.
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
    /// The transposition table. Shared with whoever else searches: the UCI
    /// session keeps one across the whole game, `bench` clears one between
    /// positions.
    tt: &'a Table,
    /// Spells castling moves in `info` lines the way the GUI expects.
    chess960: bool,
    nodes: u64,
    start: Instant,
    /// The time budget, when anything in `limits` constrains the time.
    /// `None` means the clock is never read.
    budget: Option<Budget>,
    /// Set when a limit or the stop flag ends the search mid-iteration;
    /// the iteration's partial result is discarded, unless it is the first
    /// iteration's, which is all there is.
    aborted: bool,
    completed_depth: u32,
    best: Move,
    score: Score,
    /// The principal variation of the last completed iteration.
    pv: Vec<Move>,
    /// The triangular table the iteration in progress writes.
    table: PvTable,
    /// Two quiet moves per ply: the ones that caused a beta cutoff at a
    /// node of that ply, tried at its other nodes ahead of the quiet moves
    /// that have refuted nothing. Indexed by ply, cleared when a search
    /// starts, and a kibibyte inline like `PvTable`'s row lengths.
    killers: [[Move; 2]; MAX_PLY],
}

impl<'a> Search<'a> {
    #[must_use]
    pub fn new(limits: Limits, stop: &'a AtomicBool, tt: &'a Table) -> Search<'a> {
        Search {
            limits,
            stop,
            tt,
            chess960: false,
            nodes: 0,
            start: Instant::now(),
            budget: None,
            aborted: false,
            completed_depth: 0,
            best: Move::NULL,
            score: DRAW,
            pv: Vec::with_capacity(MAX_PLY),
            table: PvTable::new(),
            killers: [[Move::NULL; 2]; MAX_PLY],
        }
    }

    /// Spell castling moves in `info` lines per `UCI_Chess960`.
    pub fn set_chess960(&mut self, on: bool) {
        self.chess960 = on;
    }

    /// The best move in `board`, or `Move::NULL` when there is none.
    ///
    /// Returns when the limits are met or `stop` is raised; under
    /// `infinite`, only when `stop` is raised. `info` lines go to `out`.
    /// `board` comes back at the ply it went in.
    pub fn run(&mut self, board: &mut Board, out: &mut dyn Write) -> Move {
        self.tt.new_search();
        self.start = Instant::now();
        self.nodes = 0;
        self.aborted = false;
        self.completed_depth = 0;
        self.best = Move::NULL;
        self.pv.clear();
        self.killers = [[Move::NULL; 2]; MAX_PLY];
        self.budget = if self.limits.infinite {
            None
        } else {
            time::budget(&self.limits, board.side_to_move())
        };

        let legal = generate_legal(board);
        if legal.is_empty() {
            self.score = if board.in_check() { mated_in(0) } else { DRAW };
            self.wait_if_infinite();
            return Move::NULL;
        }
        let mut root_moves: Vec<Move> = legal.iter().collect();

        let max_depth = self.limits.depth.unwrap_or(u32::MAX).clamp(1, MAX_DEPTH);
        for depth in 1..=max_depth {
            let (best, score) = self.search_root(board, &root_moves, depth);
            if self.aborted {
                // The last completed iteration stands. If there is none,
                // the best root move fully searched so far does -- the
                // first root move, if not even one was -- so that there is
                // always a move. With the quiescence search below it, the
                // first iteration is not small: a quarter of a million
                // nodes in Kiwipete.
                if self.completed_depth == 0 {
                    self.best = best;
                    self.score = if score == -INFINITE { DRAW } else { score };
                    self.pv.clear();
                    self.pv.push(best);
                }
                break;
            }
            self.completed_depth = depth;
            self.best = best;
            self.score = score;
            self.pv.clear();
            self.pv.extend_from_slice(self.table.line(0));
            self.report(board, out);
            // The best move first next time: what makes an aborted
            // iteration's fallback -- the previous iteration -- a good one,
            // and the only ordering there is.
            if let Some(i) = root_moves.iter().position(|&m| m == best) {
                root_moves[..=i].rotate_right(1);
            }
            if let Some(b) = self.budget
                && self.elapsed_ms() >= b.soft
            {
                break;
            }
        }
        self.wait_if_infinite();
        self.best
    }

    /// The root: every move, full window, the best move and its score.
    fn search_root(&mut self, board: &mut Board, moves: &[Move], depth: u32) -> (Move, Score) {
        self.nodes += 1;
        self.table.clear(0);
        let mut alpha = -INFINITE;
        let beta = INFINITE;
        let mut best = moves[0];
        let mut best_score = -INFINITE;
        for &m in moves {
            board.make_move(m);
            let score = -self.negamax(board, depth - 1, 1, -beta, -alpha);
            board.unmake_move(m);
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

    /// Negamax with alpha-beta, fail-soft. The value returned after an abort
    /// is meaningless and is discarded by every caller.
    fn negamax(
        &mut self,
        board: &mut Board,
        depth: u32,
        ply: usize,
        mut alpha: Score,
        beta: Score,
    ) -> Score {
        // The horizon: the quiescence search takes over, and counts the node.
        if depth == 0 {
            return self.quiesce(board, ply, alpha, beta);
        }
        // Depth is at most MAX_DEPTH at ply zero and falls by one per ply,
        // so an interior node is always inside the state stack.
        debug_assert!(ply < MAX_PLY);
        self.nodes += 1;
        self.table.clear(ply);
        // At every node, so that `nodes N` stops at N and not at N plus a
        // subtree.
        if self.out_of_time() {
            return DRAW;
        }

        // A repeated position is a draw wherever it is met: twofold inside
        // the tree, threefold against the game history.
        if board.is_repetition() {
            return DRAW;
        }

        // The table, before the moves are generated: that saving is most of
        // what it is for. The cutoff is withheld at the fifty-move limit,
        // where this node is a draw or a mate by rule whatever any subtree
        // found, because the key does not carry the halfmove clock.
        let key = board.key();
        let mut tt_move = Move::NULL;
        if let Some(hit) = self.tt.probe(key) {
            // The move is worth having whatever the depth says. A hit too
            // shallow to answer the question still names the move that
            // answered it before, and `verify` matched the whole key, so
            // that move belongs to this position.
            tt_move = hit.mv;
            if u32::from(hit.depth) >= depth && board.halfmove_clock() < 100 {
                let score = score::from_tt(hit.score, ply);
                let cutoff = match hit.bound {
                    Bound::Exact => true,
                    Bound::Lower => score >= beta,
                    Bound::Upper => score <= alpha,
                };
                if cutoff {
                    return score;
                }
            }
        }

        let mut legal = generate_legal(board);
        if legal.is_empty() {
            return if board.in_check() {
                mated_in(ply)
            } else {
                DRAW
            };
        }
        // After the mate check: a mate delivered on the hundredth half-move
        // is a mate, not a draw.
        if board.halfmove_clock() >= 100 {
            return DRAW;
        }
        // The ordering, in three stages and one sort. The table's move
        // first, refused here rather than played if the list does not hold
        // it; then the noisy moves by MVV-LVA; then this ply's killers, and
        // behind them the quiet moves that have refuted nothing, in
        // generation order. The sort starts one move in when the rotation
        // happened, so that the table's move keeps its place whatever it
        // ranks. A killer is a move that cut at a sibling and may not be
        // legal here, which needs no check: it matches nothing in the list.
        let ordered = usize::from(order_first(&mut legal, tt_move));
        picker::sort_from(board, &mut legal, ordered, self.killers[ply]);

        let original_alpha = alpha;
        let mut best = -INFINITE;
        let mut best_move = Move::NULL;
        for m in legal.iter() {
            board.make_move(m);
            let score = -self.negamax(board, depth - 1, ply + 1, -beta, -alpha);
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
                        break;
                    }
                }
            }
        }
        // Fail-soft, so the bound follows the value and not the window it
        // was found in. Nothing an aborted search computed is stored: the
        // loop above returns before this line.
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
        best
    }

    /// The quiescence search: the horizon, made quiet before it is
    /// evaluated.
    ///
    /// Out of check the side to move may **stand pat**: the static
    /// evaluation is a lower bound on what it can get, because it is never
    /// obliged to capture, so it is the score to beat and a cutoff on its
    /// own when it is at or above beta. Then every noisy move -- capture,
    /// en passant, promotion -- is tried, most valuable victim first
    /// (`picker::sort_noisy`), each followed by this search again, **except
    /// the ones that lose material**: a move whose static exchange value
    /// is negative (`see::see`) is refused without being searched. The
    /// side to move was never obliged to make it, and its result could not
    /// beat standing pat by the material it gives up. The value is computed
    /// as each move comes up, not for the whole list at sort time, so the
    /// captures behind a cutoff cost nothing, and it is why this list is
    /// the one `picker` does not sort by the exchange: the moves a lower
    /// band would move are the moves this rule has already removed. The
    /// ordering is not a refinement: in generated order, depth one from
    /// Kiwipete is 159 million nodes. In check there is no standing pat and every
    /// evasion is tried, quiet ones included and losing ones included: a
    /// side that cannot get out
    /// of check is mated, and a check is answered, not ignored. Those
    /// evasions go through the same sort (`picker::sort_from`, no
    /// killers), which puts the capture of the checking piece ahead of the
    /// king retreats the generator emits first and puts an evasion whose
    /// exchange loses material behind the ones that do not; each retreat is
    /// another position to answer a check in, so the order is worth as much
    /// here as it is among the captures. The search ends where the noisy moves
    /// run out, which material bounds, or at `MAX_PLY`, where the state
    /// stack ends and the evaluation stands whether or not the side to
    /// move is in check. Fail-soft, like the
    /// main search; it writes no principal variation, so the pv ends at the
    /// horizon.
    ///
    /// Draws as in the main search: a repetition wherever it is met; the
    /// fifty-move rule after the mate check in check, before the evaluation
    /// out of it. Stalemate is not seen here -- quiet moves are not
    /// generated -- which is the horizon effect this search accepts.
    fn quiesce(&mut self, board: &mut Board, ply: usize, mut alpha: Score, beta: Score) -> Score {
        self.nodes += 1;
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
            // After the mate check: a mate on the hundredth half-move is a
            // mate.
            if board.halfmove_clock() >= 100 {
                return DRAW;
            }
            // Noisy evasions first, by victim; the quiet ones keep the
            // order the generator emitted them in, behind all of those.
            // No killers: whether this ply's two rank the quiet evasions
            // usefully is unmeasured, and a second change.
            picker::sort_from(board, &mut evasions, 0, [Move::NULL; 2]);
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
            // A losing capture is refused, out of check only: in check the
            // list is the legal list and every entry answers the check.
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

    /// Whether a limit or the stop flag ends the search here, at any node
    /// from the first; the clock only every `CLOCK_INTERVAL` nodes, and
    /// only when there is a budget.
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
            && self.nodes >= n
        {
            self.aborted = true;
            return true;
        }
        if self.nodes & (CLOCK_INTERVAL - 1) == 0
            && let Some(b) = self.budget
            && self.elapsed_ms() >= b.hard
        {
            self.aborted = true;
            return true;
        }
        false
    }

    /// "Do not exit the search without being told so in this mode."
    fn wait_if_infinite(&self) {
        if self.limits.infinite {
            while !self.stop_requested() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// One `info` line for the iteration just completed. The pv is spelled
    /// by walking it on the board, so castling reads per the option.
    fn report(&self, board: &mut Board, out: &mut dyn Write) {
        let ms = self.elapsed_ms();
        let nps = self.nodes * 1000 / ms.max(1);
        let mut line = format!(
            "info depth {} score {} nodes {} nps {nps} time {ms} pv",
            self.completed_depth,
            score::uci(self.score),
            self.nodes
        );
        let mut made = 0;
        for &m in &self.pv {
            let legal = generate_legal(board);
            if !legal.contains(m) {
                break;
            }
            line.push(' ');
            line.push_str(&to_uci(m, &legal, self.chess960));
            board.make_move(m);
            made += 1;
        }
        for &m in self.pv[..made].iter().rev() {
            board.unmake_move(m);
        }
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    /// Nodes searched so far.
    #[must_use]
    pub fn nodes(&self) -> u64 {
        self.nodes
    }

    /// The depth of the last completed iteration; zero before any.
    #[must_use]
    pub fn completed_depth(&self) -> u32 {
        self.completed_depth
    }

    /// The root score of the last completed iteration, from the side to
    /// move's point of view.
    #[must_use]
    pub fn score(&self) -> Score {
        self.score
    }

    /// The principal variation of the last completed iteration.
    #[must_use]
    pub fn pv(&self) -> &[Move] {
        &self.pv
    }

    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    #[must_use]
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// The triangular principal-variation table: row `ply` holds the best line
/// found from ply `ply` in the subtree being searched. Allocated once per
/// search; `MAX_PLY` squared moves, 128 KiB, on the heap.
struct PvTable {
    rows: Box<[Move]>,
    len: [usize; MAX_PLY],
}

impl PvTable {
    fn new() -> PvTable {
        PvTable {
            rows: vec![Move::NULL; MAX_PLY * MAX_PLY].into_boxed_slice(),
            len: [0; MAX_PLY],
        }
    }

    #[inline]
    fn clear(&mut self, ply: usize) {
        if ply < MAX_PLY {
            self.len[ply] = 0;
        }
    }

    /// `m` is the new best at `ply`: row `ply` becomes `m` followed by row
    /// `ply + 1`.
    fn update(&mut self, ply: usize, m: Move) {
        if ply >= MAX_PLY {
            return;
        }
        let (child_len, child_start) = if ply + 1 < MAX_PLY {
            (self.len[ply + 1], (ply + 1) * MAX_PLY)
        } else {
            (0, 0)
        };
        let row = ply * MAX_PLY;
        self.rows[row] = m;
        // Rows never overlap, so the copy is between disjoint ranges.
        let n = child_len.min(MAX_PLY - 1);
        for i in 0..n {
            self.rows[row + 1 + i] = self.rows[child_start + i];
        }
        self.len[ply] = n + 1;
    }

    fn line(&self, ply: usize) -> &[Move] {
        let row = ply * MAX_PLY;
        &self.rows[row..row + self.len[ply]]
    }
}
