// SPDX-License-Identifier: GPL-3.0-or-later

//! Oracles are written from the definitions in `(file, rank)` stepping and read nothing from
//! `attacks`, `magic` or the board's own move making, so they share no implementation with the
//! engine. The move source is the obvious generator over them, deciding legality on a copy of the
//! placement, an independent second opinion on `generate_legal`.

use cadence_core::Move;
use cadence_core::bitboard::Bitboard;
use cadence_core::castling::{CastleSide, ci};
use cadence_core::position::Board;
use cadence_core::types::{Colour, File, Piece, PieceType, PromoPiece, Rank, Square};

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

/// Sliders stop at the first occupied square, inclusive.
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

#[must_use]
pub fn pieces(board: &Board) -> Vec<(Square, Piece)> {
    Square::all()
        .filter_map(|sq| board.piece_at(sq).map(|p| (sq, p)))
        .collect()
}

/// Brute force, either colour; `occ` only decides slider blocking, as in `Board::attackers_to`.
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

/// A piece of either colour is a blocker iff removing it exposes `c`'s king to an enemy slider that
/// did not attack it before; the pinner is that slider.
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

/// From the corpus's statement of the rules: every square of the closed king path unattacked with
/// both the king and the castling rook lifted.
#[must_use]
pub fn can_castle(board: &Board, c: Colour, s: CastleSide) -> bool {
    if !board.castling_rights().has(c, s) {
        return false;
    }
    let layout = board.layout();
    let (Some(kf), Some(rf)) = (
        layout.king_from[c.index()].get(),
        layout.rook_from[ci(c, s)].get(),
    ) else {
        return false;
    };
    let rank = kf.rank();
    let (kt, rt) = destinations(s, rank);
    let seg = |a: Square, b: Square| -> Vec<Square> {
        let (lo, hi) = if a.file() <= b.file() { (a, b) } else { (b, a) };
        (lo.file().index()..=hi.file().index())
            .map(|f| Square::from_file_rank(File::new(u8::try_from(f).expect("fits")), rank))
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

/// The king's and the rook's squares after castling, from the rules: the king on the g- or c-file
/// and the rook beside it, whatever the start array.
fn destinations(s: CastleSide, rank: Rank) -> (Square, Square) {
    let (king, rook) = match s {
        CastleSide::King => (File::G, File::F),
        CastleSide::Queen => (File::C, File::D),
    };
    (
        Square::from_file_rank(king, rank),
        Square::from_file_rank(rook, rank),
    )
}

// ---------------------------------------------------------------------------
// The move source
// ---------------------------------------------------------------------------

/// No legality filter, except that a king is never captured.
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
    for pt in [
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
        PieceType::King,
    ] {
        for from in board.pieces(us, pt) {
            push_to(from, attacks_from(Piece::new(us, pt), from, occ), &mut out);
        }
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
        let takes = attacks_from(Piece::new(us, PieceType::Pawn), from, occ);
        for to in takes & enemy & !enemy_king {
            if to.rank() == promo_rank {
                for p in PromoPiece::ALL {
                    out.push(Move::new_promotion_capture(from, to, p));
                }
            } else {
                out.push(Move::new_capture(from, to));
            }
        }
        if let Some(ep) = board.ep_square()
            && takes.contains(ep)
        {
            out.push(Move::new_en_passant(from, ep));
        }
    }

    for s in CastleSide::ALL {
        if can_castle(board, us, s) {
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

/// Decided on a copy of the placement with the move applied from the rules, its king's safety read
/// from the oracle attacks.
#[must_use]
pub fn legal(board: &Board) -> Vec<Move> {
    pseudo_legal(board)
        .into_iter()
        .filter(|&m| king_safe_after(board, m))
        .collect()
}

/// Whether the mover's king is unattacked once `m` is applied to a copy of the placement: castling
/// puts the king and the rook on their destinations, en passant lifts the pawn beside the capturing
/// one, and a promotion places the new piece.
fn king_safe_after(board: &Board, m: Move) -> bool {
    let us = board.side_to_move();
    let mut cells: [Option<Piece>; 64] = [None; 64];
    for (sq, p) in pieces(board) {
        cells[sq.index()] = Some(p);
    }
    let (from, to) = (m.from_sq(), m.to_sq());
    let moving = cells[from.index()]
        .take()
        .expect("a piece on the from square");
    if m.is_castle() {
        cells[to.index()] = None;
        let (kt, rt) = destinations(m.castle_side(), from.rank());
        cells[kt.index()] = Some(Piece::new(us, PieceType::King));
        cells[rt.index()] = Some(Piece::new(us, PieceType::Rook));
    } else {
        if m.is_en_passant() {
            cells[Square::from_file_rank(to.file(), from.rank()).index()] = None;
        }
        cells[to.index()] = Some(match m.promotion_piece() {
            Some(p) => Piece::new(us, p.piece_type()),
            None => moving,
        });
    }
    let king = Piece::new(us, PieceType::King);
    let ksq = Square::all()
        .find(|sq| cells[sq.index()] == Some(king))
        .expect("a king");
    let mut occ = Bitboard::EMPTY;
    for sq in Square::all() {
        if cells[sq.index()].is_some() {
            occ.set(sq);
        }
    }
    for sq in Square::all() {
        if let Some(p) = cells[sq.index()]
            && p.colour() != us
            && attacks_from(p, sq, occ).contains(ksq)
        {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Random placements
// ---------------------------------------------------------------------------

/// Kings not adjacent, no castling, no en passant, and the side not to move left in check about a
/// quarter of the time, which `tests/opponent_in_check.rs` filters for.
#[must_use]
pub fn random_placement_fen(rng: &mut Rng) -> String {
    placement_fen(rng, false)
}

/// Separate because the unbiased placement reaches it about 1 in 64. Touching kings put each king
/// in check whichever side is to move.
#[must_use]
pub fn random_touching_kings_fen(rng: &mut Rng) -> String {
    placement_fen(rng, true)
}

/// Whether one more `pt` for `c` leaves material that promotion from eight pawns can give, which
/// `from_fen` requires.
fn promotion_allows(cells: &[Option<Piece>; 64], c: Colour, pt: PieceType) -> bool {
    let mut count = [0usize; 6];
    for p in cells.iter().flatten().filter(|p| p.colour() == c) {
        count[p.piece_type().index()] += 1;
    }
    count[pt.index()] += 1;
    let promoted: usize = [
        (PieceType::Queen, 1),
        (PieceType::Rook, 2),
        (PieceType::Bishop, 2),
        (PieceType::Knight, 2),
    ]
    .into_iter()
    .map(|(t, start)| count[t.index()].saturating_sub(start))
    .sum();
    count[PieceType::Pawn.index()] + promoted <= 8
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
        if !promotion_allows(&board, c, pt) {
            continue;
        }
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
