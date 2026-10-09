// SPDX-License-Identifier: GPL-3.0-or-later

//! Legal, not pseudo-legal: nothing downstream filters the list.

use crate::attacks;
use crate::bitboard::Bitboard;
use crate::castling::{CastleSide, ci};
use crate::mv::{Move, MoveList};
use crate::position::Board;
use crate::types::{PieceType, PromoPiece, Rank, Square};

struct Ctx {
    us: crate::types::Colour,
    them: crate::types::Colour,
    occ: Bitboard,
    enemy: Bitboard,
    ksq: Square,
    checkers: Bitboard,
    pinned: Bitboard,
    /// In single check, the checker and the squares between it and the king.
    targets: Bitboard,
}

/// In no defined order.
#[must_use]
pub fn generate_legal(board: &Board) -> MoveList {
    generate::<false, false>(board)
}

/// The noisy subsequence of [`generate_legal`], in its order.
#[must_use]
pub fn generate_noisy(board: &Board) -> MoveList {
    generate::<true, false>(board)
}

/// Stops at the first block of the generator that yields a move.
#[must_use]
pub fn has_legal_move(board: &Board) -> bool {
    !generate::<false, true>(board).is_empty()
}

/// Both generators walk the branches in one order, which is what makes the noisy list a
/// subsequence. `ANY` returns at a block boundary once the list holds a move.
fn generate<const NOISY: bool, const ANY: bool>(board: &Board) -> MoveList {
    let mut list = MoveList::new();
    let us = board.side_to_move();
    let them = us.flip();
    let occ = board.occupied();
    let own = board.by_colour(us);
    let enemy = board.by_colour(them);
    let ksq = board.king_square(us);
    let checkers = board.checkers();
    // A king is never a target; in legal play the mask removes nothing.
    let not_a_king = !board.pieces(them, PieceType::King);

    let occ_without_king = occ.without(ksq);
    let king_targets = (if NOISY { enemy } else { !own }) & not_a_king;
    for to in attacks::king_attacks(ksq) & king_targets {
        if (board.attackers_to(to, occ_without_king) & enemy).is_empty() {
            list.push(capture_or_quiet(ksq, to, enemy));
        }
    }

    // Double check: only the king can help.
    if checkers.more_than_one() || (ANY && !list.is_empty()) {
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

    // Never noisy: the destination holds our own rook.
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

    pieces::<NOISY, ANY>(board, &ctx, &mut list);
    if ANY && !list.is_empty() {
        return list;
    }
    pawns::<NOISY>(board, &ctx, &mut list);
    list
}

fn pieces<const NOISY: bool, const ANY: bool>(board: &Board, c: &Ctx, list: &mut MoveList) {
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
    if ANY && !list.is_empty() {
        return;
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
    if ANY && !list.is_empty() {
        return;
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

/// Noisy: promoting pushes, captures and en passant.
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

        for to in attacks::pawn_attacks(c.us, from) & c.enemy & allowed {
            if promo_rank.contains(to) {
                for p in PromoPiece::ALL {
                    list.push(Move::new_promotion_capture(from, to, p));
                }
            } else {
                list.push(Move::new_capture(from, to));
            }
        }

        // En passant is verified by occupancy, never by mask.
        if let Some(ep) = ep
            && attacks::pawn_attacks(c.us, from).contains(ep)
        {
            let captured = Square::new(ep.index() as u8 ^ 8);
            // In check it helps only if the captured pawn is the checker, which the target mask cannot see.
            let resolves_check = c.checkers.is_empty() || c.checkers == captured.bb();
            if resolves_check {
                // Only sliders can be uncovered.
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
