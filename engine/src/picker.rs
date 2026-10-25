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

const _: () = assert!(KILLER[0] > KILLER[1]);
const _: () = assert!(KILLER[0] < NOISY_MIN);
const _: () = assert!(NOISY_MAX - LOSING < KILLER[1]);
const _: () = assert!(NOISY_MIN - LOSING > QUIET);

/// Where a noisy move that loses material is tried.
#[derive(Clone, Copy)]
enum Losing {
    /// No exchange is evaluated.
    Kept,
    BehindKillers,
}

/// Rank times [`BAND`], plus the history score where there is one.
fn move_key(board: &Board, m: Move, killers: [Move; 2], losing: Losing, history: &[i32]) -> i32 {
    if m.is_noisy() {
        let key = noisy_key(board, m);
        if matches!(losing, Losing::Kept) || see::see(board, m) >= 0 {
            key * BAND
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

/// The main search's order for the whole list, as the picker yields it.
pub fn sort_from(
    board: &Board,
    list: &mut MoveList,
    start: usize,
    killers: [Move; 2],
    history: &[i32],
) {
    let mut slots = [Slot::default(); MAX_MOVES];
    let mut picker = Picker::new(list, start, killers, history, &mut slots);
    while picker.next(board, list, &mut slots).is_some() {}
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

/// A move's key, its index in generation order and the band it is taken from, one per move in a
/// buffer per ply that the search allocates once.
#[derive(Clone, Copy, Default)]
pub struct Slot {
    key: i32,
    generated: u8,
    band: Band,
}

/// A killer is found by its move among the quiet ones.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Band {
    /// A noisy move whose exchange has not been read.
    #[default]
    Untested,
    LosingPromotion,
    Quiet,
    LosingCapture,
}

/// In the order the sort's bands fell: the bands' constants above separate them, so taking them
/// in turn is the sorted order.
#[derive(Clone, Copy)]
enum Stage {
    Head,
    Noisy,
    Killer(usize),
    LosingPromotions,
    Quiets,
    LosingCaptures,
}

/// Yields the main search's moves one band at a time, each band by key and then by generation
/// order, so ties fall as they did in the sort.
pub struct Picker {
    next: usize,
    start: usize,
    /// Where the noisy moves still to be tried end; the quiet moves follow them.
    noisy_end: usize,
    /// Noisy moves found to lose, kept in order ahead of the untested ones.
    demoted: usize,
    /// Where the losing captures begin once the quiet moves are sorted ahead of them.
    losing: usize,
    quiets: usize,
    exchanges: u32,
    killers: [Move; 2],
    stage: Stage,
}

impl Picker {
    /// The first `start` moves are yielded as they stand, and nothing else is ordered until they
    /// have been. The quiet moves are keyed now, by the history as it stands, since a child's cutoffs
    /// can change it before they are tried.
    #[must_use]
    pub fn new(
        list: &MoveList,
        start: usize,
        killers: [Move; 2],
        history: &[i32],
        slots: &mut [Slot],
    ) -> Picker {
        let moves = list.as_slice();
        let mut noisy = 0;
        for (i, &m) in moves.iter().enumerate().skip(start) {
            let generated = (i - start) as u8;
            slots[i] = if m.is_noisy() {
                noisy += 1;
                Slot {
                    key: 0,
                    generated,
                    band: Band::Untested,
                }
            } else {
                Slot {
                    key: history.get(m.from_to()).copied().unwrap_or(0),
                    generated,
                    band: Band::Quiet,
                }
            };
        }
        Picker {
            next: 0,
            start,
            noisy_end: start + noisy,
            demoted: 0,
            losing: moves.len(),
            quiets: 0,
            exchanges: 0,
            killers,
            stage: Stage::Head,
        }
    }

    /// Each move once, in order, with its index in the list and its count of the quiet moves ahead
    /// of it, which late move pruning reads; every move yielded counts, searched or given up.
    pub fn next(
        &mut self,
        board: &Board,
        list: &mut MoveList,
        slots: &mut [Slot],
    ) -> Option<(Move, usize, usize)> {
        let moves = list.as_mut_slice();
        if !self.place(board, moves, slots) {
            return None;
        }
        let m = moves[self.next];
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

    /// The noisy moves keyed by the board and put ahead of the quiet ones, once the table's move has
    /// been tried, so a cutoff on it orders nothing.
    fn rank_noisy(&self, board: &Board, moves: &mut [Move], slots: &mut [Slot]) {
        for (i, &m) in moves.iter().enumerate().skip(self.start) {
            if slots[i].band == Band::Untested {
                slots[i].key = noisy_key(board, m);
            }
        }
        insertion_sort(moves, slots, self.start, |s, t| {
            s.band == Band::Untested && (t.band == Band::Quiet || outranks(s, t))
        });
    }

    /// Puts the next move at `next`, or says there is none. A noisy move has its exchange read
    /// only when it is the greatest untested one, and one that loses stays behind in order.
    fn place(&mut self, board: &Board, moves: &mut [Move], slots: &mut [Slot]) -> bool {
        loop {
            match self.stage {
                Stage::Head if self.next < self.start => return true,
                Stage::Head => {
                    self.rank_noisy(board, moves, slots);
                    self.stage = Stage::Noisy;
                }
                Stage::Noisy => {
                    let u = self.next + self.demoted;
                    if u == self.noisy_end {
                        self.stage = Stage::Killer(0);
                        continue;
                    }
                    self.exchanges += 1;
                    if see::see(board, moves[u]) >= 0 {
                        bring(moves, slots, self.next, u);
                        return true;
                    }
                    slots[u].band = if moves[u].promotion_piece().is_some() {
                        Band::LosingPromotion
                    } else {
                        Band::LosingCapture
                    };
                    self.demoted += 1;
                }
                Stage::Killer(k) => {
                    self.stage = if k == 0 {
                        Stage::Killer(1)
                    } else {
                        Stage::LosingPromotions
                    };
                    let killer = self.killers[k];
                    if let Some(q) = (self.noisy_end..moves.len()).find(|&q| moves[q] == killer) {
                        bring(moves, slots, self.next, q);
                        self.noisy_end += 1;
                        return true;
                    }
                }
                Stage::LosingPromotions => {
                    let promotion = (self.next..self.noisy_end)
                        .find(|&p| slots[p].band == Band::LosingPromotion);
                    if let Some(p) = promotion {
                        bring(moves, slots, self.next, p);
                        return true;
                    }
                    // What is left of the noisy moves loses captures; the quiet moves, sorted
                    // once, go ahead of them.
                    let captures = self.noisy_end - self.next;
                    insertion_sort(moves, slots, self.noisy_end, outranks);
                    moves[self.next..].rotate_left(captures);
                    slots[self.next..moves.len()].rotate_left(captures);
                    self.losing = moves.len() - captures;
                    self.stage = Stage::Quiets;
                }
                Stage::Quiets if self.next < self.losing => return true,
                Stage::Quiets => self.stage = Stage::LosingCaptures,
                Stage::LosingCaptures => return self.next < moves.len(),
            }
        }
    }
}

/// The greater key first, and the earlier generated among equals.
fn outranks(s: Slot, t: Slot) -> bool {
    s.key > t.key || (s.key == t.key && s.generated < t.generated)
}

/// Moves each entry from `from` on back past every entry it goes `ahead` of, so entries that go
/// ahead of none keep their order; nearly linear on the ties history leaves.
fn insertion_sort(
    moves: &mut [Move],
    slots: &mut [Slot],
    from: usize,
    ahead: impl Fn(Slot, Slot) -> bool,
) {
    for i in from + 1..moves.len() {
        let (m, s) = (moves[i], slots[i]);
        let mut j = i;
        while j > from && ahead(s, slots[j - 1]) {
            moves[j] = moves[j - 1];
            slots[j] = slots[j - 1];
            j -= 1;
        }
        moves[j] = m;
        slots[j] = s;
    }
}

/// The entry at `from` to `to`, the ones between moved up one in order.
fn bring(moves: &mut [Move], slots: &mut [Slot], to: usize, from: usize) {
    moves[to..=from].rotate_right(1);
    slots[to..=from].rotate_right(1);
}
