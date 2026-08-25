// SPDX-License-Identifier: GPL-3.0-or-later

//! The static exchange evaluation: what a move wins or loses on the square
//! it lands on, once every recapture has been answered.
//!
//! **The definition.** The side to move plays `m`. From then on the two
//! sides take turns, and each turn the side to move either stops or
//! captures whatever now stands on `m`'s destination with its least
//! valuable piece that can legally do so, a pawn promoting to a queen when
//! that capture reaches its last rank. Material is counted by [`value`],
//! nothing else is counted, and the result is the minimax of that game for
//! the side that played `m`: the material it is up, or down, once neither
//! side wants to capture again. A castle is not an exchange and evaluates
//! to zero. `m` must be legal in `board`.
//!
//! "Legally" is most of what there is to get wrong, and it is not
//! approximated. A piece pinned to its king under the occupancy **at that
//! point in the exchange** may capture only along the pin, which is what
//! the rules say and more than the usual pin mask taken from the root
//! position says: the piece that recaptured first may have been the second
//! blocker on a line, and its departure pins the piece behind it. A
//! capture that uncovers a check on the other king, from the capturer's
//! origin or from the square an en-passant victim stood on, stops every
//! recapture but the king's. And a king recaptures only onto a square no
//! enemy piece attacks once it has moved, x-rays through its own origin
//! included. The function is gated against a second implementation that
//! plays the exchange out on the board with the legal generator, so the
//! answer is the generator's and the two agree to the integer
//! (`tests/see.rs`).
//!
//! **Least valuable** is by [`value`], then by the order of `PieceType`,
//! then by square. That tie-break is part of the definition, not a detail
//! of one implementation: two pieces of one value can reveal different
//! x-rays, and the function and its oracle have to pick the same one. The
//! king is worth zero in the table and so is tried **first**, which is the
//! opposite of the usual convention and is the better rule: a king may
//! take only where nothing can answer it, so its capture ends the exchange
//! at the full value of what stands on the square, which no other piece
//! can improve on and a slider's recapture can fall short of by uncovering
//! an enemy x-ray behind itself.
//!
//! **The values are this module's own.** The evaluation's material tables
//! are tapered by game phase, and an exchange whose sign depended on the
//! phase would prune a capture in one position and search it in a
//! structurally identical one; beyond that, a knight and a bishop are
//! worth the same here so that a minor for a minor comes out level, which
//! the evaluation's 320 and 330 would make a loss for whichever side
//! captured first. The king's entry orders it and is never read into a
//! result: a king on the square ends the exchange, because it got there
//! legally and nothing may take it.
//!
//! **Determinism and cost.** Integers, no allocation, no table beyond the
//! attack tables `core` already holds; the result is a function of the
//! board and the move alone. Each recapture costs one
//! slider lookup for the x-ray it reveals and, when the capturer stands on
//! a line with either king, one more for the pin or the discovered check.
//! Nothing reads this yet: its first reader is the quiescence search's
//! pruning of losing captures, and until that lands the bench is
//! unchanged, which is the evidence that nothing does.

use cadence_core::attacks;
use cadence_core::position::Board;
use cadence_core::types::Rank;
use cadence_core::{Bitboard, Colour, Move, PieceType, Square};

/// Material by piece type, in the exchange's own units: pawn 100, knight
/// and bishop 300, rook 500, queen 900. The king is zero, which makes it
/// the first piece to recapture with and is never read into a result (see
/// the module doc).
pub const VALUES: [i32; 6] = [100, 300, 300, 500, 900, 0];

// The cheapest-first scan in `cheapest_legal` walks `PieceType` order and
// stops at the first legal attacker, which is the least valuable one only
// while the table is non-decreasing along that order. Pinned here so that
// retuning a value cannot quietly change which piece recaptures.
const _: () = assert!(
    VALUES[0] <= VALUES[1]
        && VALUES[1] <= VALUES[2]
        && VALUES[2] <= VALUES[3]
        && VALUES[3] <= VALUES[4]
);

/// The exchange value of a piece of type `pt`, from [`VALUES`].
#[inline]
#[must_use]
pub const fn value(pt: PieceType) -> i32 {
    VALUES[pt.index()]
}

/// The most captures one exchange can hold: the first move, then at most
/// one by every other piece on the board, each leaving the occupancy as it
/// captures and never returning to it.
const MAX_CAPTURES: usize = 32;

/// The piece types that recapture by the pin rule, cheapest first. The
/// king is not among them: it is tried before all of them, on its own
/// terms, by [`cheapest_legal`].
const RECAPTURERS: [PieceType; 5] = [
    PieceType::Pawn,
    PieceType::Knight,
    PieceType::Bishop,
    PieceType::Rook,
    PieceType::Queen,
];

/// The static exchange value of `m` in `board`, per the module doc:
/// positive when the side to move comes out of the exchange on `m`'s
/// square ahead, negative when it comes out behind, zero for a castle.
///
/// # Panics
///
/// If `m.from_sq()` is empty, which no legal move's is.
#[must_use]
pub fn see(board: &Board, m: Move) -> i32 {
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

    // What each capture takes, the first move's at index zero. `occ` is the
    // board's occupancy with every piece that has captured lifted from it,
    // which is what reveals the x-rays. The square itself is occupied
    // throughout, by whichever piece last landed on it, and it is set
    // rather than assumed: a quiet move's destination and an en-passant
    // capture's are empty at the root, and a line test that saw through
    // the mover would find checks and x-rays that are not there.
    let mut taken = [0i32; MAX_CAPTURES];
    let mut n = 1;
    let mut occ = board.occupied().without(from).with(to);

    // The first move is the only one that can be en passant, a quiet move
    // or an underpromotion, so it is laid out by hand.
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

    // Both colours' attackers of the square under the lifted occupancy,
    // which already sees through the capturer's origin.
    let mut attackers = board.attackers_to(to, occ) & occ;
    let their_king = board.king_square(side.flip());
    let mut discovered = uncovers(board, side, their_king, from, to, occ)
        || ep_victim.is_some_and(|cap| uncovers(board, side, their_king, cap, to, occ));
    side = side.flip();

    // A king on the square ends the exchange: it got there legally, and
    // nothing may take it.
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

    // The minimax, from the last capture back: at every step after the
    // first the side to move takes what is on the square less what the
    // other side then gets, or stops at zero, whichever is more.
    let mut reply = 0;
    for &g in taken[1..n].iter().rev() {
        reply = (g - reply).max(0);
    }
    taken[0] - reply
}

/// The least valuable piece of `side` that attacks `to` and may legally
/// take on it, with its type, or `None` when nothing may. By value, then
/// `PieceType` order, then square, as the module doc defines it: the king
/// first, since it is worth zero, and it takes only onto a square no enemy
/// attacks once it has moved.
///
/// Under a discovered check (`discovered`: `side`'s king is attacked by a
/// piece not on `to`) nothing but the king can take, because nothing else
/// answers the check.
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

/// Whether `piece`, of `side` with its king on `king`, is pinned against
/// taking on `to` under `occ`: it stands alone between its king and an
/// enemy slider on the line, and `to` is off that line.
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

/// Whether `side`'s king on `king` may take on `to`: no enemy piece
/// attacks the square now, and none does through the king's own square
/// once it has left it.
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

/// Whether a piece of `side` leaving `vacated` has uncovered a check on
/// the enemy king at `their_king` from a slider other than whatever now
/// stands on `to`.
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

/// The sliders of `c` that a slider on `a` would see along the line
/// through `a` and `b` under `occ`, in either direction, the first piece
/// each way included: rooks and queens on a rank or file, bishops and
/// queens on a diagonal. Empty when `a` and `b` share no line. Only pieces
/// still in `occ` count, because the board's piece sets still hold the
/// ones the exchange has taken.
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

/// The sliders of either colour that a piece of type `pt` leaving its
/// square has let through to `to`: the diagonals behind a pawn or bishop,
/// the lines behind a rook, both behind a queen. A knight stands on no
/// line through the square; a king's departure ends the exchange.
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
