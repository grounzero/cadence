// SPDX-License-Identifier: GPL-3.0-or-later

//! Legal move generation.
//!
//! Legal, not pseudo-legal: nothing downstream filters this list, and the
//! corpus asserts against it directly. Legality comes from three sources,
//! all maintained by `position` and gated there:
//!
//! - **Dispatch on `checkers.count()`.** Zero: everything. One: king
//!   moves, captures of the checker, interpositions on `between(king,
//!   checker)`, **and the en-passant capture of a checking pawn**, whose
//!   destination is neither the checker's square nor on `between`, so a
//!   target mask alone cannot express it. Two or more: king moves only.
//! - **Pins**, from `blockers(us) & own`. A pinned piece moves only along
//!   `ray(king, piece)`; intersected with the evasion targets that is empty,
//!   which is correct: a pinned piece can never resolve a check.
//! - **The king under `occ ^ king_bb`**: a king in check from a slider
//!   may not retreat along the ray, and with the king still in the occupancy
//!   it blocks the ray beyond itself and the square tests as safe.
//!
//! **A king is never a target, of either colour.** Everything above decides
//! which of our moves are legal; this one decides what a move may land on,
//! and it is the only rule here that a position reachable by legal play
//! cannot exercise. Such a position never offers the enemy king as a capture
//! (that would mean the side not to move was in check), so the mask removes
//! no move from any legal position and no node count moves. It earns its
//! place on positions `from_fen` accepts and legal play cannot reach, where
//! without it generation offers the king capture, `make_move` removes the
//! king, and the check info recomputed on the way in asks `king_square` for
//! a piece that is no longer on the board. See `Board::opponent_in_check`
//! and `tests/opponent_in_check.rs`.
//!
//! Two things are verified explicitly rather than by mask. Every en-passant
//! capture (there can be two) is tested against the occupancy with both
//! pawns removed and the capturer landed, because the capture removes
//! two pieces from one rank and no pin mask can see the horizontal
//! double-vacate. Castling goes through `Board::can_castle`, which lifts
//! both the king and the castling rook and is the one deliberate
//! exception to "king moves use `occ ^ king_bb`".
//!
//! Two entry points, one generator. [`generate_noisy`] is the captures,
//! en-passant captures and promotions, as the subsequence of
//! [`generate_legal`] for which `is_noisy` holds and in the same order; it
//! exists because a quiescence search asks for exactly that at the most
//! numerous node in its tree. Both are `generate::<NOISY>` and differ only
//! in the branches the noisy one skips, which is what keeps the order
//! shared -- `tests/noisy_moves.rs` asserts it as a sequence.

use crate::attacks;
use crate::bitboard::Bitboard;
use crate::castling::{CastleSide, ci};
use crate::mv::{Move, MoveList};
use crate::position::Board;
use crate::types::{PieceType, PromoPiece, Rank, Square};

/// What every generator below reads: the position's summary for the side to
/// move, computed once.
struct Ctx {
    us: crate::types::Colour,
    them: crate::types::Colour,
    occ: Bitboard,
    enemy: Bitboard,
    ksq: Square,
    checkers: Bitboard,
    pinned: Bitboard,
    /// Where a non-king piece may land: `!own` in a quiet position, the
    /// checker plus the squares between it and the king in single check.
    targets: Bitboard,
}

/// Every legal move in `board`, in no defined order.
#[must_use]
pub fn generate_legal(board: &Board) -> MoveList {
    generate::<false>(board)
}

/// The noisy moves of `board` -- captures, en-passant captures and
/// promotions -- as the subsequence of [`generate_legal`] for which
/// [`Move::is_noisy`] holds, **in that list's order**. In check, the noisy
/// evasions. Never a castle, never a double push.
///
/// The order is part of the contract: `tests/noisy_moves.rs` compares the
/// two as sequences, and a search that swaps a filter over the legal list
/// for this must visit the same nodes. It holds because both lists come
/// from one generator walking one set of loops; this one skips the quiet
/// branches and does nothing else differently.
#[must_use]
pub fn generate_noisy(board: &Board) -> MoveList {
    generate::<true>(board)
}

/// Both generators. `NOISY` restricts every branch to its captures and
/// promotions; the branches are walked in the same order either way, which
/// is what makes the noisy list a subsequence of the legal one.
fn generate<const NOISY: bool>(board: &Board) -> MoveList {
    let mut list = MoveList::new();
    let us = board.side_to_move();
    let them = us.flip();
    let occ = board.occupied();
    let own = board.by_colour(us);
    let enemy = board.by_colour(them);
    let ksq = board.king_square(us);
    let checkers = board.checkers();
    // A king is never a target, and this is the one line the whole property
    // rests on. In a position reachable by legal play the enemy king is not
    // attacked by the side to move, so no move onto it was ever generated
    // and the mask removes nothing: the move lists, and therefore perft and
    // bench, are unchanged. In one that is not reachable -- which `from_fen`
    // accepts, because it validates that a position is representable and not
    // that it is legal -- it is what stops `make_move` taking a king off the
    // board and `king_square` then asking for one that is gone.
    let not_a_king = !board.pieces(them, PieceType::King);

    // King moves, always, with the king lifted from the occupancy.
    let occ_without_king = occ.without(ksq);
    let king_targets = (if NOISY { enemy } else { !own }) & not_a_king;
    for to in attacks::king_attacks(ksq) & king_targets {
        if (board.attackers_to(to, occ_without_king) & enemy).is_empty() {
            list.push(capture_or_quiet(ksq, to, enemy));
        }
    }

    // Double check: nothing but the king can help. The branch is right for
    // any count above one, and it used to assert that the count was exactly
    // two. That is true of a position reachable by legal play -- a
    // discovered check reveals at most one piece besides the mover -- and
    // false of one `from_fen` merely accepted, where three pieces can bear
    // on a king at once. The claim was correct; the input it was asserted
    // over was wider than the claim.
    if checkers.more_than_one() {
        return list;
    }

    let ctx = Ctx {
        us,
        them,
        occ,
        enemy,
        ksq,
        checkers,
        pinned: board.blockers(us) & own,
        targets: match checkers.lsb() {
            Some(checker) => checkers | attacks::between(ksq, checker),
            None => !own,
        } & not_a_king,
    };

    // Castling, only out of a quiet position, and never noisy: the
    // destination holds our own rook. `can_castle` lifts both the king and
    // the rook; nothing here reuses the king-lifted occupancy.
    if !NOISY && checkers.is_empty() {
        let layout = board.layout();
        for s in CastleSide::ALL {
            if board.can_castle(us, s)
                && let (Some(kf), Some(rf)) = (
                    layout.king_from[us.index()].get(),
                    layout.rook_from[ci(us, s)].get(),
                )
            {
                list.push(Move::new_castle(kf, rf));
            }
        }
    }

    pieces::<NOISY>(board, &ctx, &mut list);
    pawns::<NOISY>(board, &ctx, &mut list);
    list
}

/// Knights and sliders. A pinned knight has no move at all; a pinned slider
/// stays on the line through the king.
fn pieces<const NOISY: bool>(board: &Board, c: &Ctx, list: &mut MoveList) {
    // A piece's noisy moves are its moves onto enemy squares.
    let targets = if NOISY {
        c.targets & c.enemy
    } else {
        c.targets
    };
    for from in board.pieces(c.us, PieceType::Knight) & !c.pinned {
        for to in attacks::knight_attacks(from) & targets {
            list.push(capture_or_quiet(from, to, c.enemy));
        }
    }
    let queens = board.pieces(c.us, PieceType::Queen);
    for from in board.pieces(c.us, PieceType::Bishop) | queens {
        let mut to_set = attacks::bishop_attacks(from, c.occ) & targets;
        if c.pinned.contains(from) {
            to_set &= attacks::ray(c.ksq, from);
        }
        for to in to_set {
            list.push(capture_or_quiet(from, to, c.enemy));
        }
    }
    for from in board.pieces(c.us, PieceType::Rook) | queens {
        let mut to_set = attacks::rook_attacks(from, c.occ) & targets;
        if c.pinned.contains(from) {
            to_set &= attacks::ray(c.ksq, from);
        }
        for to in to_set {
            list.push(capture_or_quiet(from, to, c.enemy));
        }
    }
}

/// Pushes, captures, promotions and en passant. Noisy: a push only when it
/// promotes, no double push, every capture, every en passant.
fn pawns<const NOISY: bool>(board: &Board, c: &Ctx, list: &mut MoveList) {
    let promo_rank = Bitboard::rank(Rank::Eight.relative(c.us));
    let start_rank = Bitboard::rank(Rank::Two.relative(c.us));
    let ep = board.ep_square();
    let their_rq = board.pieces(c.them, PieceType::Rook) | board.pieces(c.them, PieceType::Queen);
    let their_bq = board.pieces(c.them, PieceType::Bishop) | board.pieces(c.them, PieceType::Queen);
    for from in board.pieces(c.us, PieceType::Pawn) {
        let pin_line = if c.pinned.contains(from) {
            attacks::ray(c.ksq, from)
        } else {
            Bitboard::FULL
        };
        let allowed = c.targets & pin_line;
        let from_bb = from.bb();

        // Pushes. The single push must be empty; the double push needs both.
        let single = from_bb.forward(c.us) & !c.occ;
        if let Some(to) = single.lsb() {
            if allowed.contains(to) {
                if promo_rank.contains(to) {
                    for p in PromoPiece::ALL {
                        list.push(Move::new_promotion(from, to, p));
                    }
                } else if !NOISY {
                    list.push(Move::new_quiet(from, to));
                }
            }
            if !NOISY
                && (from_bb & start_rank).any()
                && let Some(to2) = (single.forward(c.us) & !c.occ & allowed).lsb()
            {
                list.push(Move::new_double_push(from, to2));
            }
        }

        // Captures.
        for to in attacks::pawn_attacks(c.us, from) & c.enemy & allowed {
            if promo_rank.contains(to) {
                for p in PromoPiece::ALL {
                    list.push(Move::new_promotion_capture(from, to, p));
                }
            } else {
                list.push(Move::new_capture(from, to));
            }
        }

        // En passant: verified by explicit occupancy test, never by mask.
        if let Some(ep) = ep
            && attacks::pawn_attacks(c.us, from).contains(ep)
        {
            let captured = Square::new(ep.index() as u8 ^ 8);
            // In check, the capture helps only if the captured pawn is the
            // checker: its destination is neither the checker's
            // square nor on `between`, so the target mask is silent on it.
            let resolves_check = c.checkers.is_empty() || c.checkers == captured.bb();
            if resolves_check {
                // Both pawns off the rank, the capturer landed. Only
                // sliders need retesting: nothing else can be uncovered.
                let occ2 = c.occ.without(from).without(captured).with(ep);
                let exposed = (attacks::rook_attacks(c.ksq, occ2) & their_rq)
                    | (attacks::bishop_attacks(c.ksq, occ2) & their_bq);
                if exposed.is_empty() {
                    list.push(Move::new_en_passant(from, ep));
                }
            }
        }
    }
}

#[inline]
fn capture_or_quiet(from: Square, to: Square, enemy: Bitboard) -> Move {
    if enemy.contains(to) {
        Move::new_capture(from, to)
    } else {
        Move::new_quiet(from, to)
    }
}
