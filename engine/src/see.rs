// SPDX-License-Identifier: GPL-3.0-or-later

//! Each side either stops or captures with its least valuable legal piece, a pawn reaching the last
//! rank promoting to a queen.

#[cfg(feature = "counters")]
use std::cell::Cell;

use cadence_core::attacks;
use cadence_core::position::Board;
use cadence_core::types::Rank;
use cadence_core::{Bitboard, Colour, Move, PieceType, Square};

/// The king is zero, so it recaptures first, and is never read into a result.
pub const VALUES: [i32; 6] = [100, 300, 300, 500, 900, 0];

// `cheapest_legal` scans in `PieceType` order, which finds the least valuable attacker only while
// this is non-decreasing.
const _: () = assert!(
    VALUES[0] <= VALUES[1]
        && VALUES[1] <= VALUES[2]
        && VALUES[2] <= VALUES[3]
        && VALUES[3] <= VALUES[4]
);

#[inline]
#[must_use]
pub const fn value(pt: PieceType) -> i32 {
    VALUES[pt.index()]
}

/// The first move, then at most one by every other piece, each lifted from the occupancy as it
/// captures.
const MAX_CAPTURES: usize = 32;

/// The king is tried before all of them, on its own terms, by [`cheapest_legal`].
const RECAPTURERS: [PieceType; 5] = [
    PieceType::Pawn,
    PieceType::Knight,
    PieceType::Bishop,
    PieceType::Rook,
    PieceType::Queen,
];

#[cfg(feature = "counters")]
thread_local! {
    static READS: Cell<u64> = const { Cell::new(0) };
}

/// Every exchange read on this thread, which a gate holds to the counts of the places that read
/// one; only the `counters` feature keeps it.
#[cfg(feature = "counters")]
#[must_use]
pub fn reads() -> u64 {
    READS.with(Cell::get)
}

/// Positive when the side to move comes out ahead, zero for a castle.
///
/// # Panics
///
/// If `m.from_sq()` is empty.
#[must_use]
pub fn see(board: &Board, m: Move) -> i32 {
    #[cfg(feature = "counters")]
    READS.with(|r| r.set(r.get() + 1));
    if m.is_castle() {
        return 0;
    }
    let to = m.to_sq();
    let from = m.from_sq();
    let mut side = board.side_to_move();
    let mover = board
        .piece_at(from)
        .expect("see: no piece on the from square")
        .piece_type();

    // `occ` has every piece that has captured lifted from it, which reveals the x-rays.
    let mut taken = [0i32; MAX_CAPTURES];
    let mut n = 1;
    let mut occ = board.occupied().without(from).with(to);

    // Only the first move can be en passant, quiet or an underpromotion, so it is laid out by hand.
    let mut victim = board.piece_at(to).map_or(0, |p| value(p.piece_type()));
    let mut ep_victim = None;
    if m.is_en_passant() {
        let cap = Square::new(to.index() as u8 ^ 8);
        occ = occ.without(cap);
        victim = value(PieceType::Pawn);
        ep_victim = Some(cap);
    }
    let mut on_square = mover;
    taken[0] = victim;
    if let Some(p) = m.promotion_piece() {
        on_square = p.piece_type();
        taken[0] += value(on_square) - value(PieceType::Pawn);
    }

    // The lifted occupancy already sees through the capturer's origin.
    let mut attackers = board.attackers_to(to, occ) & occ;
    let their_king = board.king_square(side.flip());
    let mut discovered = uncovers(board, side, their_king, from, to, occ)
        || ep_victim.is_some_and(|cap| uncovers(board, side, their_king, cap, to, occ));
    side = side.flip();

    // A king on the square ends the exchange: it got there legally, and nothing may take it.
    while on_square != PieceType::King {
        let Some((sq, pt)) = cheapest_legal(board, side, to, attackers, occ, discovered) else {
            break;
        };
        let mut gain = value(on_square);
        if pt == PieceType::Pawn && to.rank() == Rank::Eight.relative(side) {
            gain += value(PieceType::Queen) - value(PieceType::Pawn);
            on_square = PieceType::Queen;
        } else {
            on_square = pt;
        }
        taken[n] = gain;
        n += 1;
        if pt == PieceType::King {
            break;
        }
        occ = occ.without(sq);
        attackers = (attackers | revealed(board, to, occ, pt)) & occ;
        discovered = uncovers(board, side, board.king_square(side.flip()), sq, to, occ);
        side = side.flip();
    }

    // The minimax, from the last capture back: at every step after the first the side to move
    // takes what is on the square less what the other side then gets, or stops at zero,
    // whichever is more.
    let mut reply = 0;
    for &g in taken[1..n].iter().rev() {
        reply = (g - reply).max(0);
    }
    taken[0] - reply
}

/// Whether [`see`] is at least `threshold`, read through the same captures in the same order and
/// stopped as soon as the replies not yet read cannot change the answer.
///
/// # Panics
///
/// If `m.from_sq()` is empty.
#[must_use]
pub fn see_ge(board: &Board, m: Move, threshold: i32) -> bool {
    #[cfg(feature = "counters")]
    READS.with(|r| r.set(r.get() + 1));
    if m.is_castle() {
        return threshold <= 0;
    }
    let to = m.to_sq();
    let from = m.from_sq();
    let mut side = board.side_to_move();
    let mover = board
        .piece_at(from)
        .expect("see: no piece on the from square")
        .piece_type();

    let mut taken = [0i32; MAX_CAPTURES];
    let mut n = 1;
    let mut occ = board.occupied().without(from).with(to);

    let mut victim = board.piece_at(to).map_or(0, |p| value(p.piece_type()));
    let mut ep_victim = None;
    if m.is_en_passant() {
        let cap = Square::new(to.index() as u8 ^ 8);
        occ = occ.without(cap);
        victim = value(PieceType::Pawn);
        ep_victim = Some(cap);
    }
    let mut on_square = mover;
    taken[0] = victim;
    if let Some(p) = m.promotion_piece() {
        on_square = p.piece_type();
        taken[0] += value(on_square) - value(PieceType::Pawn);
    }
    // No reply is below zero, so the first gain is the most the exchange can be worth.
    if taken[0] < threshold {
        return false;
    }

    let mut attackers = board.attackers_to(to, occ) & occ;
    let their_king = board.king_square(side.flip());
    let mut discovered = uncovers(board, side, their_king, from, to, occ)
        || ep_victim.is_some_and(|cap| uncovers(board, side, their_king, cap, to, occ));
    side = side.flip();

    while on_square != PieceType::King {
        let Some((sq, pt)) = cheapest_legal(board, side, to, attackers, occ, discovered) else {
            break;
        };
        let mut gain = value(on_square);
        if pt == PieceType::Pawn && to.rank() == Rank::Eight.relative(side) {
            gain += value(PieceType::Queen) - value(PieceType::Pawn);
            on_square = PieceType::Queen;
        } else {
            on_square = pt;
        }
        taken[n] = gain;
        n += 1;
        if pt == PieceType::King {
            break;
        }
        let (low, high) = bounds(&taken[..n]);
        if low >= threshold {
            return true;
        }
        if high < threshold {
            return false;
        }
        occ = occ.without(sq);
        attackers = (attackers | revealed(board, to, occ, pt)) & occ;
        discovered = uncovers(board, side, board.king_square(side.flip()), sq, to, occ);
        side = side.flip();
    }

    let mut reply = 0;
    for &g in taken[1..n].iter().rev() {
        reply = (g - reply).max(0);
    }
    taken[0] - reply >= threshold
}

/// The least and the most the exchange can be worth with `gains` read, every reply not yet read
/// lying anywhere from zero up. Each step back is [`see`]'s minimax over the ends of an interval.
fn bounds(gains: &[i32]) -> (i32, i32) {
    let (mut low, mut high) = (0, i32::MAX);
    for &g in gains[1..].iter().rev() {
        (low, high) = ((g - high).max(0), (g - low).max(0));
    }
    (gains[0] - high, gains[0] - low)
}

/// Under a discovered check only the king can take, because nothing else answers it.
fn cheapest_legal(
    board: &Board,
    side: Colour,
    to: Square,
    attackers: Bitboard,
    occ: Bitboard,
    discovered: bool,
) -> Option<(Square, PieceType)> {
    let own = attackers & board.by_colour(side);
    if own.is_empty() {
        return None;
    }
    let king = board.king_square(side);
    if own.contains(king) && king_may_take(board, side, king, to, attackers, occ) {
        return Some((king, PieceType::King));
    }
    if discovered {
        return None;
    }
    for pt in RECAPTURERS {
        for sq in own & board.by_type(pt) {
            if !pinned(board, side, king, sq, to, occ) {
                return Some((sq, pt));
            }
        }
    }
    None
}

/// Alone between its king and an enemy slider on the line, with `to` off that line.
fn pinned(
    board: &Board,
    side: Colour,
    king: Square,
    piece: Square,
    to: Square,
    occ: Bitboard,
) -> bool {
    let line = attacks::ray(king, piece);
    if line.is_empty() || line.contains(to) {
        return false;
    }
    if (attacks::between(king, piece) & occ).any() {
        return false;
    }
    sliders_along(board, side.flip(), piece, king, occ).any()
}

/// Nothing attacks the square now, nor through the king's own square once it has left.
fn king_may_take(
    board: &Board,
    side: Colour,
    king: Square,
    to: Square,
    attackers: Bitboard,
    occ: Bitboard,
) -> bool {
    let them = side.flip();
    if (attackers & board.by_colour(them)).any() {
        return false;
    }
    sliders_along(board, them, to, king, occ.without(king)).is_empty()
}

/// From a slider other than whatever now stands on `to`.
fn uncovers(
    board: &Board,
    side: Colour,
    their_king: Square,
    vacated: Square,
    to: Square,
    occ: Bitboard,
) -> bool {
    sliders_along(board, side, their_king, vacated, occ)
        .without(to)
        .any()
}

/// The first piece each way included; empty when `a` and `b` share no line.
fn sliders_along(board: &Board, c: Colour, a: Square, b: Square, occ: Bitboard) -> Bitboard {
    let line = attacks::ray(a, b);
    if line.is_empty() {
        return Bitboard::EMPTY;
    }
    let queens = board.pieces(c, PieceType::Queen);
    let (seen, set) = if a.file() == b.file() || a.rank() == b.rank() {
        (
            attacks::rook_attacks(a, occ),
            board.pieces(c, PieceType::Rook) | queens,
        )
    } else {
        (
            attacks::bishop_attacks(a, occ),
            board.pieces(c, PieceType::Bishop) | queens,
        )
    };
    seen & line & set & occ
}

/// A knight stands on no line through the square; a king's departure ends the exchange.
fn revealed(board: &Board, to: Square, occ: Bitboard, pt: PieceType) -> Bitboard {
    let queens = board.by_type(PieceType::Queen);
    let mut out = Bitboard::EMPTY;
    if matches!(pt, PieceType::Pawn | PieceType::Bishop | PieceType::Queen) {
        out |= attacks::bishop_attacks(to, occ) & (board.by_type(PieceType::Bishop) | queens);
    }
    if matches!(pt, PieceType::Rook | PieceType::Queen) {
        out |= attacks::rook_attacks(to, occ) & (board.by_type(PieceType::Rook) | queens);
    }
    out
}
