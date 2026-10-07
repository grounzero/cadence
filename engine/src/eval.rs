// SPDX-License-Identifier: GPL-3.0-or-later

//! Integer and symmetric by construction: tables are written from White's side and flipped for
//! Black, so a position and its mirror differ only in sign.

use cadence_core::position::Board;
use cadence_core::{Bitboard, Colour, PieceType, Square, attacks};

use crate::score::{MAX_EVAL, Score};

/// The start position's minor and major pieces; zero is a pawn ending.
pub const PHASE_MAX: i32 = 24;

const PHASE_WEIGHT: [i32; 6] = [0, 1, 1, 2, 4, 0];
const _: () = assert!(
    2 * (2 * PHASE_WEIGHT[1] + 2 * PHASE_WEIGHT[2] + 2 * PHASE_WEIGHT[3] + PHASE_WEIGHT[4])
        == PHASE_MAX
);

/// The unit every weight is stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub mg: i32,
    pub eg: i32,
}

/// One per piece type, by `PieceType::index`.
pub const MATERIAL: usize = 0;

/// `64 * piece type + square`, from White's side.
pub const PST: usize = MATERIAL + 6;

/// One per rank from its own side, second to seventh.
pub const PASSED: usize = PST + 6 * 64;

/// A pawn with no friendly pawn on an adjacent file.
pub const ISOLATED: usize = PASSED + 6;

/// Each pawn beyond the first of its colour on a file.
pub const DOUBLED: usize = ISOLATED + 1;

/// A pawn defended by a friendly pawn, or beside one on its rank.
pub const CONNECTED: usize = DOUBLED + 1;

/// One per count of usable squares: knight, bishop, rook, queen in turn.
pub const MOBILITY: usize = CONNECTED + 1;

/// By `PieceType::index`.
pub const MOBILITY_LEN: [usize; 6] = [0, 9, 14, 15, 28, 0];

/// From [`MOBILITY`]; the last entry is the total.
pub const MOBILITY_OFFSET: [usize; 6] = {
    let mut out = [0; 6];
    let mut i = 1;
    while i < 6 {
        out[i] = out[i - 1] + MOBILITY_LEN[i - 1];
        i += 1;
    }
    out
};

/// One per count of a side's knights, bishops, rooks and queens attacking the enemy king's zone.
pub const ATTACKERS: usize = MOBILITY + MOBILITY_OFFSET[5] + MOBILITY_LEN[5];

/// Four or more share the last: the training data thins below the tuner's floor there, so a rarer
/// count takes the last fitted entry rather than an average one.
pub const ATTACKERS_LEN: usize = 5;

/// One per count of a king's own pawns ahead of it on its file and the two beside it.
pub const SHIELD: usize = ATTACKERS + ATTACKERS_LEN;

/// Four or more share the last, for `ATTACKERS_LEN`'s reason.
pub const SHIELD_LEN: usize = 5;

pub const WEIGHT_COUNT: usize = SHIELD + SHIELD_LEN;

/// Fitted by `cadence texel` to self-play results; a weight carries no reason beyond the data.
#[rustfmt::skip]
pub static WEIGHTS: [Pair; WEIGHT_COUNT] = [
    // material, fitted: pawn, knight, bishop, rook, queen, king
    p(  64,   71), p( 321,  252), p( 368,  279), p( 479,  502), p(1008,  940), p(   0,    0),
    // pawn, fitted, rank 1 to rank 8, a-file first
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p( -15,   17), p(  -3,    0), p(  -9,   15), p(  -6,   14), p(  -2,   12), p(   2,   10), p(  -4,   12), p( -19,   21),
    p( -15,   11), p(  -1,    5), p(  -6,   -1), p(   2,    1), p(   5,  -10), p(  -3,   -6), p(  -1,    8), p( -19,   10),
    p( -13,   15), p(  -7,    7), p(   8,   -3), p(  13,  -15), p(  13,  -16), p(  13,   -6), p( -10,   11), p( -17,   18),
    p(   3,   30), p(   4,   18), p(  17,    4), p(  34,   -8), p(  26,  -18), p(  26,    2), p(  10,   17), p(  -7,   26),
    p(  12,   46), p(  14,   41), p(  22,   31), p(  33,   23), p(  28,   23), p(  38,   30), p(  21,   54), p(   6,   53),
    p(  28,   82), p(  40,   84), p(  30,   76), p(  36,   70), p(  39,   60), p(  44,   73), p(  43,   90), p(  35,   96),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    // knight, fitted, rank 1 to rank 8, a-file first
    p( -34,  -30), p( -20,  -21), p( -19,  -10), p(  -2,   -6), p(  -1,   -2), p( -12,  -10), p( -15,  -19), p( -40,  -37),
    p( -24,  -12), p(  -1,  -10), p(  10,  -11), p(  21,   -8), p(  15,    3), p(  -6,  -12), p(   1,  -30), p( -25,  -16),
    p( -17,  -22), p(  -3,  -13), p(   4,   12), p(  12,   15), p(  10,   18), p(   1,   13), p(  -5,    1), p( -25,  -14),
    p(   6,   -9), p(  12,    4), p(  16,   21), p(  17,   16), p(  13,   19), p(  14,   26), p(   0,    7), p(  13,   -4),
    p(   1,  -11), p(  15,    8), p(  24,   24), p(  35,   15), p(  33,   16), p(  20,   25), p(   9,    4), p(  10,   -8),
    p(  -8,   -6), p(  10,    6), p(  46,   29), p(  30,   12), p(  40,   29), p(  24,   17), p(  15,   -6), p(  -7,   -7),
    p( -39,  -23), p(  -9,   -9), p(  13,    3), p(  32,    1), p(  10,    7), p(   5,   -2), p(  -6,   -6), p( -17,  -20),
    p( -34,  -29), p( -25,  -16), p( -17,   -8), p( -19,  -12), p(  -9,   -6), p( -20,  -14), p( -29,  -19), p( -50,  -36),
    // bishop, fitted, rank 1 to rank 8, a-file first
    p(  -2,  -26), p(   0,  -11), p(  16,   -5), p(  24,   -8), p(  13,   -3), p(  15,   -8), p(  -1,   -5), p(   2,  -20),
    p(   1,  -19), p(   8,  -17), p(  10,    4), p(  13,   -1), p(  16,   -7), p(  14,   -7), p(   8,  -15), p(   7,  -11),
    p(  -3,   -5), p(  -1,    1), p(  -2,   17), p(   9,    5), p(  11,    3), p(   4,   10), p(   3,   -2), p(   7,  -15),
    p(  -7,    5), p( -13,    7), p(  10,    3), p(  10,   13), p(   9,   13), p(   0,   11), p(  -2,    5), p( -12,   -5),
    p(  -3,   -3), p(   5,   14), p(   5,   12), p(  26,   14), p(  29,    4), p(  25,   -1), p(  -7,    9), p(  -5,   -5),
    p( -11,   -8), p(  20,    1), p(  29,   11), p(  10,   -1), p(  25,    4), p(  22,    5), p(   9,    2), p(  -3,   -2),
    p( -15,  -10), p(   7,    7), p( -10,   -4), p(  -3,    8), p( -11,   -8), p(  -6,    4), p(  -3,   10), p( -13,  -10),
    p( -11,   -6), p(  -8,   -3), p( -10,   -5), p( -25,  -20), p( -13,   -2), p( -23,   -7), p( -12,   -8), p( -13,   -8),
    // rook, fitted, rank 1 to rank 8, a-file first
    p( -40,   -3), p( -10,  -15), p(  15,  -19), p(  21,  -12), p(  22,  -12), p(  19,  -14), p(   2,   -7), p( -31,   -1),
    p( -37,   -6), p(  -1,  -24), p(  10,  -11), p(   7,  -10), p(   8,  -13), p(   0,  -13), p(   2,  -16), p( -32,   -4),
    p( -25,   -8), p(  -3,   -8), p(  -9,    3), p( -13,    3), p(  -1,   -4), p(  -7,    1), p( -17,   -3), p( -32,   -5),
    p( -21,    2), p( -12,    3), p(  -5,   12), p( -13,   -1), p( -15,    3), p( -11,   12), p( -12,    0), p( -19,    2),
    p(   1,   -1), p(   7,    8), p(   8,   16), p(   6,   11), p(  13,    4), p(  12,    8), p( -12,    7), p( -14,    5),
    p(   2,    6), p(   8,    6), p(  21,   17), p(  25,    9), p(  22,    9), p(  29,    7), p(   3,    5), p(   8,   10),
    p(  17,   -7), p(  30,   -7), p(  37,   -5), p(  34,   -7), p(  32,   -8), p(  44,   -5), p(  29,   -1), p(  28,    2),
    p(   3,   17), p(   8,   12), p(   8,   13), p(  12,   16), p(   7,    7), p(  10,   18), p(  13,   20), p(  11,   29),
    // queen, fitted, rank 1 to rank 8, a-file first
    p( -21,  -44), p( -19,  -28), p(   9,  -45), p(   7,  -43), p(   9,  -33), p(   3,  -41), p(  -9,  -35), p( -20,  -32),
    p( -26,  -16), p(  -3,  -14), p(  -7,   -4), p(   6,  -25), p(   5,  -10), p(   7,  -24), p(  -9,   -7), p( -14,   -8),
    p( -14,    1), p( -18,    1), p(  -5,   12), p( -14,    5), p( -13,    9), p(  -7,   11), p(  -6,    6), p( -15,   -7),
    p( -10,    2), p( -21,   10), p(  -9,   16), p( -15,   25), p(  -5,   14), p(  -8,   12), p(  -7,   12), p( -15,    8),
    p(  -8,   15), p(  -8,   15), p( -13,   23), p(   5,   23), p(   6,   24), p(   2,   20), p(   4,   10), p( -10,    9),
    p(  10,    3), p(   7,   11), p(  24,   19), p(  24,   35), p(  21,   20), p(  13,   15), p(   2,    9), p(   5,   11),
    p(  -7,    3), p(   8,    4), p(  -1,   13), p(  16,   19), p(  -9,   15), p(  11,   12), p(  -3,    0), p(  -3,    2),
    p(  -7,   -9), p(   0,   -1), p(   9,    4), p(   3,    7), p(   7,   10), p(  -2,    3), p(  -1,   -3), p(  -6,   -7),
    // king, fitted, rank 1 to rank 8, a-file first
    p(  -7,  -33), p(  -3,  -21), p( -15,  -13), p( -30,  -15), p( -25,  -13), p( -23,  -23), p(  15,  -32), p(  17,  -48),
    p( -13,  -16), p( -14,   -5), p( -32,    1), p( -30,   -4), p( -41,    1), p( -35,    4), p( -29,   -5), p(  -9,  -19),
    p( -68,   -7), p( -54,    9), p( -64,   12), p( -83,    7), p( -64,    4), p( -69,   14), p( -57,    3), p( -66,   -5),
    p( -85,   -6), p( -77,   15), p( -84,    9), p( -77,    6), p( -84,    5), p( -85,   13), p( -88,    5), p( -88,   -3),
    p(-110,    0), p(-108,   16), p(-104,   14), p(-106,   13), p(-109,    8), p(-109,   22), p(-111,   20), p(-113,    4),
    p(-137,   -2), p(-133,   26), p(-134,   35), p(-136,   28), p(-133,   32), p(-136,   33), p(-132,   31), p(-137,    6),
    p(-163,   -5), p(-163,    6), p(-163,   12), p(-163,   20), p(-163,   17), p(-163,   10), p(-163,    8), p(-163,   -7),
    p(-188,  -16), p(-188,  -10), p(-188,    0), p(-188,    2), p(-188,    3), p(-188,   -4), p(-188,  -11), p(-188,  -16),
    // pawn structure, fitted: passed on ranks 2 to 7, isolated, doubled, connected
    p(   3,    9), p(  -1,   23), p(  -6,   47), p(  15,   61), p(  40,   62), p(  48,   60),
    p(  -6,   -5), p(  -4,  -15), p(   6,    6),
    // mobility by count of usable squares, fitted: knight 0 to 8, bishop 0 to 13, rook 0 to 14,
    // queen 0 to 27
    p( -39,  -34), p( -17,   -9), p(  -8,   -6), p(  -3,   10), p(   2,   15), p(   5,   22), p(  14,   21), p(  22,    2),
    p(  22,  -23),
    p( -56,  -82), p( -28,  -39), p( -12,  -15), p(  -5,    2), p(   0,    6), p(   4,   15), p(   7,   19), p(  16,   17),
    p(  18,   21), p(  21,   19), p(  22,   20), p(  10,   22), p(   8,    5), p(  -8,  -14),
    p( -62,  -62), p( -44,  -40), p( -37,  -29), p( -32,   -5), p( -19,    1), p( -13,   10), p(  -4,   13), p(   2,   18),
    p(  11,   17), p(  19,   14), p(  30,   12), p(  31,   22), p(  31,   19), p(  26,   17), p(  60,  -12),
    p( -76,  -33), p( -45,  -39), p( -31,  -35), p( -18,  -45), p( -14,  -29), p( -12,  -14), p(  -9,  -11), p(  -7,    0),
    p(  -4,   13), p(   1,   20), p(   3,   20), p(   6,   31), p(  14,   35), p(  14,   36), p(  23,   30), p(  29,   34),
    p(  28,   31), p(  33,   29), p(  28,   22), p(  30,   17), p(  21,    8), p(  12,  -11), p(   2,  -10), p(  -5,  -24),
    p(  -9,  -24), p( -11,  -28), p(   0,   -9), p(   0,  -13),
    // king safety, fitted: attackers on the enemy king's zone 0 to 4, then pawns shielding the king
    // 0 to 4, four or more sharing the last of each
    p( -49,   -4), p( -45,    5), p( -10,  -21), p(  38,    4), p(  66,   16),
    p( -67,   41), p( -24,   13), p(   9,  -12), p(  42,  -33), p(  40,   -9),
];

const fn p(mg: i32, eg: i32) -> Pair {
    Pair { mg, eg }
}

/// # Panics
///
/// If `index` is not below [`WEIGHT_COUNT`].
#[must_use]
pub fn weight_name(index: usize) -> String {
    const NAMES: [&str; 6] = ["pawn", "knight", "bishop", "rook", "queen", "king"];
    assert!(index < WEIGHT_COUNT, "weight {index} of {WEIGHT_COUNT}");
    if index < PST {
        format!("material.{}", NAMES[index - MATERIAL])
    } else if index >= SHIELD {
        format!("shield.{}", index - SHIELD)
    } else if index >= ATTACKERS {
        format!("attackers.{}", index - ATTACKERS)
    } else if index >= MOBILITY {
        let at = index - MOBILITY;
        let pt = (1..5).rfind(|&pt| MOBILITY_OFFSET[pt] <= at).unwrap_or(1);
        format!("mobility.{}.{}", NAMES[pt], at - MOBILITY_OFFSET[pt])
    } else if index >= CONNECTED {
        "connected".to_string()
    } else if index >= DOUBLED {
        "doubled".to_string()
    } else if index >= ISOLATED {
        "isolated".to_string()
    } else if index >= PASSED {
        format!("passed.r{}", index - PASSED + 2)
    } else {
        let (pt, sq) = ((index - PST) / 64, (index - PST) % 64);
        let square = Square::new(sq as u8);
        format!("pst.{}.{square}", NAMES[pt])
    }
}

// --- the evaluation --------------------------------------------------------

#[must_use]
pub fn phase(board: &Board) -> i32 {
    let mut phase = 0;
    for pt in PieceType::ALL {
        let n = board.by_type(pt).count();
        phase += PHASE_WEIGHT[pt.index()] * i32::try_from(n).unwrap_or(i32::MAX / 8);
    }
    phase.min(PHASE_MAX)
}

/// [`evaluate`] and [`trace`] run one walk, so the search and the tuner cannot disagree about what
/// is evaluated.
pub trait Sink {
    /// Negative for Black.
    fn add(&mut self, index: usize, count: i32);
}

struct Sum {
    mg: i32,
    eg: i32,
}

impl Sink for Sum {
    #[inline(always)]
    fn add(&mut self, index: usize, count: i32) {
        let w = WEIGHTS[index];
        self.mg += count * w.mg;
        self.eg += count * w.eg;
    }
}

/// The evaluation before its clamp is these coefficients dotted with [`WEIGHTS`], blended by
/// `phase`.
#[derive(Clone, Debug)]
pub struct Trace {
    pub coefficients: [i32; WEIGHT_COUNT],
    pub phase: i32,
}

impl Sink for Trace {
    #[inline]
    fn add(&mut self, index: usize, count: i32) {
        self.coefficients[index] += count;
    }
}

/// Returns the phase.
#[inline(always)]
fn terms<S: Sink>(board: &Board, sink: &mut S) -> i32 {
    let mut phase = 0;
    for pt in PieceType::ALL {
        let i = pt.index();
        for sq in board.pieces(Colour::White, pt) {
            sink.add(MATERIAL + i, 1);
            sink.add(PST + 64 * i + sq.index(), 1);
            phase += PHASE_WEIGHT[i];
        }
        for sq in board.pieces(Colour::Black, pt) {
            sink.add(MATERIAL + i, -1);
            sink.add(PST + 64 * i + sq.flip_vertical().index(), -1);
            phase += PHASE_WEIGHT[i];
        }
    }
    pawn_structure(board, Colour::White, 1, sink);
    pawn_structure(board, Colour::Black, -1, sink);
    piece_terms(board, Colour::White, 1, sink);
    piece_terms(board, Colour::Black, -1, sink);
    shield(board, Colour::White, 1, sink);
    shield(board, Colour::Black, -1, sink);
    phase.min(PHASE_MAX)
}

/// Ahead on its own file and both neighbours: a pawn is passed if no enemy pawn stands there.
static PASSED_MASKS: [[Bitboard; 64]; 2] = passed_masks();

const fn passed_masks() -> [[Bitboard; 64]; 2] {
    let mut out = [[Bitboard(0); 64]; 2];
    let mut sq = 0;
    while sq < 64 {
        let (file, rank) = (sq % 8, sq / 8);
        let mut bits = [0u64; 2];
        let mut f = if file > 0 { file - 1 } else { 0 };
        while f <= file + 1 && f < 8 {
            let mut r = 0;
            while r < 8 {
                if r > rank {
                    bits[0] |= 1 << (8 * r + f);
                }
                if r < rank {
                    bits[1] |= 1 << (8 * r + f);
                }
                r += 1;
            }
            f += 1;
        }
        out[0][sq] = Bitboard(bits[0]);
        out[1][sq] = Bitboard(bits[1]);
        sq += 1;
    }
    out
}

const ADJACENT_FILES: [Bitboard; 8] = {
    let mut out = [Bitboard(0); 8];
    let mut f = 0;
    while f < 8 {
        let file = Bitboard::FILE_A.0 << f;
        out[f] =
            Bitboard(((file << 1) & !Bitboard::FILE_A.0) | ((file >> 1) & !Bitboard::FILE_H.0));
        f += 1;
    }
    out
};

/// `sign` is one for White, minus one for Black.
#[inline(always)]
fn pawn_structure<S: Sink>(board: &Board, colour: Colour, sign: i32, sink: &mut S) {
    let own = board.pieces(colour, PieceType::Pawn);
    let enemy = board.pieces(colour.flip(), PieceType::Pawn);
    let beside = own.east() | own.west();
    for sq in own {
        let f = sq.file().index();
        if (PASSED_MASKS[colour.index()][sq.index()] & enemy).is_empty() {
            let rank = match colour {
                Colour::White => sq.index() / 8,
                Colour::Black => 7 - sq.index() / 8,
            };
            sink.add(PASSED + rank - 1, sign);
        }
        if (ADJACENT_FILES[f] & own).is_empty() {
            sink.add(ISOLATED, sign);
        }
        if (attacks::pawn_attacks(colour.flip(), sq) & own).any() || beside.contains(sq) {
            sink.add(CONNECTED, sign);
        }
    }
    for f in 0..8 {
        let on_file = (own & Bitboard(Bitboard::FILE_A.0 << f)).count();
        if on_file > 1 {
            sink.add(DOUBLED, sign * (i32::try_from(on_file).unwrap_or(8) - 1));
        }
    }
}

/// The one walk over those attacks, under full occupancy; each term applies its own mask.
#[inline(always)]
fn piece_attacks(board: &Board, colour: Colour, mut visit: impl FnMut(PieceType, Bitboard)) {
    let occupied = board.occupied();
    for pt in [
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
    ] {
        for sq in board.pieces(colour, pt) {
            let reach = match pt {
                PieceType::Knight => attacks::knight_attacks(sq),
                PieceType::Bishop => attacks::bishop_attacks(sq, occupied),
                PieceType::Rook => attacks::rook_attacks(sq, occupied),
                _ => attacks::queen_attacks(sq, occupied),
            };
            visit(pt, reach);
        }
    }
}

/// Mobility counts attacked squares holding none of the piece's own side and no enemy pawn attack;
/// the attack term counts pieces reaching the enemy king's zone.
#[inline(always)]
fn piece_terms<S: Sink>(board: &Board, colour: Colour, sign: i32, sink: &mut S) {
    let enemy = colour.flip();
    let enemy_pawns = board.pieces(enemy, PieceType::Pawn);
    let area = !board.by_colour(colour) & !attacks::pawn_attacks_bb(enemy, enemy_pawns);
    let zone = KING_ZONES[enemy.index()][board.king_square(enemy).index()];
    let mut attackers = 0;
    piece_attacks(board, colour, |pt, reach| {
        let table = MOBILITY + MOBILITY_OFFSET[pt.index()];
        sink.add(table + (reach & area).count() as usize, sign);
        attackers += usize::from((reach & zone).any());
    });
    sink.add(ATTACKERS + attackers.min(ATTACKERS_LEN - 1), sign);
}

/// The king's square and the eight around it, plus the three in front of those for a king on its
/// back two ranks.
static KING_ZONES: [[Bitboard; 64]; 2] = king_zones();

const fn king_zones() -> [[Bitboard; 64]; 2] {
    let mut out = [[Bitboard(0); 64]; 2];
    let mut sq = 0;
    while sq < 64 {
        let (file, rank) = (sq % 8, sq / 8);
        let (low, high) = (
            if file > 0 { file - 1 } else { 0 },
            if file < 7 { file + 1 } else { 7 },
        );
        let mut around = 0u64;
        let mut r = if rank > 0 { rank - 1 } else { 0 };
        while r <= rank + 1 && r < 8 {
            let mut f = low;
            while f <= high {
                around |= 1 << (8 * r + f);
                f += 1;
            }
            r += 1;
        }
        let fronts = [
            if rank <= 1 { Some(rank + 2) } else { None },
            if rank >= 6 { Some(rank - 2) } else { None },
        ];
        let mut colour = 0;
        while colour < 2 {
            let mut bits = around;
            if let Some(front) = fronts[colour] {
                let mut f = low;
                while f <= high {
                    bits |= 1 << (8 * front + f);
                    f += 1;
                }
            }
            out[colour][sq] = Bitboard(bits);
            colour += 1;
        }
        sq += 1;
    }
    out
}

#[inline(always)]
fn shield<S: Sink>(board: &Board, colour: Colour, sign: i32, sink: &mut S) {
    let king = board.king_square(colour);
    let ahead = PASSED_MASKS[colour.index()][king.index()];
    let count = (board.pieces(colour, PieceType::Pawn) & ahead).count() as usize;
    sink.add(SHIELD + count.min(SHIELD_LEN - 1), sign);
}

/// From the side to move's point of view, strictly inside `(-MAX_EVAL, MAX_EVAL)`.
#[must_use]
pub fn evaluate(board: &Board) -> Score {
    let mut sum = Sum { mg: 0, eg: 0 };
    let phase = terms(board, &mut sum);
    // Truncating division: symmetric under negation, so the mirror of a position evaluates to
    // the exact negative.
    let white = (sum.mg * phase + sum.eg * (PHASE_MAX - phase)) / PHASE_MAX;
    // `from_fen` accepts sixty queens, which must still not reach the mate scale.
    let white = white.clamp(-MAX_EVAL + 1, MAX_EVAL - 1);
    match board.side_to_move() {
        Colour::White => white,
        Colour::Black => -white,
    }
}

/// From White's point of view whichever side is to move. The search never calls it.
#[must_use]
pub fn trace(board: &Board) -> Trace {
    let mut t = Trace {
        coefficients: [0; WEIGHT_COUNT],
        phase: 0,
    };
    t.phase = terms(board, &mut t);
    t
}

/// From the owner's point of view. For inspection and tests; the evaluation reads [`WEIGHTS`]
/// directly.
#[must_use]
pub fn piece_square(colour: Colour, pt: PieceType, sq: Square) -> (i32, i32) {
    let s = match colour {
        Colour::White => sq.index(),
        Colour::Black => sq.flip_vertical().index(),
    };
    let w = WEIGHTS[PST + 64 * pt.index() + s];
    (w.mg, w.eg)
}
