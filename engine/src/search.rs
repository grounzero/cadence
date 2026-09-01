// SPDX-License-Identifier: GPL-3.0-or-later

//! The search, and what bounds it.
//!
//! `Limits` is the parsed `go` command. `Search` is one search: it holds the
//! per-thread state -- there are no globals -- and runs in whichever thread
//! calls it. The UCI layer gives it a board of its own and a stop flag, and
//! prints whatever it returns.
//!
//! **What bounds a line of checks.** Where every move gives check the
//! depth never falls, so nothing conditioned on depth ends the line. Four
//! things do: a repetition is a draw wherever it is met; the fifty-move
//! rule ends any sequence that neither captures nor moves a pawn;
//! [`EXTEND_WITHIN`] refuses the extension past a multiple of the root
//! depth; and the ply bound below holds the arrays when that cap is above
//! `MAX_PLY`.
//!
//! **The ply bound.** Both searches stop at `MAX_PLY` whatever depth is
//! left and answer with the static evaluation: the per-ply arrays end
//! there, and in release a read past their end is a bounds check rather
//! than the assertion a debug build gets. It landed before anything
//! extended, when no search could reach it, and the extension above is what
//! makes it reachable: it is the floor the cap does not stand on.
//!
//! **What it is not, on purpose.** No extension on anything but a
//! check, and no pruning beyond the two rules above and the quiescence
//! exchange rule below: no delta pruning. That
//! search extends nothing and has nothing to extend: it carries no depth,
//! and in check it already answers with every legal evasion. Folded in
//! together the result would not identify which change helped or hurt.
//!
//! **Determinism.** The node count at a fixed depth is a function of the
//! position, the code, and the state of the table it is given: both move
//! lists are generated in a fixed order, the root order
//! depends only on the previous iteration, the table's index and
//! replacement are integer functions of the key and the generation, and
//! the clock is consulted only when there is a time budget -- under a
//! depth or node limit `Instant::now()` is never read on a decision path.
//! There is no hash map, no float, no thread. The killers and the history
//! table are cleared when a search starts, so both are state within one
//! search and never across two. `bench` supplies a table of a fixed size and clears it between
//! positions, which is what turns "and the state of the table" back into
//! "the code alone". Quiescence nodes are nodes: they count, and they
//! check the limits.
//!
//! **Stopping.** The stop flag, a node limit and the hard budget end the
//! search mid-iteration, and the last completed iteration stands. Stopped
//! inside the first, the search returns the best root move it has fully
//! searched, or the first root move if not even one, so there is always a
//! move to print. Under `infinite` it returns only when `stop` is raised.

use std::io::Write;
use std::iter::Peekable;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadence_core::position::Board;
use cadence_core::{
    Colour, MAX_PLY, Move, MoveList, PieceType, generate_legal, generate_noisy, to_uci,
};

use crate::eval;
use crate::history::{self, History};
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

    /// Whether this `go` named a clock at all, for either side. A `go` that
    /// named the other side's clock and not ours is clocked: nothing is
    /// known about our own time, and the safe reading of that is zero rather
    /// than unlimited.
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

/// The deepest iteration: `MAX_PLY`, which is the state stack's bound. It
/// is no longer the deepest ply either search reaches -- a check extension
/// gives a ply back, and [`extension`] states the bound that replaces it --
/// so both searches stop at `MAX_PLY` whatever depth is left and stand on
/// the evaluation there.
const MAX_DEPTH: u32 = MAX_PLY as u32;
const _: () = assert!(MAX_DEPTH as usize == MAX_PLY);

/// Move `first` to the head of `list`, keeping the rest in the order they
/// were generated in. Returns whether `list` held it at all.
///
/// **It validates, and the validation costs nothing**, because finding the
/// move is the operation: a move the list does not hold has no index to
/// rotate to the front. A stale or colliding table entry therefore cannot
/// make the search play a move absent from the freshly generated legal
/// list.
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
/// that caused a beta cutoff at a sibling of this node, newest first, with
/// no duplicates.
pub fn remember_killer(killers: &mut [Move; 2], m: Move) {
    if m.is_noisy() || killers[0] == m {
        return;
    }
    killers[1] = killers[0];
    killers[0] = m;
}

/// How many times the root depth a check extension is granted within.
///
/// The cap is doing the job [`extension`] gives it, which is bounding the
/// pathological line, and it is not shaping the ordinary tree: at the depths
/// the bench reaches, what ends an extended line is depth running out rather
/// than ply.
const EXTEND_WITHIN: usize = 2;

/// How much deeper the child of `m` is searched, in plies: one when the
/// move gave check, none otherwise. `check` is the child's
/// `Board::in_check`, read after the move is made.
///
/// **The cap is on ply, and a cap on depth would not be a cap.** Where every
/// move gives check the depth never falls, so a rule of the form "extend
/// while at least so much depth is left" stays true for as long as the
/// checks do. Past [`EXTEND_WITHIN`] times the root depth no child is
/// granted one, so the deepest interior node an iteration can produce is at
/// ply `(EXTEND_WITHIN + 1) * root_depth - 2`. That is above `MAX_PLY` for a
/// root depth over 85, and what holds there is the ply bound in
/// [`Search::negamax`].
#[must_use]
pub fn extension(check: bool, ply: usize, root_depth: u32) -> u32 {
    u32::from(check && ply < EXTEND_WITHIN * root_depth as usize)
}

/// How many plies past the one the move would have cost a null-move
/// verification is shortened by: the reduced search runs at
/// `depth - 1 - null_reduction(depth)`, floored at zero, where the floor
/// hands the question to the quiescence search.
#[must_use]
pub fn null_reduction(depth: u32) -> u32 {
    3 + depth / 3
}

/// The first index a late move's search may be shortened at, and the first
/// a late move may be given up at.
///
/// **One constant read by two rules, and the tie is the argument for
/// [`lmp_count`]'s floor rather than a convenience.** A move inside this
/// prefix is one the search will not shorten by a single ply on the
/// strength of its rank; giving it up entirely on the same evidence is the
/// larger claim, so the rule that cannot re-search takes its floor from the
/// rule that can.
pub const REDUCTION_INDEX: usize = 3;

/// How many plies a late move's first search is shortened by: zero for the
/// first [`REDUCTION_INDEX`] moves of a node, zero below depth three, and
/// otherwise one plus a quarter of the product of the two integer
/// logarithms. The caller holds the exemptions. `ilog2` because the
/// determinism contract admits no float on a path that decides a move.
#[must_use]
pub fn lmr_reduction(depth: u32, index: usize) -> u32 {
    if depth < 3 || index < REDUCTION_INDEX {
        return 0;
    }
    1 + depth.ilog2() * index.ilog2() / 4
}

/// How many plies the first search of move `m`, at `index` in its node's
/// sorted list, is shortened by. Zero at a node in check, for a move that
/// gives check, for a noisy move and for a killer; otherwise
/// [`lmr_reduction`].
#[must_use]
pub fn reduction(
    in_check: bool,
    gives_check: bool,
    m: Move,
    killers: [Move; 2],
    depth: u32,
    index: usize,
) -> u32 {
    if in_check || gives_check || m.is_noisy() || m == killers[0] || m == killers[1] {
        return 0;
    }
    lmr_reduction(depth, index)
}

/// How many plies a late move whose base reduction is `base` is actually
/// reduced by, once its history score is read.
///
/// **It adjusts a reduction and never creates one.** A base of zero comes
/// back zero, so every exemption [`reduction`] holds survives whatever the
/// table says, and so does the depth threshold.
#[must_use]
pub fn history_reduction(base: u32, history: i32) -> u32 {
    if base == 0 {
        return 0;
    }
    base.saturating_add_signed(-history::shift(history))
}

/// Whether the side to move has any piece beside its pawns and king.
///
/// The null move's zugzwang guard, and the one condition there that is a
/// chess claim rather than a search claim: passing is what a side in
/// zugzwang wants and may not have, so the evidence a null move collects is
/// inverted exactly there.
#[must_use]
pub fn has_non_pawn_material(board: &Board) -> bool {
    let us = board.side_to_move();
    (board.by_colour(us) & !(board.by_type(PieceType::Pawn) | board.by_type(PieceType::King))).any()
}

/// Whether the side to move's position is improving: the static evaluation
/// written at this ply against the one two plies back. `false` wherever
/// either reading is missing, because a rule reading this flag wants "known
/// to be getting better", and an unknown is not that.
#[must_use]
pub fn improving(evals: &[Option<Score>], ply: usize) -> bool {
    let now = evals.get(ply).copied().flatten();
    let then = ply
        .checked_sub(2)
        .and_then(|p| evals.get(p))
        .copied()
        .flatten();
    match (now, then) {
        (Some(now), Some(then)) => now > then,
        _ => false,
    }
}

/// The deepest node at which a quiet move may be given up for its place in
/// the order.
///
/// **Eight, and what bounds it is where the reduction still has something
/// to delete.** Over the bench positions the moves this rule would give up
/// are ones the reduction searches at reduced depth in 94% of cases from
/// depth four up; at depths nine and above there are 24,000 of them against
/// 13.3 million inside the band, so the rule is off there because there is
/// nothing there rather than because it is unsafe there. What makes the
/// limit a limit is the other end: a node eight plies from the horizon is
/// the deepest at which this project is willing to delete a move on its
/// rank alone.
const LMP_DEPTH: u32 = 8;

/// What the count of moves a node searches grows by: the square of the
/// remaining depth over this.
///
/// **The square rather than the slope, and one number rather than two.**
/// The count has to grow faster than the move list does or the rule turns
/// off by itself at the depths where the list is longest, which a linear
/// count does; the base is [`REDUCTION_INDEX`] and is not a parameter, so
/// the shape arrives with a single number to tune, as
/// [`FUTILITY_MARGIN`]'s slope does.
///
/// **Two, which is timider than the counts published elsewhere, and it was
/// chosen against this tree rather than against them.** Six shapes of this
/// rule were measured on move agreement against a consensus of all of them,
/// and the four that give up more than this one all came back net negative
/// while this one did not. What the field fits its counts on is a tree whose
/// late quiet moves are ranked by continuation histories; ours ranks them by
/// one butterfly table with a lifetime of one search, and a rule that
/// deletes a move on its rank is worth what that rank is worth. So a timid
/// count is the right answer to a weaker signal rather than to a weaker
/// search, which is the reduction's own reading arriving at a second rule.
const LMP_DIVISOR: u32 = 2;

/// How many moves a node at `depth` searches before the quiet moves behind
/// them are given up.
///
/// Total for [`futility_margin`]'s reason, and it cannot overflow: `depth`
/// is bounded by [`LMP_DEPTH`] at the one place that reads it against a
/// node, and the product is taken in `u32`.
#[must_use]
pub fn lmp_count(depth: u32) -> usize {
    REDUCTION_INDEX + (depth.saturating_mul(depth) / LMP_DIVISOR) as usize
}

/// The index from which this node gives up its quiet moves, or `None` where
/// the rule cannot fire here at all: past [`LMP_DEPTH`], at a node in check,
/// or at a node with no move the count does not already admit.
///
/// **In check is refused here and not left to the move.** Every legal move
/// at such a node is an evasion and what a wrong skip loses there is a mate
/// defence, which is the exemption this rule's asymmetry bears on hardest:
/// [`reduction`] refuses the same node and can afford to be wrong, because
/// a reduced search that beats alpha is re-run.
///
/// A node-level question read once, like [`futile_node`], with the
/// move-level question at [`lmp_skips`].
#[must_use]
pub fn lmp_index(in_check: bool, depth: u32, moves: usize) -> Option<usize> {
    let count = lmp_count(depth);
    (!in_check && depth <= LMP_DEPTH && moves > count).then_some(count)
}

/// Whether the move at `index` of a node [`lmp_index`] admitted is a
/// candidate to be given up without being searched: never a noisy move,
/// never a killer, and never inside the count. The check exemption is the
/// caller's, because it is the one question here that costs anything, which
/// is [`futility_skips`]'s division of the same work.
///
/// **The killer exemption comes from [`reduction`] and not from
/// [`futility_skips`], which is the one place the two lists here disagree.**
/// The margin's trigger is a claim about the node -- no quiet move can reach
/// alpha from where the evaluation stands -- and it holds over a killer like
/// anything else. This rule's trigger is a claim about the move's rank
/// alone, and a killer's rank is the one in the list the search has evidence
/// about: it cut at a sibling. A rule that can re-search still declines to
/// bet a ply against that evidence, so a rule that cannot declines to delete
/// the move.
///
/// **A noisy move is refused for the sort's sake rather than the move's.**
/// `picker` ranks every noisy move that keeps material above every quiet
/// one, so a count read against the sorted index would delete the moves the
/// sort ranked highest at a node holding more captures than the count
/// admits. What prunes a noisy move at an interior node is the exchange
/// evaluation, which is a rule of its own.
#[must_use]
pub fn lmp_skips(from: Option<usize>, m: Move, killers: [Move; 2], index: usize) -> bool {
    from.is_some_and(|count| index >= count) && !m.is_noisy() && m != killers[0] && m != killers[1]
}

/// The deepest node at which a quiet move may be skipped for the margin.
///
/// **Below the limit the rule is off, not weaker.** A node outside the band
/// searches every move it generates, so the exemptions at [`futility_skips`]
/// are the only thing that has to be right about the nodes inside it.
const FUTILITY_DEPTH: u32 = 3;

/// What the margin grows by per ply of remaining depth, in centipawns.
///
/// **Linear rather than squared because the evidence is linear**: the
/// material a search can win grows with the moves it has, not with their
/// square. A constant term beside the slope is the obvious second parameter
/// and is left out on purpose, so this rule has one number to tune.
const FUTILITY_MARGIN: Score = 150;

/// How far below alpha a node's static evaluation may sit and still have
/// its quiet moves searched: [`FUTILITY_MARGIN`] per ply of `depth`.
#[must_use]
pub fn futility_margin(depth: u32) -> Score {
    // Saturating, and total for that reason: no caller passes a depth
    // outside the band, and a function that is only right for the
    // arguments something happens to hand it is one a gate cannot pin.
    // [`futile_node`] adds it to the evaluation with a saturating add for
    // the same reason, so the pair cannot overflow at any depth at all.
    FUTILITY_MARGIN.saturating_mul(Score::try_from(depth).unwrap_or(Score::MAX))
}

/// Whether a node may skip quiet moves for the margin: its static
/// evaluation plus [`futility_margin`] still does not reach `alpha`. Alpha
/// on the mate scale refuses it, and in check `evals[ply]` is `None`, so the
/// rule cannot read anything and cannot fire. Read once, before the move
/// loop, and alpha only rises inside it.
#[must_use]
pub fn futile_node(eval: Option<Score>, depth: u32, alpha: Score) -> bool {
    let Some(eval) = eval else {
        return false;
    };
    depth <= FUTILITY_DEPTH
        && !score::is_mate(alpha)
        && eval.saturating_add(futility_margin(depth)) <= alpha
}

/// Whether the move at `index` of a node [`futile_node`] admitted is a
/// candidate to be skipped without being searched: never the node's first
/// move, and never a noisy one. The check exemption is the caller's, because
/// it is the one question here that costs anything.
#[must_use]
pub fn futility_skips(futile: bool, m: Move, index: usize) -> bool {
    futile && index > 0 && !m.is_noisy()
}

/// What the margin a node is returned on grows by per ply of remaining
/// depth, in centipawns.
///
/// **It is the only thing bounding this rule**, because there is no depth
/// limit here, so it is chosen where it bounds as well as where it sizes.
const REVERSE_FUTILITY_MARGIN: Score = 150;

/// How far above `beta` a node's static evaluation must stand before the
/// node is returned without being searched: [`REVERSE_FUTILITY_MARGIN`] per
/// ply of `depth`.
#[must_use]
pub fn reverse_futility_margin(depth: u32) -> Score {
    // Saturating, and total for that reason, like [`futility_margin`].
    REVERSE_FUTILITY_MARGIN.saturating_mul(Score::try_from(depth).unwrap_or(Score::MAX))
}

/// The bound a node may be returned at without being searched at all: its
/// static evaluation less [`reverse_futility_margin`], where that still
/// stands at or above `beta`. `None` is a node that has to be searched.
///
/// **What it returns is the quantity the condition established, not the
/// evaluation.** `eval - margin` is the least this rule can claim, and
/// returning `eval` would claim back the margin that was there to discount
/// it. Everywhere else here fail-soft means the bound follows the value the
/// search found; nothing is searched here, so it follows the condition.
///
/// **There is no depth limit**, and that is the one place this rule departs
/// from every formulation of it elsewhere: the margin already is one.
///
/// **In check needs no exemption**, for [`futile_node`]'s reason: `evals`
/// holds `None` wherever the side to move is in check. **A beta on the mate
/// scale refuses it**, because a mate score is not a quantity a centipawn
/// margin is commensurable with.
///
/// A free function for the same reason [`extension`] is.
#[must_use]
pub fn reverse_futile(eval: Option<Score>, depth: u32, beta: Score) -> Option<Score> {
    let bound = eval?.saturating_sub(reverse_futility_margin(depth));
    (!score::is_mate(beta) && bound >= beta).then_some(bound)
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
    /// What each quiet move has been worth across this whole search: the
    /// butterfly table `picker` ranks the quiet band by and the reduction
    /// reads through [`history_reduction`]. Cleared beside the killers, so
    /// the two have one lifetime; thirty-two kibibytes, on the heap for
    /// `PvTable`'s reason rather than inline for `killers`'.
    history: History,
    /// The depth of the iteration in progress, which is what the check
    /// extension's ply cap is a multiple of. Set at the head of each
    /// iteration; a field rather than a sixth argument to `negamax`
    /// because it does not change inside one.
    root_depth: u32,
    /// The static evaluation at each ply of the line being searched:
    /// written at every interior node of the main search that survives the
    /// table probe, `None` where the side to move is in check, because a
    /// position under attack has no quiet reading worth comparing. Two
    /// kibibytes inline, like `killers`. One stack rather than a local per
    /// rule, so that a rule comparing this ply's reading against an
    /// ancestor's ([`improving`]) reads what that ancestor wrote instead of
    /// re-deriving it.
    evals: [Option<Score>; MAX_PLY],
    /// How often the search tried a null move, cut on one, and refused one
    /// for material alone. Written wherever the rule runs and read on no
    /// decision path, so a depth-limited search stays a function of the
    /// code alone; what reads them is `tests/pruning.rs`, whose gates need
    /// to see that the pruning happened, not only that the count moved.
    null_attempts: u64,
    null_cutoffs: u64,
    /// Every other condition admitted the null move and the side to move
    /// had nothing but pawns beside the king. The zugzwang gate asserts
    /// this is the reason a pawn endgame never tried one, rather than the
    /// question never coming up.
    null_refused_material: u64,
    /// How often a late move's first search ran at reduced depth, and how
    /// often that reduced search beat alpha and was re-run at full depth
    /// before being believed. The first is how a gate sees that later
    /// moves were searched shallower, the second that a reduced fail-high
    /// was verified rather than trusted.
    lmr_reductions: u64,
    lmr_researches: u64,
    /// How often a history score shortened a reduction the index had
    /// already decided on, and how often it lengthened one. Two rather
    /// than one because the malus is the half that produces a negative
    /// score, so a gate that sees only the first cannot tell a table that
    /// credits from a table that also debits.
    history_reduced_less: u64,
    history_reduced_more: u64,
    /// How often the margin admitted a node, how many quiet moves it
    /// skipped there, and how often it would have skipped one and did not
    /// because the move gives check.
    ///
    /// The third is the shape [`Search::null_refused_by_material`] has: a
    /// gate asserting that an exempt move was searched needs to see the
    /// exemption *decide*, not merely see that no counterexample turned
    /// up, and a rule that never met a checking move at a futile node
    /// would pass a gate written the other way.
    futility_nodes: u64,
    futility_skipped: u64,
    futility_kept_check: u64,
    /// How often a node was one this rule could act at, how many quiet
    /// moves it gave up there, and how often it would have given one up and
    /// did not because the move gives check.
    ///
    /// **The first two are not comparable with the margin's, and the reason
    /// is the order in the loop.** The margin is asked first and keeps the
    /// moves it was already taking, so these count what this rule *adds*.
    /// Over the bench positions the two populations overlap by 43%, so a
    /// reader adding the two skip counters is counting most of that overlap
    /// once and none of it twice, which is right, and comparing them as
    /// shares of one population is not.
    ///
    /// The third has [`Search::null_refused_by_material`]'s shape, for
    /// [`Search::futility_kept_check`]'s reason: a gate asserting that an
    /// exempt move was searched has to see the exemption decide.
    lmp_nodes: u64,
    lmp_skipped: u64,
    lmp_kept_check: u64,
    /// How often the margin returned a node without searching it, and how
    /// often it would have and did not because the node had the full
    /// window.
    ///
    /// The second is the shape [`Search::null_refused_by_material`] has and
    /// it is here for the same reason: a gate asserting that a full-window
    /// node was searched needs to see the refusal *decide*, and a tree in
    /// which no full-window node ever cleared the margin would pass a gate
    /// written the other way.
    reverse_futility_cutoffs: u64,
    reverse_futility_refused_window: u64,
    /// Elapsed milliseconds at the end of each completed iteration, in
    /// order, and empty where there is no budget.
    ///
    /// Written from the reading the soft-budget test already takes, so it
    /// adds no clock read anywhere, and under a depth or node limit it adds
    /// no entry either: `bench` leaves this empty and reads no clock, which
    /// is the contract `tests/time.rs` pins.
    iterations: Vec<u64>,
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
            history: History::new(),
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
            iterations: Vec::new(),
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
        self.history.clear();
        self.evals = [None; MAX_PLY];
        self.null_attempts = 0;
        self.null_cutoffs = 0;
        self.null_refused_material = 0;
        self.lmr_reductions = 0;
        self.lmr_researches = 0;
        self.history_reduced_less = 0;
        self.history_reduced_more = 0;
        self.futility_nodes = 0;
        self.futility_skipped = 0;
        self.futility_kept_check = 0;
        self.lmp_nodes = 0;
        self.lmp_skipped = 0;
        self.lmp_kept_check = 0;
        self.reverse_futility_cutoffs = 0;
        self.reverse_futility_refused_window = 0;
        self.iterations.clear();
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
            self.root_depth = depth;
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
            if let Some(b) = self.budget {
                let elapsed = self.elapsed_ms();
                self.iterations.push(elapsed);
                // Two reasons not to start another, and they are different
                // reasons: the soft budget says this move has had its
                // share, and the prediction says the next iteration would
                // be abandoned unfinished at the hard budget and buy
                // nothing.
                if elapsed >= b.soft || !time::another_iteration_fits(&self.iterations, b) {
                    break;
                }
            }
        }
        self.wait_if_infinite();
        self.best
    }

    /// The root: every move, the first in the full window and the rest in
    /// a null one, the best move and its score.
    fn search_root(&mut self, board: &mut Board, moves: &[Move], depth: u32) -> (Move, Score) {
        self.nodes += 1;
        self.table.clear(0);
        let mut alpha = -INFINITE;
        let beta = INFINITE;
        let mut best = moves[0];
        let mut best_score = -INFINITE;
        for (i, &m) in moves.iter().enumerate() {
            board.make_move(m);
            // A root move that gives check is extended like any other: the
            // rule is about the move, and the root has no claim on being
            // the exception.
            let ext = extension(board.in_check(), 1, depth);
            let child = depth - 1 + ext;
            // The same rule as the interior nodes, and the root is where
            // it is most nearly free: the move in hand is the one the last
            // iteration chose, and a root move that beats it is what an
            // iteration is looking for rather than what it expects. `beta`
            // here is `INFINITE`, so the second condition on the re-search
            // is true whenever the first is; it is written out because the
            // rule is one rule.
            let mut score = if i == 0 {
                -self.negamax(board, child, 1, -beta, -alpha)
            } else {
                -self.negamax(board, child, 1, -alpha - 1, -alpha)
            };
            if i > 0 && !self.aborted && score > alpha && score < beta {
                score = -self.negamax(board, child, 1, -beta, -alpha);
            }
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

    /// One interior node of the main search, at the `ply` and `depth`
    /// given, searched with the full window.
    #[must_use]
    pub fn node(&mut self, board: &mut Board, depth: u32, ply: usize) -> Score {
        self.node_window(board, depth, ply, -INFINITE, INFINITE)
    }

    /// The same node, searched with the window given instead of the full
    /// one.
    ///
    /// The seam a null window is reached through. What wants it is the
    /// gate on the property the search's own windows rest on: a search of
    /// a window that brackets the value agrees with a full-window search
    /// about that value, and answers a question the full window did not
    /// have to ask. Driving that through [`Search::run`] would test the
    /// windows the search chooses rather than the arithmetic they are
    /// chosen by, which is the distinction [`Search::node`] above exists
    /// for.
    #[must_use]
    pub fn node_window(
        &mut self,
        board: &mut Board,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Score {
        // As though `depth` were the iteration's, so that the extension's
        // ply cap means here what it means in a search rather than reading
        // whatever the last iteration left behind.
        self.root_depth = depth;
        self.negamax(board, depth, ply, alpha, beta)
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

        // The ply bound, above the sort because that is where the first
        // read past the arrays would be. In release a read past their end is
        // a bounds check rather than the assertion a debug build gets.
        if ply >= MAX_PLY {
            return eval::evaluate(board);
        }

        // The table, before the moves are generated: that saving is most of
        // what it is for.
        let key = board.key();
        let (tt_move, cutoff) = self.probe(board, key, depth, ply, alpha, beta);
        if let Some(score) = cutoff {
            return score;
        }

        // The static evaluation, written whether or not anything below
        // reads it at this node: the stack is how a rule at ply `p`
        // compares its own reading against the one at `p - 2`, so a hole
        // here is a wrong answer there, not a saving. A node in check
        // writes `None` rather than a number, because what the evaluation
        // measures is a position nobody is about to win material in, and a
        // check is exactly that claim being contested.
        let in_check = board.in_check();
        self.evals[ply] = if in_check {
            None
        } else {
            Some(eval::evaluate(board))
        };

        // The margin, above the null move because the node's preamble runs
        // the margin tests there and because below it the rule would be
        // nothing but its own error case: see [`Search::reverse_futility`].
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
        // After the mate check: a mate delivered on the hundredth half-move
        // is a mate, not a draw.
        if board.halfmove_clock() >= 100 {
            return DRAW;
        }
        // The history row is the side to move's, read here and again per
        // move below, so `us` is taken once at the node rather than after a
        // move has changed it.
        let us = board.side_to_move();
        let killers = self.order(board, &mut legal, tt_move, us, ply);

        // The margin, read once with the node's own alpha. The interior
        // node's preamble runs the margin tests beside the static
        // evaluation and above the null move, and this sits below it
        // instead: nothing between the two writes what the test reads, and
        // this rule returns no score of its own, so the sequence is
        // unaffected and what the placement saves is the test at every
        // node the null move cuts.
        let futile = futile_node(self.evals[ply], depth, alpha);
        self.futility_nodes += u64::from(futile);

        // The count, read once against the list this node actually holds,
        // because the rule is off at a node the count already admits
        // whole. It sits beside the margin and not above it: the two act on
        // one population and overlap over 43% of it, and whichever is asked
        // first keeps the moves it takes. The margin is asked first so its
        // own counters keep counting the population they were calibrated
        // against, and this rule's report what it adds.
        let give_up = lmp_index(in_check, depth, legal.len());
        self.lmp_nodes += u64::from(give_up.is_some());

        let original_alpha = alpha;
        let mut best = -INFINITE;
        let mut best_move = Move::NULL;
        for (i, m) in legal.iter().enumerate() {
            if self.futile(board, futile, m, i) {
                continue;
            }
            // Before the move is made, which is the whole of what the rule
            // buys: a move given up here costs the node its exemption tests
            // and nothing else. It is also above the reduction rather than
            // beside it, and the two are not alternatives at a node where
            // both would fire -- the move is gone and [`reduction`] never
            // sees it.
            if self.given_up(board, give_up, m, killers, i) {
                continue;
            }
            board.make_move(m);
            // The check extension, on the child's own state: `make_move`
            // has just recomputed the checkers, so asking costs nothing
            // that was not already spent. The same read is the reduction's
            // check exemption below, so it is taken once and named.
            let gives_check = board.in_check();
            let ext = extension(gives_check, ply + 1, self.root_depth);
            let child = depth - 1 + ext;
            // The first move gets the window this node was given and
            // every move behind it the narrower question. The module doc
            // has the window, what it buys and what it costs.
            //
            // Late move reductions sit inside that cheaper question:
            // [`reduction`] holds the exemptions, [`lmr_reduction`] the
            // size, and [`Search::late_move`] the reduced search and the
            // full-depth verification of anything it concluded above
            // alpha.
            let mut score = if i == 0 {
                -self.negamax(board, child, ply + 1, -beta, -alpha)
            } else {
                // The index decides whether this move is reduced at all
                // and the score by how much; [`history_reduction`] has why
                // those are not the same reading.
                let base = reduction(in_check, gives_check, m, killers, depth, i);
                self.late_move(board, child, base, self.history.get(us, m), ply, alpha)
            };
            // No re-search at or above beta, which is already the bound
            // this node returns, and none at a node whose own window is a
            // null one, where there is no room for a score to ask for one.
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
                        // The moves it beat are the ones before it in the
                        // sorted list, which the list already holds, so
                        // nothing has to be remembered as it runs.
                        //
                        // **Some of them were never searched**, and that is
                        // stated rather than left for a reader to find: the
                        // margin above and this rule both skip moves inside
                        // this slice, so a quiet cutoff debits moves that
                        // failed to cut and moves that were given up alike.
                        // Over the bench positions that is 31% of the
                        // debits against the margin's 2% before this rule.
                        // It is left as it is because separating the two
                        // changes what the table holds, which is a change
                        // with its own test; what a reader would otherwise
                        // get wrong is that the slice is the moves the node
                        // tried, and it is the moves the node passed.
                        self.remember_history(us, &legal.as_slice()[..i], m, depth);
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

    /// Put the node's move list in the order it will be searched, and hand
    /// back the killers the caller needs again below.
    ///
    /// Three stages and one sort. The table's move first, refused here
    /// rather than played if the list does not hold it; then the noisy
    /// moves by MVV-LVA; then this ply's killers, and behind them the
    /// remaining quiet moves by history score. The sort starts one move in
    /// when the rotation happened, so that the table's move keeps its place
    /// whatever it ranks. A killer is a move that cut at a sibling and may
    /// not be legal here, which needs no check: it matches nothing in the
    /// list.
    ///
    /// A method rather than four lines in [`Search::negamax`] for the
    /// reason [`Search::probe`] is one, which is the line-count gate and
    /// not a claim that the work belongs apart.
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

    /// The transposition table at an interior node: the move a hit named,
    /// and the score to return where the stored bound answers this node's
    /// question outright. The move comes back whatever the depth says, which
    /// is why the two halves come back separately.
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

    /// The null-window search of one move behind a node's first, reduced by
    /// as many plies as [`reduction`] and [`history_reduction`] allow. A
    /// reduced search that beats alpha is re-run at the full child depth
    /// before its answer is believed.
    fn late_move(
        &mut self,
        board: &mut Board,
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

    /// Whether the move at `index` of a node the margin admitted is skipped
    /// without being searched. `gives_check` is asked last, because it is
    /// the only expensive question here and only a move that would otherwise
    /// be skipped has to answer it.
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

    /// Whether the move at `index` of a node [`lmp_index`] admitted is
    /// given up without being searched. `gives_check` is asked last, for
    /// [`Search::futile`]'s reason, and it is the whole cost of the rule at
    /// a move it does give up.
    ///
    /// **Nothing verifies this the way a reduction is verified**, which is
    /// what the exemptions at [`lmp_skips`] and [`lmp_index`] are sized
    /// against: a reduced search that beats alpha is re-run at full depth,
    /// and a move that is never searched produces no evidence the search
    /// could act on if giving it up was wrong.
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

    /// Record what this node's cutoff says about its quiet moves: credit
    /// `cut`, and debit every quiet move tried ahead of it at this node.
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
    /// [`reverse_futility_margin`] above beta, the node is returned at the
    /// bound its own arithmetic established, without generating a move.
    /// `Some` is that bound; `None` means search the node.
    ///
    /// [`reverse_futile`] holds the arithmetic and the refusals that follow
    /// from it. Two more are here, because both read the board or the window
    /// rather than the margin: **a full-window node is refused**, because
    /// what this returns is a bound and no move and no line; and **a
    /// halfmove clock at the limit is refused**, where the node is a draw by
    /// rule and `negamax` has not run its draw check yet.
    ///
    /// **The zugzwang guard is deliberately not taken**, and that is where
    /// this rule and the null move part company: this one collects no
    /// evidence by passing, so the guard would be guarding against nothing.
    /// `tests/reverse_futility.rs` gates the difference as a difference, so
    /// adding it for symmetry fails a test rather than passing quietly.
    ///
    /// **A stalemated side is not in check, so it has an evaluation**, and a
    /// node that clears beta on it is returned rather than scored as the
    /// draw it is. That exposure is the null move's too and is accepted on
    /// the same ground: the move list is what would settle it, and
    /// generating one is the whole of what both rules save.
    ///
    /// Nothing is stored in the table: no search happened, so the entry
    /// would offer a later probe a cutoff at a depth nothing ever paid for.
    fn reverse_futility(
        &mut self,
        board: &Board,
        depth: u32,
        ply: usize,
        alpha: Score,
        beta: Score,
    ) -> Option<Score> {
        let bound = reverse_futile(self.evals[ply], depth, beta)?;
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

    /// Null-move pruning at one node: `Some` is the cutoff, `None` means
    /// search the node. Refused in check, at a full window, on a mate-scale
    /// beta, below beta, at a position the null move itself reached, on a
    /// halfmove clock at the limit, and where the side to move has nothing
    /// but pawns beside the king ([`has_non_pawn_material`]). A mate-scale
    /// result comes back as `beta`, and nothing is stored: the entry would
    /// carry a reduced depth as if it were `depth`.
    fn null_move(
        &mut self,
        board: &mut Board,
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

    /// The quiescence search. Out of check the side to move may stand pat,
    /// then every noisy move is tried most valuable victim first, except
    /// those whose static exchange loses material. In check there is no
    /// standing pat and every evasion is tried, the quiet and the losing
    /// ones included. It ends where the noisy moves run out or at `MAX_PLY`,
    /// where the state stack ends. Fail-soft; it writes no principal
    /// variation, so the pv ends at the horizon.
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
            // No killers and no history: whether either ranks the quiet
            // evasions usefully is unmeasured, and a second change.
            picker::sort_from(board, &mut evasions, 0, [Move::NULL; 2], &[]);
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

    /// How many null moves the last search tried.
    #[must_use]
    pub fn null_attempts(&self) -> u64 {
        self.null_attempts
    }

    /// How many of those produced a cutoff.
    #[must_use]
    pub fn null_cutoffs(&self) -> u64 {
        self.null_cutoffs
    }

    /// How often every other condition admitted a null move and the side
    /// to move had nothing but pawns beside the king.
    #[must_use]
    pub fn null_refused_by_material(&self) -> u64 {
        self.null_refused_material
    }

    /// How many late moves the last search first searched at reduced
    /// depth.
    #[must_use]
    pub fn lmr_reductions(&self) -> u64 {
        self.lmr_reductions
    }

    /// How many of those reduced searches beat alpha and were re-run at
    /// full depth.
    #[must_use]
    pub fn lmr_researches(&self) -> u64 {
        self.lmr_researches
    }

    /// How often a history score shortened a reduction the index had
    /// decided on, and how often it lengthened one.
    #[must_use]
    pub fn history_reduced_less(&self) -> u64 {
        self.history_reduced_less
    }

    #[must_use]
    pub fn history_reduced_more(&self) -> u64 {
        self.history_reduced_more
    }

    /// How many nodes the margin admitted, how many quiet moves it skipped
    /// there, and how many it would have skipped and did not because the
    /// move gives check.
    #[must_use]
    pub fn futility_nodes(&self) -> u64 {
        self.futility_nodes
    }

    #[must_use]
    pub fn futility_skipped(&self) -> u64 {
        self.futility_skipped
    }

    #[must_use]
    pub fn futility_kept_check(&self) -> u64 {
        self.futility_kept_check
    }

    /// How many nodes the margin returned without searching, and how many
    /// it would have returned and did not because the node had the full
    /// window.
    /// How often a node admitted this rule, how many quiet moves it gave up
    /// there, and how often a move that would have been given up was kept
    /// for giving check.
    #[must_use]
    pub fn lmp_nodes(&self) -> u64 {
        self.lmp_nodes
    }

    #[must_use]
    pub fn lmp_skipped(&self) -> u64 {
        self.lmp_skipped
    }

    #[must_use]
    pub fn lmp_kept_check(&self) -> u64 {
        self.lmp_kept_check
    }

    #[must_use]
    pub fn reverse_futility_cutoffs(&self) -> u64 {
        self.reverse_futility_cutoffs
    }

    #[must_use]
    pub fn reverse_futility_refused_by_window(&self) -> u64 {
        self.reverse_futility_refused_window
    }

    /// The table the last search left behind, for a gate that wants to see
    /// what the cutoffs wrote and what the ordering would do with it.
    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The depth of the last completed iteration; zero before any.
    /// Elapsed milliseconds at the end of each completed iteration, in
    /// order. Empty when the search ran without a time budget, which is
    /// what `bench` and every `go depth` run under.
    #[must_use]
    pub fn iterations_ms(&self) -> &[u64] {
        &self.iterations
    }

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
