// SPDX-License-Identifier: GPL-3.0-or-later

//! The order moves are tried in. Both searches order their moves here, and both by MVV-LVA:
//! most valuable victim first, least valuable attacker among equal victims, a queen promotion
//! above every capture, an underpromotion below every one.

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

/// Capture keys lie in `0..=37`. A queen promotion is above all of them and an underpromotion
/// below; a promotion that also captures adds its capture's key, so it keeps the capture order
/// within its class.
const QUEEN_PROMOTION: i32 = 64;
const UNDERPROMOTION: i32 = -64;

/// The key of a capture of `victim` by `attacker`; a higher key is tried first. The victim's
/// rank dominates, eight to one, and the cheaper attacker wins among equal victims.
#[must_use]
pub const fn capture_key(attacker: PieceType, victim: PieceType) -> i32 {
    rank(victim) * 8 + (5 - rank(attacker))
}

/// The key of a noisy move in `board`: its capture's key, if it captures, with a promotion's
/// bonus or penalty on top. A quiet move is zero.
///
/// # Panics
///
/// If a capture's squares do not hold pieces, which no generated move's fail to.
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

/// The highest and lowest keys [`noisy_key`] can return: a queen promotion that captures a
/// queen, and an underpromotion that captures nothing. Every band below is placed against these
/// rather than against the literals, so that retuning a capture key moves the bands with it.
const NOISY_MAX: i32 = QUEEN_PROMOTION + capture_key(PieceType::Pawn, PieceType::Queen);
const NOISY_MIN: i32 = UNDERPROMOTION;

/// The rank of a quiet move that is not a killer, and one rank for all of them. Below every
/// other band there is.
const QUIET: i32 = -512;

/// What one rank is worth on the scale [`move_key`] returns, and what makes the ranks bands
/// rather than points. Wider than the history score's whole range, which is the property the
/// stage order rests on; the assertion below is what keeps the two in step if either moves.
const BAND: i32 = 1 << 16;

const _: () = assert!(BAND > 2 * HISTORY_MAX);
const _: () = assert!(NOISY_MAX as i64 * BAND as i64 <= i32::MAX as i64);
const _: () = assert!(QUIET as i64 * BAND as i64 - HISTORY_MAX as i64 >= i32::MIN as i64);

/// The two killer ranks, one per slot. Two numbers rather than one shared rank: the sort is
/// stable, so a single rank would leave the slots in generator order, which is the order they
/// exist to override.
const KILLER: [i32; 2] = [-128, -129];

/// What a noisy move's key is reduced by when the exchange on the square it lands on loses
/// material: `see::see` below zero. A subtraction rather than a rank, so the group moves and
/// nothing inside it does.
const LOSING: i32 = 256;

const _: () = assert!(KILLER[0] > KILLER[1]);
const _: () = assert!(KILLER[0] < NOISY_MIN);
const _: () = assert!(NOISY_MAX - LOSING < KILLER[1]);
const _: () = assert!(NOISY_MIN - LOSING > QUIET);

/// The key of any legal move: its rank times [`BAND`], plus its history score where it has one.
/// The rank is [`noisy_key`] for a noisy move whose exchange does not lose material, that key
/// less [`LOSING`] for one that does, a [`KILLER`] rank for a quiet move in a slot, and
/// [`QUIET`] for the rest.
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

/// Order `list`, a noisy move list of `board`, by descending [`noisy_key`]: most valuable
/// victim first. `demote_losing` is off here, so no [`see::see`] is paid for a move the caller
/// is about to skip anyway.
pub fn sort_noisy(board: &Board, list: &mut MoveList) {
    sort_impl(board, list, 0, [Move::NULL; 2], false, &[]);
}

/// Order `list` from `start` onward: the noisy moves by descending [`noisy_key`], then the
/// killers, then the losing noisy moves, then the quiet moves by history score. Stable, so ties
/// keep generation order.
pub fn sort_from(
    board: &Board,
    list: &mut MoveList,
    start: usize,
    killers: [Move; 2],
    history: &[i32],
) {
    sort_impl(board, list, start, killers, true, history);
}

/// The sort both entry points above run, `demote_losing` deciding whether a noisy move that
/// loses material is ranked below the killers or left among the other noisy moves. [`move_key`]
/// says why the two callers answer that differently.
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
