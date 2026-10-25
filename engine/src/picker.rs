// SPDX-License-Identifier: GPL-3.0-or-later

//! Both searches order by MVV-LVA; a queen promotion ranks above every capture, an underpromotion
//! below every one.

use cadence_core::position::Board;
use cadence_core::types::PromoPiece;
use cadence_core::{MAX_MOVES, Move, MoveList, Piece, PieceType, Square};

use crate::history::HISTORY_MAX;
use crate::see;

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

/// Capture keys lie in `0..=37`; a promotion that also captures adds its capture's key, keeping the
/// capture order within its class.
const QUEEN_PROMOTION: i32 = 64;
const UNDERPROMOTION: i32 = -64;

/// A higher key is tried first.
#[must_use]
pub const fn capture_key(attacker: PieceType, victim: PieceType) -> i32 {
    rank(victim) * 8 + (5 - rank(attacker))
}

/// A quiet move is zero.
///
/// # Panics
///
/// If a capture's squares do not hold pieces.
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

/// Bands are placed against these rather than literals, so retuning a capture key moves them.
const NOISY_MAX: i32 = QUEEN_PROMOTION + capture_key(PieceType::Pawn, PieceType::Queen);
const NOISY_MIN: i32 = UNDERPROMOTION;

/// Below every other band.
const QUIET: i32 = -512;

/// Wider than the history score's whole range, which the stage order rests on; the assertion below
/// keeps the two in step.
const BAND: i32 = 1 << 16;

const _: () = assert!(BAND > 2 * HISTORY_MAX);
const _: () = assert!(NOISY_MAX as i64 * BAND as i64 <= i32::MAX as i64);
const _: () = assert!(QUIET as i64 * BAND as i64 - HISTORY_MAX as i64 >= i32::MIN as i64);

/// Two ranks, not one: the sort is stable, so a shared rank would leave the slots in generator
/// order.
const KILLER: [i32; 2] = [-128, -129];

/// A subtraction rather than a rank, so the group moves and nothing inside it does.
const LOSING: i32 = 256;

/// The main search's losing captures, below every quiet move and its history score.
const LAST: i32 = 1024;

const _: () = assert!(KILLER[0] > KILLER[1]);
const _: () = assert!(KILLER[0] < NOISY_MIN);
const _: () = assert!(NOISY_MAX - LOSING < KILLER[1]);
const _: () = assert!(NOISY_MIN - LOSING > QUIET);
const _: () = assert!(NOISY_MAX - LAST < QUIET);
const _: () = assert!((NOISY_MIN - LAST) as i64 * BAND as i64 >= i32::MIN as i64);

/// Where a noisy move that loses material is tried.
#[derive(Clone, Copy)]
enum Losing {
    /// No exchange is evaluated.
    Kept,
    BehindKillers,
    /// A losing promotion keeps its place behind the killers.
    Last,
}

/// Rank times [`BAND`], plus the history score where there is one.
fn move_key(board: &Board, m: Move, killers: [Move; 2], losing: Losing, history: &[i32]) -> i32 {
    if m.is_noisy() {
        let key = noisy_key(board, m);
        if matches!(losing, Losing::Kept) || see::see(board, m) >= 0 {
            key * BAND
        } else if matches!(losing, Losing::Last) && m.promotion_piece().is_none() {
            (key - LAST) * BAND
        } else {
            (key - LOSING) * BAND
        }
    } else if m == killers[0] {
        KILLER[0] * BAND
    } else if m == killers[1] {
        KILLER[1] * BAND
    } else {
        QUIET * BAND + history.get(m.from_to()).copied().unwrap_or(0)
    }
}

/// No exchange is evaluated: the caller is about to skip a losing move anyway.
pub fn sort_noisy(board: &Board, list: &mut MoveList) {
    sort_impl(board, list, 0, [Move::NULL; 2], Losing::Kept, &[]);
}

/// Losing noisy moves stay ahead of the quiet evasions, which carry no history to rank them.
pub fn sort_evasions(board: &Board, list: &mut MoveList) {
    sort_impl(board, list, 0, [Move::NULL; 2], Losing::BehindKillers, &[]);
}

/// Noisy moves, then killers, then losing promotions, then quiet moves by history, then losing
/// captures. Stable, so ties keep generation order.
pub fn sort_from(
    board: &Board,
    list: &mut MoveList,
    start: usize,
    killers: [Move; 2],
    history: &[i32],
) {
    sort_impl(board, list, start, killers, Losing::Last, history);
}

fn sort_impl(
    board: &Board,
    list: &mut MoveList,
    start: usize,
    killers: [Move; 2],
    losing: Losing,
    history: &[i32],
) {
    let all = list.as_mut_slice();
    if start >= all.len() {
        return;
    }
    let moves = &mut all[start..];
    let mut keys = [0i32; MAX_MOVES];
    for (key, &m) in keys.iter_mut().zip(moves.iter()) {
        *key = move_key(board, m, killers, losing, history);
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

/// A move's place in the picker, one per move in a buffer per ply that the search allocates once.
#[derive(Clone, Copy, Default)]
pub struct Slot(());

/// Yields the main search's moves in [`sort_from`]'s order.
pub struct Picker {
    next: usize,
    quiets: usize,
    exchanges: u32,
}

impl Picker {
    /// The first `start` moves are yielded as they stand, as [`sort_from`] leaves them.
    #[must_use]
    pub fn new(
        board: &Board,
        list: &mut MoveList,
        start: usize,
        killers: [Move; 2],
        history: &[i32],
        _slots: &mut [Slot],
    ) -> Picker {
        let noisy = list.as_slice().iter().skip(start).filter(|m| m.is_noisy());
        let exchanges = noisy.count() as u32;
        sort_from(board, list, start, killers, history);
        Picker {
            next: 0,
            quiets: 0,
            exchanges,
        }
    }

    /// Each move once, in order, with its index in the list and its count of the quiet moves ahead
    /// of it, which late move pruning reads; every move yielded counts, searched or given up.
    pub fn next(
        &mut self,
        _board: &Board,
        list: &mut MoveList,
        _slots: &mut [Slot],
    ) -> Option<(Move, usize, usize)> {
        let m = list.as_slice().get(self.next).copied()?;
        let place = (m, self.next, self.quiets);
        self.next += 1;
        self.quiets += usize::from(!m.is_noisy());
        Some(place)
    }

    /// Exchanges read so far, for the gate that holds them to the moves about to be tried.
    #[must_use]
    pub fn exchanges(&self) -> u32 {
        self.exchanges
    }
}
