// SPDX-License-Identifier: GPL-3.0-or-later

//! The oracle side of the position gates, and a move source for walks.
//!
//! Two different things live here and the distinction matters.
//!
//! **Oracles** (`attackers_to`, `blockers_and_pinners`, `can_castle`) are
//! written from the definitions, in `(file, rank)` integer stepping, and read
//! nothing from `attacks` or `magic`. They are what the engine's answers are
//! compared with, so they must not share an implementation with it.
//!
//! **The move source** (`pseudo_legal` and `legal`) exists so that a walk
//! can happen before `generate_legal` does. It is deliberately the obvious
//! generator: piece attack sets, pawn pushes and captures, promotions, en
//! passant from the state, castling from `Board::can_castle`, then a
//! make/unmake filter for "left the king in check". It uses the crate's
//! attack tables and `attackers_to`, both of which have their own gates; a
//! bug in either does not make a walk assertion pass, it makes the walk go
//! somewhere strange, and the oracle assertions still hold there. Once
//! `generate_legal` exists this becomes an independent second opinion on it.

use cadence_core::Move;
use cadence_core::attacks;
use cadence_core::bitboard::Bitboard;
use cadence_core::castling::{CastleSide, ci};
use cadence_core::position::Board;
use cadence_core::types::{Colour, Piece, PieceType, PromoPiece, Rank, Square};

use super::generative::Rng;

// ---------------------------------------------------------------------------
// Oracle attacks, in (file, rank) integers
// ---------------------------------------------------------------------------

const ROOK_DIRS: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const BISHOP_DIRS: [(i8, i8); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
const KNIGHT: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];
const KING: [(i8, i8); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

fn coords(sq: Square) -> (i8, i8) {
    let i = i8::try_from(sq.index()).expect("fits");
    (i % 8, i / 8)
}

fn at(f: i8, r: i8) -> Option<u64> {
    ((0..8).contains(&f) && (0..8).contains(&r))
        .then(|| 1u64 << (u8::try_from(r * 8 + f).expect("in range")))
}

fn leaps(sq: Square, deltas: &[(i8, i8)]) -> u64 {
    let (f0, r0) = coords(sq);
    deltas
        .iter()
        .filter_map(|&(df, dr)| at(f0 + df, r0 + dr))
        .fold(0, |acc, b| acc | b)
}

fn slides(sq: Square, occ: u64, dirs: &[(i8, i8)]) -> u64 {
    let (f0, r0) = coords(sq);
    let mut out = 0u64;
    for &(df, dr) in dirs {
        let (mut f, mut r) = (f0 + df, r0 + dr);
        while let Some(b) = at(f, r) {
            out |= b;
            if occ & b != 0 {
                break;
            }
            f += df;
            r += dr;
        }
    }
    out
}

/// The squares `piece` on `sq` attacks under `occ`. Pawns attack diagonally
/// forward; kings and knights leap; sliders stop at the first occupied square
/// (inclusive).
#[must_use]
pub fn attacks_from(piece: Piece, sq: Square, occ: Bitboard) -> Bitboard {
    let occ = occ.0;
    Bitboard(match piece.piece_type() {
        PieceType::Pawn => match piece.colour() {
            Colour::White => leaps(sq, &[(1, 1), (-1, 1)]),
            Colour::Black => leaps(sq, &[(1, -1), (-1, -1)]),
        },
        PieceType::Knight => leaps(sq, &KNIGHT),
        PieceType::King => leaps(sq, &KING),
        PieceType::Bishop => slides(sq, occ, &BISHOP_DIRS),
        PieceType::Rook => slides(sq, occ, &ROOK_DIRS),
        PieceType::Queen => slides(sq, occ, &ROOK_DIRS) | slides(sq, occ, &BISHOP_DIRS),
    })
}

/// Every piece on the board, as `(square, piece)`.
#[must_use]
pub fn pieces(board: &Board) -> Vec<(Square, Piece)> {
    Square::all()
        .filter_map(|sq| board.piece_at(sq).map(|p| (sq, p)))
        .collect()
}

/// Brute force: every piece on the board, of either colour, that attacks
/// `sq` under `occ`. The piece sets are the board's; `occ` only decides
/// slider blocking, the same contract as `Board::attackers_to`.
#[must_use]
pub fn attackers_to(board: &Board, sq: Square, occ: Bitboard) -> Bitboard {
    let mut out = Bitboard::EMPTY;
    for (s, p) in pieces(board) {
        if attacks_from(p, s, occ).contains(sq) {
            out.set(s);
        }
    }
    out
}

/// The definition: a piece (of either colour) is a blocker for `c`'s king iff
/// removing it from the occupancy exposes that king to an enemy slider of the
/// matching type that did not attack it before; the pinner is that slider.
#[must_use]
pub fn blockers_and_pinners(board: &Board, c: Colour) -> (Bitboard, Bitboard) {
    let ksq = board.king_square(c);
    let occ = board.occupied();
    let them = c.flip();
    let mut blockers = Bitboard::EMPTY;
    let mut pinners = Bitboard::EMPTY;
    for (p_sq, _) in pieces(board) {
        if p_sq == ksq {
            continue;
        }
        let without = occ.without(p_sq);
        for (s_sq, s) in pieces(board) {
            if s.colour() != them {
                continue;
            }
            if !matches!(
                s.piece_type(),
                PieceType::Bishop | PieceType::Rook | PieceType::Queen
            ) {
                continue;
            }
            let attacks_now = attacks_from(s, s_sq, occ).contains(ksq);
            let attacks_without = attacks_from(s, s_sq, without).contains(ksq);
            if attacks_without && !attacks_now {
                blockers.set(p_sq);
                pinners.set(s_sq);
            }
        }
    }
    (blockers, pinners)
}

/// The castling-legality predicate written from the corpus's statement of
/// the rules, with
/// the oracle attackers: right held, both segments empty bar the two origins,
/// and every square of the closed king path unattacked with **both** the king
/// and the castling rook lifted.
#[must_use]
pub fn can_castle(board: &Board, c: Colour, s: CastleSide) -> bool {
    if !board.castling_rights().has(c, s) {
        return false;
    }
    let layout = board.layout();
    let i = ci(c, s);
    let (Some(kf), Some(rf), Some(kt), Some(rt)) = (
        layout.king_from[c.index()].get(),
        layout.rook_from[i].get(),
        layout.king_to[i].get(),
        layout.rook_to[i].get(),
    ) else {
        return false;
    };
    let rank = kf.rank();
    let seg = |a: Square, b: Square| -> Vec<Square> {
        let (lo, hi) = if a.file() <= b.file() { (a, b) } else { (b, a) };
        (lo.file().index()..=hi.file().index())
            .map(|f| {
                Square::from_file_rank(
                    cadence_core::types::File::new(u8::try_from(f).expect("fits")),
                    rank,
                )
            })
            .collect()
    };
    let occ = board.occupied();
    for sq in seg(kf, kt).into_iter().chain(seg(rf, rt)) {
        if sq != kf && sq != rf && occ.contains(sq) {
            return false;
        }
    }
    let lifted = occ.without(kf).without(rf);
    let them = board.by_colour(c.flip());
    seg(kf, kt)
        .into_iter()
        .all(|sq| (attackers_to(board, sq, lifted) & them).is_empty())
}

// ---------------------------------------------------------------------------
// The move source
// ---------------------------------------------------------------------------

/// Every pseudo-legal move for the side to move: the obvious generator, with
/// no legality filter except that a king is never captured.
#[must_use]
pub fn pseudo_legal(board: &Board) -> Vec<Move> {
    let us = board.side_to_move();
    let them = us.flip();
    let occ = board.occupied();
    let own = board.by_colour(us);
    let enemy = board.by_colour(them);
    let enemy_king = board.pieces(them, PieceType::King);
    let mut out = Vec::with_capacity(64);

    let push_to = |from: Square, targets: Bitboard, out: &mut Vec<Move>| {
        for to in targets & !own & !enemy_king {
            if enemy.contains(to) {
                out.push(Move::new_capture(from, to));
            } else {
                out.push(Move::new_quiet(from, to));
            }
        }
    };
    for from in board.pieces(us, PieceType::Knight) {
        push_to(from, attacks::knight_attacks(from), &mut out);
    }
    for from in board.pieces(us, PieceType::Bishop) {
        push_to(from, attacks::bishop_attacks(from, occ), &mut out);
    }
    for from in board.pieces(us, PieceType::Rook) {
        push_to(from, attacks::rook_attacks(from, occ), &mut out);
    }
    for from in board.pieces(us, PieceType::Queen) {
        push_to(from, attacks::queen_attacks(from, occ), &mut out);
    }
    for from in board.pieces(us, PieceType::King) {
        push_to(from, attacks::king_attacks(from), &mut out);
    }

    let promo_rank = Rank::Eight.relative(us);
    let start_rank = Rank::Two.relative(us);
    for from in board.pieces(us, PieceType::Pawn) {
        let single = from.bb().forward(us) & !occ;
        if let Some(to) = single.lsb() {
            if to.rank() == promo_rank {
                for p in PromoPiece::ALL {
                    out.push(Move::new_promotion(from, to, p));
                }
            } else {
                out.push(Move::new_quiet(from, to));
                if from.rank() == start_rank
                    && let Some(to2) = (single.forward(us) & !occ).lsb()
                {
                    out.push(Move::new_double_push(from, to2));
                }
            }
        }
        for to in attacks::pawn_attacks(us, from) & enemy & !enemy_king {
            if to.rank() == promo_rank {
                for p in PromoPiece::ALL {
                    out.push(Move::new_promotion_capture(from, to, p));
                }
            } else {
                out.push(Move::new_capture(from, to));
            }
        }
        if let Some(ep) = board.ep_square()
            && attacks::pawn_attacks(us, from).contains(ep)
        {
            out.push(Move::new_en_passant(from, ep));
        }
    }

    for s in CastleSide::ALL {
        if board.can_castle(us, s) {
            let kf = board.layout().king_from[us.index()]
                .get()
                .expect("king_from");
            let rf = board.layout().rook_from[ci(us, s)]
                .get()
                .expect("rook_from");
            out.push(Move::new_castle(kf, rf));
        }
    }
    out
}

/// The pseudo-legal moves that do not leave the mover's king attacked,
/// decided by making each one and asking the crate's `attackers_to`.
#[must_use]
pub fn legal(board: &mut Board) -> Vec<Move> {
    let us = board.side_to_move();
    let them = us.flip();
    pseudo_legal(board)
        .into_iter()
        .filter(|m| {
            board.make_move(*m);
            let ksq = board.king_square(us);
            let safe =
                (board.attackers_to(ksq, board.occupied()) & board.by_colour(them)).is_empty();
            board.unmake_move(*m);
            safe
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Random placements
// ---------------------------------------------------------------------------

/// A random legal-looking placement as a FEN: one king each, not adjacent;
/// up to fifteen other pieces per side with pawns off the back ranks; random
/// side to move; no castling rights; no en-passant square. Nothing is done
/// about the side not to move being in check: attackers, blockers and
/// pinners are defined regardless, and that is what these feed. About a
/// quarter of them are that way, which is what `tests/opponent_in_check.rs`
/// filters for.
#[must_use]
pub fn random_placement_fen(rng: &mut Rng) -> String {
    placement_fen(rng, false)
}

/// The same, with the two kings deliberately adjacent.
///
/// A separate generator rather than a relaxed one, because the case is worth
/// reaching on purpose and the unbiased placement above reaches it with
/// probability about 1/64. Touching kings are the sharp corner of "the side
/// not to move is in check": each king attacks the other, so it holds
/// whichever side is to move, and the enemy king appears in the attacker set
/// that decides check as well as in the target sets that decide moves.
#[must_use]
pub fn random_touching_kings_fen(rng: &mut Rng) -> String {
    placement_fen(rng, true)
}

fn placement_fen(rng: &mut Rng, kings_touch: bool) -> String {
    let mut board: [Option<Piece>; 64] = [None; 64];
    let wk = Square::new(u8::try_from(rng.below(64)).expect("fits"));
    let bk = loop {
        let sq = Square::new(u8::try_from(rng.below(64)).expect("fits"));
        let (f1, r1) = coords(wk);
        let (f2, r2) = coords(sq);
        let adjacent = (f1 - f2).abs() <= 1 && (r1 - r2).abs() <= 1;
        if sq != wk && adjacent == kings_touch {
            break sq;
        }
    };
    board[wk.index()] = Some(Piece::WKing);
    board[bk.index()] = Some(Piece::BKing);

    let n = rng.below(31);
    let menu = [
        PieceType::Pawn,
        PieceType::Pawn,
        PieceType::Pawn,
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
    ];
    let mut placed = 0;
    let mut tries = 0;
    while placed < n && tries < 400 {
        tries += 1;
        let sq = Square::new(u8::try_from(rng.below(64)).expect("fits"));
        if board[sq.index()].is_some() {
            continue;
        }
        let pt = menu[rng.below(menu.len())];
        if pt == PieceType::Pawn && (sq.rank() == Rank::One || sq.rank() == Rank::Eight) {
            continue;
        }
        let c = if rng.below(2) == 0 {
            Colour::White
        } else {
            Colour::Black
        };
        board[sq.index()] = Some(Piece::new(c, pt));
        placed += 1;
    }

    let mut fen = String::new();
    for r in (0..8).rev() {
        let mut empty = 0;
        for f in 0..8 {
            match board[r * 8 + f] {
                Some(p) => {
                    if empty > 0 {
                        fen.push_str(&empty.to_string());
                        empty = 0;
                    }
                    fen.push(p.to_char());
                }
                None => empty += 1,
            }
        }
        if empty > 0 {
            fen.push_str(&empty.to_string());
        }
        if r > 0 {
            fen.push('/');
        }
    }
    let stm = if rng.below(2) == 0 { 'w' } else { 'b' };
    format!("{fen} {stm} - - 0 1")
}
