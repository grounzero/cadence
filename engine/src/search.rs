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
//! the previous iteration's best move searched first at the root; the
//! first move at a node searched with the window the node was given and
//! every move behind it with a null one; a
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
//! of `picker::sort_from`. What the bands are, what each is worth and why
//! a history score sits inside a band rather than between two of them is
//! `picker`'s own header. At the root the previous iteration's best move
//! goes first, and that is unchanged. The quiescence search orders its
//! moves through the same sort and has no killers to give it.
//!
//! **The one thing the quiescence search refuses.** Out of check, a noisy
//! move whose static exchange value is negative (`see::see`) is skipped
//! without being searched; in check nothing is skipped, because every
//! entry in that list is an answer to the check. `Search::quiesce` holds
//! the rest of the rule and `see`'s own header holds what the exchange
//! is.
//!
//! **The check extension.** A move that gives check is searched one ply
//! deeper: the child's depth is its parent's rather than one less. The
//! predicate is the child's `Board::in_check`, read after the move is made,
//! because `make_move` recomputes the checkers whatever the caller wants
//! and that read is already paid for. It applies at the root's moves for
//! the same reason it applies at every other node's.
//!
//! **What bounds it.** On a line where every move gives check the depth
//! never falls, so the arithmetic the search used to rest on -- depth is at
//! most the root's and loses one per ply -- stops holding, and a rule
//! conditioned on depth would never stop being true. Four things bound it,
//! and only the third is this change's own. A repetition is a draw wherever
//! it is met, so a true perpetual is scored at the second visit rather than
//! searched. The fifty-move rule ends any check sequence that neither
//! captures nor moves a pawn. **The ply cap** refuses the extension to any
//! child past `EXTEND_WITHIN` times the root depth, which is what holds
//! the tree to a multiple of its nominal depth rather than merely finite.
//! And the ply bound below is what holds the arrays when the cap is above
//! `MAX_PLY`, which it is for a root depth over 85.
//!
//! **The null window.** The first move at a node is searched with the
//! window the node was given. Every move behind it is asked a narrower
//! question first -- not what it is worth, but whether it is worth more
//! than the move already in hand -- which is the window `(alpha, alpha+1)`
//! and holds no room for an answer to the first. Nearly all of them are
//! not, and a move that is not better is refuted inside that window over a
//! smaller tree than the full one would take to refute it. A move that is
//! better fails high in it, and what comes back then is a bound and not a
//! value, so that move is searched again with the full window to find out
//! what it is worth. Nothing is discarded on the window's account and
//! nothing is trusted that was not searched. **Two searches of one
//! position agree only where their windows do**, because null-move pruning
//! below reads beta, so what a node returns depends on the window it was
//! asked in. The machinery stands anyway, because every score that comes
//! back is a fail-soft bound and every consumer treats it as one; what a
//! window-dependent return costs is the guarantee that a re-search walks
//! to the value the narrow window found, and the window gates in
//! `tests/search.rs` are scoped around exactly that.
//! Two conditions the re-search is not run under: a score at or above beta
//! is already the bound this node returns, and at a node whose own window
//! is a null one there is no room between alpha and beta for a score to
//! ask for one, so the re-searches happen along the principal variation
//! rather than through the tree hanging off it.
//!
//! What it is paid for is the ordering above it. When the first move
//! searched is the best one, every move behind it is refuted cheaply; when
//! it is not, the re-search is work the full window would have done once.
//! The root is where the first move is likeliest to be best, because it is
//! the move the previous iteration chose, and the root window is the full
//! one and stays that way: narrowing it is a separate change against a
//! different mechanism, and both narrow the window a search runs in, so
//! whichever lands first takes the overlap and the second is measured
//! against a search that already has it. Revisit this paragraph when
//! something narrows the root window.
//!
//! **The ply bound.** Both searches stop at `MAX_PLY` whatever depth is
//! left and answer with the static evaluation: the per-ply arrays end
//! there, and in release a read past their end is a bounds check rather
//! than the assertion a debug build gets. It landed before anything
//! extended, when no search could reach it, and the extension above is what
//! makes it reachable: it is the floor the cap does not stand on.
//!
//! **Null-move pruning.** At an interior node of the main search, out of
//! check and inside a null window, where the static evaluation already
//! stands at or above beta, the side to move hands the opponent the move
//! and a reduced search asks whether it still stands there
//! ([`null_reduction`] plies past the one the move would have cost). If it
//! does, the node is cut without its move list being generated. Every
//! condition that refuses it is named at the rule in `negamax` with the
//! wrong answer it exists to avoid; the one that is a chess claim rather
//! than a search claim is the zugzwang guard, [`has_non_pawn_material`].
//!
//! **Late move reductions.** A quiet move far down the sorted list of an
//! interior node is first searched shallower than its siblings, by
//! [`lmr_reduction`] plies, inside the null window the move behind the
//! first is asked anyway; a reduced search that beats alpha is re-run at
//! the full child depth before its answer is believed, and only then may
//! the full-window re-search above follow. The size of the reduction is at
//! [`lmr_reduction`] and the exemptions at [`reduction`], each with the
//! wrong answer it exists to avoid. The null-move rule above and this one
//! compose vertically, never on the same search: that one cuts a whole
//! node before its move list exists, this one shortens one real move's
//! subtree, and a reduced child may itself null -- its verification then
//! reads the already-reduced depth, which is fine because every conclusion
//! above alpha climbs back through the re-search ladder at full depth
//! before anything trusts it.
//!
//! **The history heuristic.** A beta cutoff by a quiet move credits that
//! move in a butterfly table ([`history`]) and debits every quiet move
//! tried ahead of it at the same node. Two things read it, the sort and a
//! late move's reduction ([`history_reduction`]), and `history`'s own
//! header is why those are two uses of one number rather than two
//! mechanisms. The table lives for one search and is cleared where the
//! killers are.
//!
//! **Futility pruning.** Near the horizon, a quiet move is skipped
//! without being searched where the node's static evaluation plus a
//! margin ([`futility_margin`], a pawn and a half per ply of remaining
//! depth, up to [`FUTILITY_DEPTH`]) still does not reach alpha: the move
//! changes no material and the search left below it is too short to build
//! a threat worth the gap. The node-level half is [`futile_node`], read
//! once with the node's own alpha; the per-move half is
//! [`futility_skips`], which holds the exemptions and the one question
//! asked last because it is the only expensive one
//! ([`Board::gives_check`]). Being in check needs no exemption:
//! `evals[ply]` is `None` there, so the rule has nothing to read. It
//! composes with the reductions the way the null move does -- a move this
//! skips is a move that never reaches the reduction, so the two share the
//! same pool of late quiet moves and the second gets what the first
//! leaves.
//!
//! **Reverse futility.** The same margin read from the other side of the
//! window, and the one rule here that returns a node rather than a move.
//! At an interior node out of check, inside a null window, where the
//! static evaluation stands [`reverse_futility_margin`] *above* beta, the
//! node answers with `eval - margin` and generates no moves at all: a side
//! that far ahead is not going to fail low, and the margin is what the
//! search left below it would have to take away for that to be wrong. It
//! and futility pruning cannot both fire, and nothing arranges that.
//! [`reverse_futile`] holds the arithmetic, that argument, and the
//! measurement behind the one place this rule departs from every
//! formulation of it elsewhere: it carries no depth limit, because the
//! margin already is one. [`Search::reverse_futility`] is the half that
//! reads the board and the window, and holds why it sits above the null
//! move.
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
//! **Stopping.** The stop flag, a node limit and the hard time budget end
//! the search mid-iteration, at any node from the first; the iteration's
//! partial result is discarded and the last completed one stands. Stopped
//! inside its first iteration, the search returns the best root move it
//! has fully searched -- the first root move, if not even one -- so there
//! is always a move. The soft budget only prevents starting another
//! iteration, and so does a prediction that the next one cannot finish
//! inside the hard budget ([`crate::time::another_iteration_fits`]). Under
//! `infinite` the search returns only when `stop` is raised, as the
//! protocol requires.

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

/// How many times the root depth a check extension is granted within.
///
/// Two. One is the variant this change named and did not take: it would
/// hold the deepest interior node to twice the root depth rather than
/// three times it, and it is a cheaper search and a separately testable
/// one, not a tidier version of this. Deciding between them is deciding
/// what the extension is worth, which is what its match test measures, so
/// the alternative is named here and left where a later test can reach it.
///
/// **The bench cannot choose between the two, and that is measured rather
/// than assumed.** Built at each cap, over the same list, the counts are
/// within 1.6% of each other at every depth tried -- 1.3% at five, 0.8% at
/// six, 0.9% at seven, 1.6% at nine, each read as the ratio against the
/// same list before any extension. At these depths the cap almost never
/// binds: what ends an extended line is depth running out, not ply. So the
/// cap is doing the job the module doc gives it, which is bounding the
/// pathological line, and it is not shaping the ordinary tree -- this is
/// not a tuning decision wearing a safety decision's clothes, and a node
/// count is the wrong instrument for it either way.
const EXTEND_WITHIN: usize = 2;

/// How much deeper the child of `m` is searched, in plies: one when the
/// move gave check, none otherwise.
///
/// `check` is the child's `Board::in_check`, read after the move is made.
/// `make_move` recomputes the checkers whether or not anyone asks, so that
/// read is already paid for. `Board::gives_check` answers the same question
/// before the move is made, at two slider lookups, and what wants it is a
/// decision taken instead of making the move rather than after it, so it
/// goes on waiting for its first caller.
///
/// **The cap is on ply, and a cap on depth would not be a cap.** Where
/// every move gives check the depth never falls, so a rule of the form
/// "extend while at least so much depth is left" stays true for as long as
/// the checks do, and what would stop the recursion is the fifty-move rule
/// and a repetition, hundreds of plies down. A ply cap stops it where it is
/// asked to. `ply` is the child's, and past `EXTEND_WITHIN` times the root
/// depth no child is granted one, so from there depth falls by one per ply
/// and the deepest interior node an iteration can produce is at ply
/// `(EXTEND_WITHIN + 1) * root_depth - 2`.
///
/// **That bounds the tree and not the arrays.** It is above `MAX_PLY` for
/// a root depth over 85, and what holds there is the ply bound in
/// [`Search::negamax`], which is why that bound was established before
/// anything extended rather than beside it.
///
/// A free function for the same reason [`order_first`] is: the thing worth
/// gating is a total function of three arguments, and a gate that can call
/// it directly does not have to find a position that reaches every case.
#[must_use]
pub fn extension(check: bool, ply: usize, root_depth: u32) -> u32 {
    u32::from(check && ply < EXTEND_WITHIN * root_depth as usize)
}

/// How many plies past the one the move would have cost a null-move
/// verification is shortened by: the reduced search runs at
/// `depth - 1 - null_reduction(depth)`, floored at zero, where the floor
/// hands the question to the quiescence search.
///
/// Three plus a third of the depth, and the depth term is why. The
/// verification is not trying to value the position, it is asking whether
/// a side that stands above beta *after giving up the move* can be pushed
/// back under it, and the deeper the node the more depth the answer keeps
/// even after a large reduction. A constant sized for the shallow nodes
/// wastes most of the saving at the deep ones, which is where the subtrees
/// are. Scaling by how far the evaluation clears beta is a separate
/// mechanism with its own test, refused here, not folded in.
#[must_use]
pub fn null_reduction(depth: u32) -> u32 {
    3 + depth / 3
}

/// How many plies a late move's first search is shortened by: zero for the
/// first three moves of a node, zero below depth three, and otherwise one
/// plus a quarter of the product of the two integer logarithms. The caller
/// holds the exemptions (what is never reduced is its decision, not this
/// function's); this is only the size of the reduction where one applies.
///
/// Logarithmic in both arguments, because that is the shape of the claim
/// being made. The move index measures how far down a sorted list the move
/// sits, and the ordering's confidence falls off multiplicatively rather
/// than linearly: the difference between the fourth move and the eighth is
/// worth about as much as the difference between the eighth and the
/// sixteenth. The depth measures how much tree hangs below the node, which
/// grows exponentially, so equal plies of reduction buy exponentially more
/// saving at equal risk. `ilog2` is the integer logarithm the determinism
/// contract allows; the floats the textbook formula uses have no place on
/// a decision path.
///
/// The table this produces, reduction by depth band and move index band:
///
/// | depth \ index | 3 | 4-7 | 8-15 | 16-31 | 32+ |
/// |---|---|---|---|---|---|
/// | 3 | 1 | 1 | 1 | 2 | 2 |
/// | 4-7 | 1 | 2 | 2 | 3 | 3 |
/// | 8-15 | 1 | 2 | 3 | 4 | 4 |
/// | 16-31 | 2 | 3 | 4 | 5 | 6 |
///
/// Below depth three the child search is at most one ply from the
/// quiescence search already, so there is nothing left to shorten that a
/// reduction would not hand straight to it. Below index three the moves
/// are the ones the ordering placed deliberately, and three is cheap
/// insurance besides: a list short enough that its tail starts earlier is
/// a list too small for reductions to pay on.
#[must_use]
pub fn lmr_reduction(depth: u32, index: usize) -> u32 {
    if depth < 3 || index < 3 {
        return 0;
    }
    1 + depth.ilog2() * index.ilog2() / 4
}

/// How many plies the first search of move `m`, at `index` in its node's
/// sorted list, is reduced by: [`lmr_reduction`]'s size where a reduction
/// applies, and zero for every move on the exempt list. Each exemption is
/// a wrong answer somewhere: a move while the node is in check (`in_check`
/// -- every move answers the check and the line is forcing); a move that
/// gives check (`gives_check` -- forcing whether or not the extension's
/// ply cap still grants the ply back, and otherwise the extension deepens
/// what this would undo); a noisy move, whose point is a tactic a
/// shallower search is built to miss; and a killer, the one class of
/// quiet move with evidence behind its placement. What is left is the
/// quiet moves the sort ranked behind everything with no evidence to
/// their name, which is what makes them safe to reduce: the ones the
/// ordering misjudged fail high in the reduced search and are re-searched
/// at full depth before anything believes them.
///
/// A free function for the same reason [`extension`] is: a total function
/// of its arguments, so a gate can pin every exemption without finding a
/// position that reaches it.
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
/// reduced by, once its history score is read: [`history::shift`] plies
/// fewer for a move the table thinks well of, and that many more for one it
/// has refuted.
///
/// **It adjusts a reduction and never creates one.** A base of zero comes
/// back zero, so every exemption [`reduction`] holds -- the first three
/// moves, a node in check, a noisy move, a killer, a move that gives check
/// -- survives whatever the table says, and so does the depth threshold. A
/// history score is evidence about a move across the whole search; an
/// exemption is a claim about this node, and this node is where the wrong
/// answer would be.
///
/// **What it adds that the index cannot supply.** The index is a rank
/// inside one node's list. A rank cannot tell a node whose twentieth quiet
/// move is still decent from a node whose fourth is worthless, because both
/// lists are ranked from one to however many, and after the ordering above
/// reads the same table the rank is *derived* from the score and loses
/// exactly the part that is absolute. This reads the score itself.
#[must_use]
pub fn history_reduction(base: u32, history: i32) -> u32 {
    if base == 0 {
        return 0;
    }
    base.saturating_add_signed(-history::shift(history))
}

/// Whether the side to move has any piece beside its pawns and king.
///
/// The zugzwang guard. The null move reads "even giving up the move, I
/// stand above beta" as strength, and in a pawn-and-king position that
/// reading inverts: the obligation to move is often the losing condition,
/// so passing is exactly the move the side wants and cannot have, and a
/// cutoff taken on it prunes the one class of position where the
/// conclusion is systematically wrong. With a piece on the board a waiting
/// move nearly always exists and the reading holds. Zugzwang with pieces
/// exists and is not checked for; what makes that survivable is that it is
/// rare, that the damage is one wrong bound at a node the evaluation
/// already called winning, and that nothing is stored in the table on the
/// way out.
///
/// The side checked is the side to move, and only that side: the guard is
/// about who is asking to pass, not about what is left on the board.
#[must_use]
pub fn has_non_pawn_material(board: &Board) -> bool {
    let us = board.side_to_move();
    (board.by_colour(us) & !(board.by_type(PieceType::Pawn) | board.by_type(PieceType::King))).any()
}

/// Whether the side to move's position is improving: the static evaluation
/// written at `ply` exceeds the one written two plies above it, where both
/// exist. `false` wherever either reading is missing -- the ply is inside
/// the first two, or either node was in check -- because a rule reading
/// this flag wants "known to be getting better", and an unknown is not
/// that.
///
/// Read by nothing yet, deliberately: the rules that consult it each carry
/// their own test, and a flag folded into the first of them would be two
/// changes in one. It exists now because the evaluation stack it reads
/// exists now, and the definition is settled once rather than per reader.
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

/// The deepest node at which a quiet move may be skipped for the margin.
///
/// **Three, chosen where the saving stops paying for the risk, and both
/// halves of that are measured.** The margin is a claim that the remaining
/// search cannot recover the gap, and the deeper the node the less the
/// claim is worth: at depth three the child still has two plies of real
/// search under it, which is enough to play a quiet move, be answered, and
/// cash a tactic. What the depths past it buy, on the bench positions at
/// depth eleven against 32,872,578: one 30,201,879, two 25,691,630, three
/// 24,519,578, four 23,660,924, five 23,333,146. In doublings saved that
/// is 0.122, 0.356, 0.423, 0.474, 0.494, so the third ply is worth 0.067
/// and the fourth 0.051 and the fifth 0.020. **The curve flattens where
/// the claim gets weaker, which is the argument**: a fourth ply would add
/// about a tenth of the saving already taken and would stake it on a
/// margin covering three plies of real search rather than two.
///
/// **Below the limit the rule is off, not weaker.** A node outside the
/// band searches every move it generates, so the exemptions below are the
/// only thing that has to be right about the nodes inside it.
///
/// The band being a constant is also why the fixed-depth node ratio this
/// change produces is the same at every depth, where a rule that fires at
/// every node compounds: the tree this removes sits in the last three
/// plies, and the share of a tree that sits in its last three plies does
/// not grow with the root depth.
const FUTILITY_DEPTH: u32 = 3;

/// What the margin grows by per ply of remaining depth, in centipawns.
///
/// A hundred and fifty, which is a pawn and a half on this evaluation's
/// own scale, where a pawn is a hundred.
///
/// **Measured against the distribution the rule reads, over the bench
/// positions at depth nine.** At the sites the rule can act on -- a quiet
/// move behind a node's first, out of check, not giving check -- the gap
/// between alpha and the static evaluation has a median of 300 at depths
/// one to three, with quartiles at 0 and between 700 and 800, and **about
/// a fifth of all sites sit within fifty centipawns of zero**, which is
/// where the evaluation is saying nothing at all. The margin has to clear
/// that mass and this one does; past it the distribution is flat, so at
/// depth one a margin of 100 skips 61% of the sites, 300 skips 48% and 600
/// skips 28%.
///
/// **What the choice is worth on the tree**, same positions at depth
/// eleven against 32,872,578: a margin of 100 leaves 21,521,020, 150
/// leaves 24,519,578, 200 leaves 26,190,374 and 300 leaves 27,881,084.
/// The lever is much sharper than the depth limit above, which is the
/// reason to set it against the distribution rather than against the
/// saving.
///
/// **Why not a pawn, which the saving argues for.** This evaluation is
/// material and piece-square tables and is the seed for the first trained
/// net rather than a serious reading, so a gap it reports is worth less
/// than the same gap from a strong evaluation, and the margin is what
/// prices that. Half a pawn of slack per ply is the price paid for it, and
/// it is deliberately paid: the constant is the first thing to hand a
/// parameter tune once there is an evaluation worth trusting.
///
/// **What the slope is for.** A quiet move at depth one is answered by the
/// quiescence search, which resolves captures and nothing else, so what it
/// can swing is the move's own placement plus one exchange. At depth three
/// there are two plies of real search below it and a piece can be won, so
/// the margin has to grow with the depth or the deepest band of the rule
/// is the reckless one. Linear rather than squared because the evidence is
/// linear: the material a search can win grows with the moves it has, not
/// with their square. A constant term beside the slope is the obvious
/// second parameter and is left out on purpose, so that this rule arrives
/// with one number to tune rather than two.
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
/// evaluation plus [`futility_margin`] still does not reach `alpha`.
///
/// **The in-check exemption is the missing evaluation and not a condition.**
/// `evals[ply]` is `None` wherever the side to move is in check, because a
/// position under attack has no quiet reading worth comparing, and a rule
/// that cannot read the evaluation cannot claim anything about it. So the
/// exemption that would otherwise have to be written and remembered falls
/// out of the shape of the data.
///
/// **Alpha on the mate scale refuses it**, which is the same refusal the
/// null move takes on beta and for the same reason: a mate score is not a
/// quantity a margin is commensurable with, and an alpha that already
/// names a mate would make every quiet move futile at a node that may hold
/// a shorter one.
///
/// It reads the node's own alpha, once, before the move loop. Alpha only
/// rises inside the loop, so a node the test admitted stays admitted, and
/// one it refused is never re-admitted by a move beating alpha -- which is
/// the conservative direction: a node whose alpha rises is a node that is
/// no longer failing low.
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
/// candidate to be skipped without being searched, on everything that can
/// be decided from the move and its place in the list.
///
/// Two exemptions, and neither is about the ordering:
///
/// - **A noisy move**, whose whole point is the material it changes. The
///   margin is a claim about what a *quiet* move can do to the score, and a
///   capture answers it by taking a queen.
/// - **The node's first move**, so that a node that skips everything else
///   still returns a score something searched and a move to store. It is
///   not the reduction's index threshold arriving again: that one is a
///   claim about how far the sort can be trusted, and this one is a claim
///   about the node having an answer at all.
///
/// **A killer is not exempt, deliberately.** [`reduction`] exempts one
/// because a reduction is a bet on the sort and a killer is the one quiet
/// move with evidence behind its placement. This rule is not a bet on the
/// sort: it says the evaluation is too far below alpha for any quiet move
/// to matter, which is a claim about the node and not about the move's
/// rank. A move that cut at a sibling is still a quiet move, and a quiet
/// move still cannot make up a minor piece.
///
/// **The check exemption is not here**, because it is the one question that
/// costs something ([`Board::gives_check`], two slider lookups) and only a
/// move this returns `true` for ever has to answer it.
#[must_use]
pub fn futility_skips(futile: bool, m: Move, index: usize) -> bool {
    futile && index > 0 && !m.is_noisy()
}

/// What the margin a node is returned on grows by per ply of remaining
/// depth, in centipawns.
///
/// A hundred and fifty, a pawn and a half on this evaluation's own scale,
/// and the same slope [`FUTILITY_MARGIN`] carries for the same reason: this
/// evaluation is material and piece-square tables and is the seed for the
/// first trained net rather than a serious reading, so a gap it reports is
/// worth less than the same gap from a strong evaluation, and half a pawn
/// of slack per ply is what prices that. It is the first thing to hand a
/// parameter tune once there is an evaluation worth trusting.
///
/// **It is the only thing bounding this rule, so it is chosen where it
/// bounds as well as where it sizes.** There is no depth limit here and
/// [`reverse_futile`] is where that absence is argued and measured.
///
/// **Measured against the distribution the rule reads**, over the bench
/// positions at depth eleven, at every site the rule can look at: an
/// interior node of the main search, out of check, inside a null window.
/// The gap between the static evaluation and beta has a median of 100 at
/// depth one and of zero from depth five, an upper quartile between 100 and
/// 400 at every depth, and **about a third of all sites within fifty
/// centipawns of zero**, where the evaluation is saying nothing at all. The
/// margin has to clear that mass at every depth and this one does. What it
/// then takes is concentrated near the horizon without a limit saying so:
/// of the 1,251,848 sites it admits, **81.5% are at depth one, 92.8% within
/// two plies and 97.6% within three.**
///
/// **What the choice is worth on the tree**, the same positions at depth
/// eleven against 24,519,578: a margin of 75 leaves 11,873,561, 100 leaves
/// 13,961,452, 125 leaves 15,573,296, 150 leaves 16,024,935, 175 leaves
/// 17,402,325 and 200 leaves 17,861,062.
///
/// **Why not a pawn, which the saving argues for, and the instrument that
/// decides it.** [`futility_margin`] had only the distribution to set
/// itself against. This rule has a second reading, because the rule below
/// it in the node's preamble checks the same claim by searching: at a node
/// this returns, the null move would otherwise have handed the opponent the
/// move and asked a reduced search whether the evaluation's reading
/// survives. Measured over the bench positions at depth eleven, of the
/// sites this slope takes, **91.3% are nodes the null move independently
/// cut**; the other 8.7% are nodes whose verification came back refusing,
/// where this rule overrules a search that disagreed with it. That share is
/// 11.4% at a slope of 100 and 12.9% at 50, and each twenty-five centipawns
/// taken off buys a band of sites of which about **one in five** overrules
/// the verification, against about one in eleven across the whole rule. The
/// slope sits at the conservative end of that trade deliberately, and this
/// is the reading to re-take before moving it.
const REVERSE_FUTILITY_MARGIN: Score = 150;

/// How far above `beta` a node's static evaluation must stand before the
/// node is returned without being searched: [`REVERSE_FUTILITY_MARGIN`] per
/// ply of `depth`.
#[must_use]
pub fn reverse_futility_margin(depth: u32) -> Score {
    // Saturating, and total for that reason, like [`futility_margin`]: a
    // function that is only right for the arguments something happens to
    // hand it is one a gate cannot pin.
    REVERSE_FUTILITY_MARGIN.saturating_mul(Score::try_from(depth).unwrap_or(Score::MAX))
}

/// The bound a node may be returned at without being searched at all: its
/// static evaluation less [`reverse_futility_margin`], where that still
/// stands at or above `beta`. `None` is a node that has to be searched.
///
/// The claim is the mirror of [`futile_node`]'s and, like it, it is a claim
/// about the node and not about any move: a side whose static evaluation
/// clears beta by more than the search left below it can plausibly take
/// away is not going to fail low here, so the node answers with the bound
/// its own arithmetic established and never generates a move. **The two
/// members of the family cannot both fire and nothing has to arrange
/// that**: inside a null window beta is `alpha + 1`, so a node this admits
/// has its evaluation above alpha and a node [`futile_node`] admits has it
/// a margin below, which is the other side of the same window.
///
/// **What it returns is the quantity the condition established, not the
/// evaluation.** `eval - margin` is the least this rule can claim: the test
/// is that this number stands at or above beta, so returning it returns
/// exactly what was shown, where returning `eval` would claim back the
/// margin that was there to discount it. Everywhere else in this search
/// fail-soft means the bound follows the value the search found; nothing is
/// searched here, so it follows the condition instead, and it is the
/// smallest bound that satisfies it.
///
/// **There is no depth limit, and the absence is measured rather than
/// assumed.** The rule elsewhere carries one; on this tree it decides
/// almost nothing, because the trigger already falls away with depth on its
/// own. It demands `margin * depth` more of the evaluation at every further
/// ply while the spread of what the evaluation reports does not grow with
/// depth, so the requirement outruns the distribution without being told
/// to. Over the bench positions at depth eleven against a shipped count of
/// 16,024,935: a limit of eight gives the same count to the node, a limit
/// of six gives 16,027,675 (0.017% more), and a limit of three -- tighter
/// than the band the margin already produces -- gives 16,505,225, 3.0%
/// more. **And a limit does less the deeper the search runs, which is the
/// direction that matters**: at bench depth thirteen the whole span from a
/// limit of three to no limit is 0.84% where at eleven it is 3.0%, because
/// a deeper search spends proportionally more of its tree near the horizon,
/// which is where this rule acts.
///
/// What a limit would be for is the belief that the claim gets worse with
/// depth faster than a linear slope prices it. That belief is reasonable
/// and it is an argument for curvature in the margin, not for a cliff in
/// the depth: a linear slope with a cliff on the end is the worse half of
/// each, and nothing measured here places either. So the rule arrives with
/// one number and the number is the bound. Revisit it against a curved
/// margin, not against a limit, and re-take the depth-share reading at
/// [`REVERSE_FUTILITY_MARGIN`] first.
///
/// **In check needs no exemption**, for [`futile_node`]'s reason: `evals`
/// holds `None` wherever the side to move is in check, so a rule that
/// cannot read the evaluation cannot claim anything about it.
///
/// **A beta on the mate scale refuses it**, which is the refusal the null
/// move takes for the same reason: a mate score is not a quantity a
/// centipawn margin is commensurable with, and a reduced claim about a
/// forced mate has proved nothing about one.
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
    ///
    /// Public for the same reason [`order_first`] is a free function: the
    /// thing worth gating is reached directly rather than through a search.
    /// What needs it here is the ply bound in `negamax`. That bound was
    /// unreachable through [`Search::run`] when it landed, because depth
    /// fell by one per ply and the root's was capped at `MAX_PLY`, so a
    /// gate driven through `run` would have passed without reaching the
    /// guard it names. The check extension gives a ply back and makes the
    /// boundary reachable in principle, at a root depth over 85; a gate
    /// through `run` would still not reach it, because no search of that
    /// depth finishes.
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
        // read past the arrays is. `killers` is `MAX_PLY` long and
        // `self.killers[ply]` is an argument to `sort_from` below, taken
        // before any move is made; `make_move`'s push of `states[ply + 1]`
        // is the failure behind it, and a guard placed at the make would
        // look right and never be reached. Both are slice bounds checks,
        // which release keeps and which end the process under
        // `panic = "abort"`.
        //
        // A `debug_assert!(ply < MAX_PLY)` stood here instead, over a
        // comment arguing that depth is at most `MAX_DEPTH` at ply zero and
        // falls by one per ply, so an interior node is always inside the
        // state stack. The argument was sound and the assertion was inert
        // in release, which is where the games are played; and the argument
        // held only while nothing gave a ply back. The check extension
        // below gives one back, so what stands here is a bound and not an
        // assertion, and it was established before the extension rather
        // than beside it.
        //
        // The answer is the one `quiesce` gives at the same boundary: the
        // static evaluation, whether or not the side to move is in check.
        // Handing the node to `quiesce` instead would reach the same value
        // by way of its own bound and count a second node for it.
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
        // The ordering, in three stages and one sort. The table's move
        // first, refused here rather than played if the list does not hold
        // it; then the noisy moves by MVV-LVA; then this ply's killers, and
        // behind them the remaining quiet moves by history score. The sort
        // starts one move in when the rotation happened, so that the
        // table's move keeps its place whatever it ranks. A killer is a
        // move that cut at a sibling and may not be legal here, which needs
        // no check: it matches nothing in the list. The history row is the
        // side to move's, read here and again per move below, so `us` is
        // taken once at the node rather than after a move has changed it.
        let us = board.side_to_move();
        let ordered = usize::from(order_first(&mut legal, tt_move));
        let killers = self.killers[ply];
        picker::sort_from(board, &mut legal, ordered, killers, self.history.side(us));

        // The margin, read once with the node's own alpha. The interior
        // node's preamble runs the margin tests beside the static
        // evaluation and above the null move, and this sits below it
        // instead: nothing between the two writes what the test reads, and
        // this rule returns no score of its own, so the sequence is
        // unaffected and what the placement saves is the test at every
        // node the null move cuts.
        let futile = futile_node(self.evals[ply], depth, alpha);
        self.futility_nodes += u64::from(futile);

        let original_alpha = alpha;
        let mut best = -INFINITE;
        let mut best_move = Move::NULL;
        for (i, m) in legal.iter().enumerate() {
            if self.futile(board, futile, m, i) {
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
            // The first move gets the window this node was given. Every
            // move behind it is asked the cheaper question first: not
            // "what is this worth" but "is it worth more than the move in
            // hand", which is the window `(alpha, alpha + 1)` and holds no
            // room for an answer to the first. A move that is not better
            // fails low inside it and is refuted over a smaller tree than
            // the full window would have taken to refute it; a move that
            // is better fails high, and what comes back is a bound and not
            // a value, so it is searched again with the full window to
            // find out what it is worth.
            //
            // Late move reductions sit inside that cheaper question:
            // [`reduction`] holds the exemptions, [`lmr_reduction`] the
            // size, and [`Search::late_move`] the reduced search and the
            // full-depth verification of anything it concluded above
            // alpha.
            let mut score = if i == 0 {
                -self.negamax(board, child, ply + 1, -beta, -alpha)
            } else {
                // The index decides whether this move is reduced at all,
                // and the history score decides by how much. The two
                // readings are not the same reading: the index is a rank
                // inside this node's list, so it says where the move came
                // in the sort, and after that sort reads the same table the
                // rank is derived from the score and has lost the part of
                // it that is absolute.
                let base = reduction(in_check, gives_check, m, killers, depth, i);
                self.late_move(board, child, base, self.history.get(us, m), ply, alpha)
            };
            // The re-search, and the two conditions it is not run under.
            // A score at or above beta needs none: fail-soft makes it a
            // lower bound, this node is cutting on it, and what the parent
            // is told is a bound either way. And where this node is itself
            // inside a null window, `beta` is `alpha + 1` and no score can
            // sit between them, so a node searched by the rule above never
            // re-searches its own children under it: the re-searches
            // happen on the principal variation and not through the tree
            // hanging off it. The abort is checked here rather than after
            // the unmake because a search that stopped mid-way returns a
            // value that means nothing, and re-searching it would only
            // spend the time twice.
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
                        // sorted list, which the list already holds: this
                        // loop skips no move, so nothing has to be
                        // remembered as it runs.
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

    /// The transposition table at an interior node: the move a hit named,
    /// and the score to return where the stored bound answers this node's
    /// question outright.
    ///
    /// **The move is worth having whatever the depth says.** A hit too
    /// shallow to answer the question still names the move that answered it
    /// before, and `verify` matched the whole key, so that move belongs to
    /// this position. That is why the two halves come back separately: the
    /// ordering fires far more often than the cutoff does.
    ///
    /// **The cutoff is withheld at the fifty-move limit**, where this node
    /// is a draw or a mate by rule whatever any subtree found, because the
    /// key does not carry the halfmove clock.
    ///
    /// A method and not a free function: it reads the table the search was
    /// handed. It was inline in `negamax` until futility pruning, which is
    /// the change that made that function long enough for the split to be
    /// worth taking; nothing about it moved, and the bench count either
    /// side of the extraction is the same number.
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

    /// The null-window search of one move behind a node's first, reduced
    /// by as many plies as `base` and `history` between them call for: the
    /// late-move arm of the move loop, on the board with the move already
    /// made. The reduced search asks the same question the null window
    /// always asks, over a smaller tree; an answer at or below alpha is the
    /// expected one and stands, and an answer above alpha is re-earned at
    /// the full child depth before anything believes it, so no score a
    /// shallow search concluded reaches the node unverified. The caller's
    /// full-window re-search then follows its own rule on what this
    /// returns, which is how a reduced move that turns out best still gets
    /// the window the node was given.
    ///
    /// **The size arrives in two parts and they are combined here rather
    /// than at the call site**, so that the counters below sit beside the
    /// search they describe: `base` is what the move's index and the
    /// exemptions decided ([`reduction`]), and `history` is the move's
    /// score, which [`history_reduction`] turns into an adjustment of that
    /// and never into a reduction of its own.
    ///
    /// The floor of one ply keeps the reduced search a main-search node
    /// with the quiescence search below it, where the null move's own
    /// reduction may saturate to the quiescence search directly: that
    /// verification starts from a position already above beta, while this
    /// is the first look at a real move.
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
    /// without being made: [`futility_skips`] on everything the move and
    /// its place in the list decide, and then the check exemption.
    ///
    /// **`gives_check` is asked last and only here.** It is the one
    /// expensive question in the rule, two slider lookups against a board
    /// nothing has moved, and only a move that would otherwise be skipped
    /// ever has to answer it: a move that survives the cheap half is
    /// searched without it being asked, and a move that is skipped paid for
    /// the answer with a whole subtree. A move that gives check is forcing,
    /// and [`extension`] deepens exactly what this would discard.
    ///
    /// A method rather than a free function, unlike the three above it,
    /// because it is the half that writes the counters and reads the board.
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

    /// Record what this node's cutoff says about its quiet moves: credit
    /// `cut`, the move that caused it, and debit every quiet move in
    /// `tried`, the moves the node searched ahead of it. Both by
    /// [`history::bonus`] of the node's depth, through the ageing update
    /// that bounds the table.
    ///
    /// **The debit is not an optional half.** It is what gives a move a
    /// negative score, and a negative score is the whole of what
    /// [`history_reduction`] reads on the downside. A table that only
    /// credited would separate the moves that have cut from the moves that
    /// have not, which is what the two killer slots already do at this ply,
    /// and the reduction would have nothing to lengthen.
    ///
    /// **Only a quiet cutoff writes.** When a capture cuts, the quiet moves
    /// behind it were never asked anything this table can learn from: the
    /// sort puts every noisy move that keeps material ahead of every quiet
    /// one, so they were not competing with it. A capture's own ordering is
    /// what it takes, and a slot here spent on one would be a slot spent on
    /// a move the sort already ranks.
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
    /// from it. Two more are here, because both read the board or the
    /// window rather than the margin:
    ///
    /// - **A full-window node is refused**, which is the refusal the null
    ///   move takes and for the same reason: what this returns is a bound
    ///   and no move and no line, which is the answer a null-window
    ///   question wants and not the answer the principal variation wants.
    ///   Where [`futile_node`] declined a window condition, it could: that
    ///   rule returns no score of its own and the node still searches its
    ///   first move. This one returns the node, so the argument that
    ///   excused it there is exactly what does not carry here.
    /// - **A halfmove clock at the limit is refused**, where the node is a
    ///   draw by rule whatever the evaluation reads and `negamax` has not
    ///   run its draw check yet. The same guard the null move carries, one
    ///   line above it, for the same reason.
    ///
    /// **What it shares with the null move and does not guard against.** A
    /// stalemated side is not in check, so it has an evaluation, and a node
    /// that clears beta on it is returned rather than scored as the draw it
    /// is. That exposure is already the null move's and is accepted on the
    /// same ground: the move list is what would settle it, and generating
    /// one is the whole of what both rules save.
    ///
    /// **The zugzwang guard is deliberately not taken**, and that is the
    /// one place these two rules part company. The null move refuses a side
    /// with nothing but pawns beside its king because its mechanism is
    /// passing, and passing is precisely what a side in zugzwang wants and
    /// may not have, so the evidence it collects is inverted there. This
    /// rule collects no such evidence: it compares a reading against a
    /// bound, and its exposure to a position the evaluation misreads is the
    /// one every member of the margin family carries rather than the one
    /// the null move's mechanism creates. `tests/reverse_futility.rs` gates
    /// the difference as a difference, so adding the guard for symmetry
    /// fails a test rather than passing quietly.
    ///
    /// **Why it sits above the null move, which is a measurement and not
    /// only the preamble's order.** The two triggers are nested: this rule
    /// fires where the evaluation clears beta by a margin, the null move
    /// where it merely reaches beta, so every node this returns is a node
    /// the null move would also have taken up. Measured over the bench
    /// positions at depth eleven, of the nodes this rule returns, 91.3% are
    /// nodes whose null-move verification would have cut anyway, at the
    /// cost of a reduced search each, and 8.7% are nodes whose verification
    /// would have come back refusing. **Below the null move, only that
    /// second group would ever reach this rule** -- the first would already
    /// have been cut -- so the whole of what it did would be overruling a
    /// search that had just disagreed with it. The ordering is what makes
    /// the ratio 91 to 9 instead of 0 to 100.
    ///
    /// Nothing is stored in the table. The null move withholds a store
    /// because its entry would carry a reduced depth as if it were `depth`;
    /// here no search happened at all, so the entry would offer a later
    /// probe a cutoff at a depth nothing ever paid for.
    ///
    /// A method rather than a free function, like [`Search::futile`],
    /// because it is the half that reads the board and writes the counters.
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

    /// Null-move pruning at one node: where the side to move can hand the
    /// opponent the move and still stand at or above beta at reduced
    /// depth, the node is cut without its move list being generated.
    /// `Some` is the cutoff (or the meaningless post-abort value every
    /// caller discards); `None` means search the node. Standing above beta
    /// after conceding a whole tempo is the strongest cheap evidence a
    /// node can offer that it will not fail low, and the reduced search
    /// ([`null_reduction`] plies past the one the move would have cost) is
    /// what checks the evidence rather than trusting the evaluation alone.
    ///
    /// What refuses it, and each condition is a wrong answer somewhere: a
    /// node in check (`evals[ply]` is `None` there: passing is not
    /// available, in rule or in spirit); a full-window node, because the
    /// consumer of a bound is a null-window question and the principal
    /// variation wants a value and a line; a beta on the mate scale,
    /// because a reduced search asserting or denying a forced mate has not
    /// proved one, and refusing here is what keeps mate proofs searched in
    /// full; an evaluation below beta, where the evidence is not there to
    /// check; a position reached by the null move itself
    /// (`plies_from_null` of zero), because two passes in a row search the
    /// same position two reductions shallower and answer nothing; a
    /// halfmove clock at the limit, where the node is a draw by rule and
    /// `negamax` has not run its draw check yet; and a side to move with
    /// nothing but pawns beside the king, which is the zugzwang guard
    /// ([`has_non_pawn_material`]).
    ///
    /// A cutoff returns the reduced search's score, fail-soft like every
    /// other bound here, except that a score on the mate scale comes back
    /// as `beta`: the reduction means no mate was proved at this node's
    /// depth, and an unproved mate distance poisons the scale in the way
    /// `score::to_tt` documents. Nothing is stored in the table: the entry
    /// would carry a reduced depth as if it were `depth`, and the saving a
    /// probe hit buys is the saving this rule already took.
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
