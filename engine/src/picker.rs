// SPDX-License-Identifier: GPL-3.0-or-later

//! The order moves are tried in.
//!
//! Both searches order their moves here, and both by MVV-LVA: most
//! valuable victim first, least valuable attacker among equal victims, a
//! queen promotion above every capture, an underpromotion below every one.
//! The quiescence search sorts a list that is noisy by construction,
//! except in check, where it sorts the legal list and the quiet evasions
//! rank behind the capture of the checking piece. The main search sorts
//! the legal list, which is mostly quiet, from behind whatever
//! `search::order_first` left at its head: a quiet move ranks
//! below every noisy one, and among the quiet moves the two that caused a
//! beta cutoff at a sibling of this node come first. The rest rank by
//! their history score ([`crate::history`]), which is what a quiet move
//! has been worth elsewhere in this search, and the ones with no score
//! keep generation order among themselves.
//!
//! **Every rank here is a band, and only one band has anything inside
//! it.** A score is not a constant and does not fit between two of them,
//! so the ranks below are multiplied by [`BAND`] and the history score is
//! added within the quiet one. `BAND` is wider than the score's whole
//! range, so no history can carry a move out of the class its rank put it
//! in: the stage order is a property of the ranks and the score only
//! decides position inside the last stage.
//!
//! **Why this is part of the quiescence search rather than a refinement
//! of it.** Measured before it existed: depth one from Kiwipete, noisy
//! moves in generated order, is 159,421,843 nodes. Alpha-beta cuts a
//! capture sequence only once it has seen the capture that refutes it,
//! and in generated order the pawn that takes the queen comes after the
//! knight, bishop and queen captures that do not. Sorted, the same measured
//! search is a few hundred nodes.
//!
//! **Determinism.** The key is a function of the move and the two pieces
//! it touches, the sort is a stable insertion sort, and ties keep
//! generation order, so the order -- and with it the bench number -- is a
//! function of the position alone.

use cadence_core::position::Board;
use cadence_core::types::PromoPiece;
use cadence_core::{MAX_MOVES, Move, MoveList, Piece, PieceType, Square};

use crate::history::HISTORY_MAX;
use crate::see;

/// A piece's rank in the order pawn, knight, bishop, rook, queen, king.
const fn rank(pt: PieceType) -> i32 {
    match pt {
        PieceType::Pawn => 0,
        PieceType::Knight => 1,
        PieceType::Bishop => 2,
        PieceType::Rook => 3,
        PieceType::Queen => 4,
        PieceType::King => 5,
    }
}

/// Capture keys lie in `0..=37`. A queen promotion is above all of them
/// and an underpromotion below; a promotion that also captures adds its
/// capture's key, so it keeps the capture order within its class.
const QUEEN_PROMOTION: i32 = 64;
const UNDERPROMOTION: i32 = -64;

/// The key of a capture of `victim` by `attacker`; a higher key is tried
/// first. The victim's rank dominates, eight to one, and the cheaper
/// attacker wins among equal victims.
#[must_use]
pub const fn capture_key(attacker: PieceType, victim: PieceType) -> i32 {
    rank(victim) * 8 + (5 - rank(attacker))
}

/// The key of a noisy move in `board`: its capture's key, if it captures,
/// with a promotion's bonus or penalty on top. A quiet move is zero.
///
/// # Panics
///
/// If a capture's squares do not hold pieces, which no generated move's
/// fail to.
#[must_use]
pub fn noisy_key(board: &Board, m: Move) -> i32 {
    let capture = if m.is_en_passant() {
        capture_key(PieceType::Pawn, PieceType::Pawn)
    } else if m.is_capture() {
        capture_key(
            piece_type_at(board, m.from_sq()),
            piece_type_at(board, m.to_sq()),
        )
    } else {
        0
    };
    match m.promotion_piece() {
        Some(PromoPiece::Queen) => QUEEN_PROMOTION + capture,
        Some(_) => UNDERPROMOTION + capture,
        None => capture,
    }
}

fn piece_type_at(board: &Board, sq: Square) -> PieceType {
    board
        .piece_at(sq)
        .map_or_else(|| panic!("no piece on {sq}"), Piece::piece_type)
}

/// The highest and lowest keys [`noisy_key`] can return: a queen promotion
/// that captures a queen, and an underpromotion that captures nothing.
/// Every band below is placed against these rather than against the
/// literals, so that retuning a capture key moves the bands with it.
const NOISY_MAX: i32 = QUEEN_PROMOTION + capture_key(PieceType::Pawn, PieceType::Queen);
const NOISY_MIN: i32 = UNDERPROMOTION;

/// The rank of a quiet move that is not a killer, and one rank for all of
/// them. Below every other band there is.
///
/// One rank and a score inside it: the history heuristic is what
/// distinguishes one of these from another, and a quiet move whose score is
/// zero -- nothing has cut with it and nothing has refuted it -- keeps the
/// place a stable sort gives it, which is generation order.
const QUIET: i32 = -512;

/// What one rank is worth on the scale [`move_key`] returns, and what makes
/// the ranks bands rather than points.
///
/// Wider than the history score's whole range, which is the property the
/// stage order rests on: a quiet move at the top of its band still sorts
/// below the lowest losing capture, and one at the bottom still sorts above
/// nothing at all, because there is nothing below it. Sixty-five thousand
/// five hundred and thirty six, against a range of thirty-two thousand
/// seven hundred and sixty nine, and the assertion below is what keeps the
/// two in step if either moves.
const BAND: i32 = 1 << 16;

const _: () = assert!(BAND > 2 * HISTORY_MAX);
const _: () = assert!(NOISY_MAX as i64 * BAND as i64 <= i32::MAX as i64);
const _: () = assert!(QUIET as i64 * BAND as i64 - HISTORY_MAX as i64 >= i32::MIN as i64);

/// The two killer ranks, one per slot: the quiet moves that caused a beta
/// cutoff at a sibling of the node being sorted.
///
/// They sit below every noisy rank a move that does not lose material can
/// have, and above the band the losing ones are moved to. Each of those
/// edges is what the stage order means and none of them is visible where
/// the numbers are chosen, so all of them are pinned below.
///
/// **Two numbers rather than one shared rank.** The sort is stable, so a
/// single rank would leave the two slots in the order the generator emitted
/// them in, which is the order the slots exist to override.
const KILLER: [i32; 2] = [-128, -129];

/// What a noisy move's key is reduced by when the exchange on the square it
/// lands on loses material: `see::see` below zero.
///
/// **A band rather than one rank, and the subtraction is what makes it
/// one.** A losing capture keeps its key relative to the other losing
/// ones, so the group moves and nothing inside it does. Ranking within the
/// group by how much it loses rather than by what it takes is a different
/// change with a number of its own.
const LOSING: i32 = 256;

const _: () = assert!(KILLER[0] > KILLER[1]);
const _: () = assert!(KILLER[0] < NOISY_MIN);
const _: () = assert!(NOISY_MAX - LOSING < KILLER[1]);
const _: () = assert!(NOISY_MIN - LOSING > QUIET);

/// The key of any legal move: its rank times [`BAND`], plus its history
/// score where it has one. The rank is [`noisy_key`] for a noisy move whose
/// exchange does not lose material, that key less [`LOSING`] for one that
/// does, a [`KILLER`] rank for a quiet move held in one of the slots, and
/// [`QUIET`] for the rest.
///
/// **Only the quiet band carries a score.** A killer has two ranks of its
/// own and mixing a score into them would blur what the killer stage
/// means: the slots say "this move cut at a sibling of *this* node", which
/// is a sharper claim than the table's and is the reason they sit above it.
/// A noisy move is ranked by what it takes.
///
/// `history` is the side to move's row, or an empty slice for a caller with
/// no table to offer, which reads a zero for every move and is the order
/// this function returned before the table existed.
///
/// **The order of the branches is the contract.** Nothing in the signature
/// stops a slot holding a capture, and one ranked as a killer would sort
/// below every other capture.
///
/// **`demote_losing` is off for the one list that can gain nothing from
/// it**, which is the quiescence search's out-of-check noisy list.
/// `quiesce` refuses every move whose exchange loses material before it
/// searches it, so ranking those moves lower reorders only moves that are
/// about to be skipped: the moves actually searched come out in the same
/// order either way and the node count cannot move. Leaving it off there is
/// not a shortcut, it is the sort not being asked a question that costs a
/// [`see::see`] call per move and whose answer is discarded.
/// `tests/ordering.rs` pins that the two agree on the moves that are
/// searched.
///
/// [`noisy_key`] is not this function. It answers zero for a quiet move,
/// which is the rank of a king capturing a pawn and above every
/// underpromotion, and that is harmless only because its own caller is
/// handed a list with no quiet move in it.
fn move_key(
    board: &Board,
    m: Move,
    killers: [Move; 2],
    demote_losing: bool,
    history: &[i32],
) -> i32 {
    if m.is_noisy() {
        let key = noisy_key(board, m);
        if demote_losing && see::see(board, m) < 0 {
            (key - LOSING) * BAND
        } else {
            key * BAND
        }
    } else if m == killers[0] {
        KILLER[0] * BAND
    } else if m == killers[1] {
        KILLER[1] * BAND
    } else {
        QUIET * BAND + history.get(m.from_to()).copied().unwrap_or(0)
    }
}

/// Order `list`, a noisy move list of `board`, by descending
/// [`noisy_key`]: most valuable victim first, losing captures among the
/// rest rather than behind them.
///
/// No killers: a killer is a quiet move and this list holds none, and
/// `Move::NULL` matches nothing a generator emits.
///
/// **This is the one sort that does not put the losing captures last, and
/// the reason is that its caller does not search them at all.** The
/// quiescence search out of check refuses a noisy move whose exchange
/// loses material (`search::quiesce`), so demoting those moves would
/// reorder only moves that are about to be skipped, leave the searched
/// moves in the order they are in now, and cost a [`see::see`] call per
/// move to do it. `sort_from` is the sort that demotes, and it is what the
/// two lists that do search their losing moves are given: the main
/// search's legal list, and the in-check evasion list, where a losing
/// capture is still a legal answer to the check.
pub fn sort_noisy(board: &Board, list: &mut MoveList) {
    sort_impl(board, list, 0, [Move::NULL; 2], false, &[]);
}

/// Order `list` from `start` onward: the noisy moves by descending
/// [`noisy_key`], then the two killers, then the remaining quiet moves by
/// their history score. The moves before `start` are left where they are.
///
/// **Why it takes quiet moves at all.** The main search sorts the legal
/// list, which is mostly quiet, and so does the quiescence search when the
/// side to move is in check. A quiet move ranks below every noisy one, below
/// an underpromotion, which is the lowest noisy rank there is. Every quiet
/// move but the two in `killers` sits in that one band, ordered inside it by
/// what `history` says the move has been worth, and moves the table says
/// nothing about keep the order the generator emitted them in.
///
/// **What `history` is for.** The side to move's row of the search's
/// [`crate::history::History`], or an empty slice from a caller with none:
/// the quiescence search's in-check evasions pass one, on the same ground
/// they pass no killers, since whether either ranks a quiet evasion
/// usefully is unmeasured and is a change of its own.
///
/// **What `start` is for.** It keeps the two ordering stages apart. The
/// transposition table's move is rotated to the head by
/// [`crate::search::order_first`] and has to stay there whatever it ranks,
/// so the search sorts from one when the rotation happened and from zero
/// when it did not.
///
/// **What `killers` is for.** The two quiet moves that cut at a sibling of
/// this node, which rank above every other quiet move and below every noisy
/// one. A killer that is not legal here is not a case to handle: it matches
/// nothing in the list and ranks nobody, which is the whole of the check.
/// A caller with none passes `[Move::NULL; 2]`, and `a1a1` quiet is a
/// pattern no generator emits.
///
/// **How it sorts.** A stable insertion sort over a parallel key array,
/// which is what the quiescence search's lists have always used: the lists
/// are short, ties keep generation order, and the whole order -- and with
/// it the bench number -- stays a function of the position alone.
pub fn sort_from(
    board: &Board,
    list: &mut MoveList,
    start: usize,
    killers: [Move; 2],
    history: &[i32],
) {
    sort_impl(board, list, start, killers, true, history);
}

/// The sort both entry points above run, `demote_losing` deciding whether
/// a noisy move that loses material is ranked below the killers or left
/// among the other noisy moves. [`move_key`] says why the two callers
/// answer that differently.
fn sort_impl(
    board: &Board,
    list: &mut MoveList,
    start: usize,
    killers: [Move; 2],
    demote_losing: bool,
    history: &[i32],
) {
    let all = list.as_mut_slice();
    if start >= all.len() {
        return;
    }
    let moves = &mut all[start..];
    let mut keys = [0i32; MAX_MOVES];
    for (key, &m) in keys.iter_mut().zip(moves.iter()) {
        *key = move_key(board, m, killers, demote_losing, history);
    }
    for i in 1..moves.len() {
        let (m, k) = (moves[i], keys[i]);
        let mut j = i;
        while j > 0 && keys[j - 1] < k {
            moves[j] = moves[j - 1];
            keys[j] = keys[j - 1];
            j -= 1;
        }
        moves[j] = m;
        keys[j] = k;
    }
}
