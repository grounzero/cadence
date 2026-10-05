// SPDX-License-Identifier: GPL-3.0-or-later

//! Integer and symmetric by construction: tables are written from White's side and flipped for
//! Black, so a position and its mirror differ only in sign.

use cadence_core::position::Board;
use cadence_core::types::{File, Rank};
use cadence_core::{Bitboard, Colour, PieceType, Square, attacks};

use crate::kpk;
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

/// Per rook: on a file holding no pawn, then on one holding only the other side's.
pub const ROOK_FILE: usize = SHIELD + SHIELD_LEN;

pub const ROOK_FILE_LEN: usize = 2;

/// A bishop on each colour, so two on one colour after a promotion are not a pair.
pub const BISHOP_PAIR: usize = ROOK_FILE + ROOK_FILE_LEN;

/// The only weight whose coefficient depends on whose move it is: one for the side to move.
pub const TEMPO: usize = BISHOP_PAIR + 1;

pub const WEIGHT_COUNT: usize = TEMPO + 1;

/// Fitted by `cadence texel` to self-play results; a weight carries no reason beyond the data.
#[rustfmt::skip]
pub static WEIGHTS: [Pair; WEIGHT_COUNT] = [
    // material, fitted: pawn, knight, bishop, rook, queen, king
    p(  95,  104), p( 310,  282), p( 328,  295), p( 469,  520), p( 983,  946), p(   0,    0),
    // pawn, fitted, rank 1 to rank 8, a-file first
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p( -54,   -3), p( -40,  -26), p( -41,  -14), p( -41,  -13), p( -41,  -13), p( -38,  -16), p( -42,  -14), p( -54,   -1),
    p( -49,  -13), p( -24,  -23), p( -29,  -33), p( -24,  -29), p( -24,  -38), p( -30,  -36), p( -25,  -19), p( -48,  -15),
    p( -40,  -10), p( -24,  -21), p(  -7,  -36), p(  -4,  -47), p(  -9,  -47), p(  -6,  -35), p( -28,  -18), p( -40,   -9),
    p( -20,    5), p(  -6,  -11), p(  10,  -28), p(  24,  -38), p(  14,  -47), p(  15,  -28), p(  -2,  -11), p( -27,    0),
    p(  -2,   25), p(   9,   19), p(  26,    4), p(  32,   -2), p(  25,    0), p(  37,    4), p(  16,   32), p(  -7,   32),
    p(  18,   79), p(  40,   78), p(  31,   68), p(  40,   62), p(  42,   51), p(  45,   67), p(  42,   85), p(  28,   93),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    // knight, fitted, rank 1 to rank 8, a-file first
    p( -51,  -43), p( -36,  -29), p( -35,  -17), p( -20,  -12), p( -20,   -9), p( -29,  -17), p( -30,  -27), p( -57,  -50),
    p( -34,  -18), p( -20,  -17), p(  -7,  -19), p(   5,  -15), p(   0,   -5), p( -23,  -20), p( -17,  -36), p( -37,  -22),
    p( -26,  -30), p( -12,  -20), p(  -7,    1), p(   1,    5), p(  -2,    9), p(  -8,    3), p( -14,   -7), p( -33,  -22),
    p(   4,  -16), p(  14,   -3), p(  20,   10), p(  21,    4), p(  18,    7), p(  17,   15), p(   2,   -1), p(  12,  -11),
    p(   4,  -17), p(  21,    0), p(  29,   14), p(  41,    6), p(  41,    6), p(  26,   14), p(  14,   -5), p(  12,  -15),
    p(  -4,  -10), p(  15,   -1), p(  52,   19), p(  39,    2), p(  50,   18), p(  31,    9), p(  21,  -12), p(  -3,  -13),
    p( -37,  -27), p(  -8,  -13), p(  15,   -2), p(  36,   -4), p(  16,    1), p(   9,   -7), p(  -6,  -11), p( -18,  -24),
    p( -34,  -31), p( -26,  -19), p( -18,  -10), p( -18,  -14), p(  -8,   -9), p( -19,  -16), p( -29,  -22), p( -51,  -39),
    // bishop, fitted, rank 1 to rank 8, a-file first
    p(   4,  -26), p(   6,  -10), p(  21,   -2), p(  26,   -5), p(  16,    0), p(  20,   -6), p(   6,   -5), p(   8,  -20),
    p(  14,  -20), p(  15,  -17), p(  17,    5), p(  20,   -1), p(  24,   -6), p(  21,   -7), p(  16,  -15), p(  21,  -12),
    p(  11,   -5), p(  11,    0), p(   7,   17), p(  19,    4), p(  23,    3), p(  13,   10), p(  16,   -2), p(  21,  -15),
    p(   8,    5), p(   3,    7), p(  24,    3), p(  26,   13), p(  26,   13), p(  15,   11), p(  11,    5), p(   6,   -5),
    p(  15,   -2), p(  23,   14), p(  24,   12), p(  47,   14), p(  51,    4), p(  44,   -1), p(  12,    9), p(  12,   -5),
    p(   5,   -8), p(  36,    1), p(  47,   11), p(  29,    1), p(  43,    4), p(  41,    6), p(  25,    2), p(  14,   -2),
    p(  -4,   -9), p(  16,    8), p(  -3,   -2), p(   6,    9), p(   0,   -7), p(   1,    5), p(   6,   10), p(  -5,  -10),
    p(  -3,   -5), p(  -3,   -2), p(  -8,   -5), p( -20,  -20), p(  -8,   -1), p( -18,   -7), p(  -9,   -7), p(  -7,   -7),
    // rook, fitted, rank 1 to rank 8, a-file first
    p( -47,   -9), p( -19,  -19), p(   3,  -22), p(   9,  -14), p(   9,  -14), p(   6,  -17), p(  -6,  -11), p( -39,   -7),
    p( -48,  -10), p( -14,  -28), p(  -1,  -15), p(  -7,  -13), p(  -6,  -16), p( -15,  -16), p( -10,  -20), p( -41,  -10),
    p( -34,  -13), p( -11,  -14), p( -19,   -1), p( -26,   -1), p( -14,   -8), p( -20,   -3), p( -25,   -7), p( -39,  -10),
    p( -29,   -3), p( -17,   -1), p(  -9,    7), p( -20,   -5), p( -22,   -2), p( -20,    8), p( -16,   -4), p( -24,   -2),
    p(  -3,   -4), p(   6,    6), p(   9,   13), p(   4,    8), p(  11,    1), p(   9,    5), p( -12,    4), p( -16,    2),
    p(   3,    3), p(  11,    4), p(  24,   14), p(  28,    6), p(  26,    7), p(  29,    5), p(   5,    3), p(  10,    7),
    p(  16,  -11), p(  28,  -11), p(  35,   -8), p(  33,  -10), p(  31,  -11), p(  42,   -8), p(  26,   -4), p(  24,   -1),
    p(   1,   13), p(   5,    9), p(   6,   10), p(  10,   12), p(   6,    3), p(   8,   14), p(  10,   16), p(   7,   25),
    // queen, fitted, rank 1 to rank 8, a-file first
    p( -32,  -41), p( -29,  -25), p(  -3,  -40), p(  -6,  -37), p(  -4,  -28), p(  -8,  -37), p( -18,  -33), p( -31,  -29),
    p( -31,  -17), p( -11,  -14), p( -13,   -4), p(  -2,  -24), p(  -3,   -9), p(  -1,  -24), p( -15,   -6), p( -18,   -8),
    p( -15,    0), p( -19,   -1), p(  -9,   10), p( -19,    3), p( -17,    6), p( -12,    8), p(  -6,    5), p( -15,   -8),
    p(  -8,    2), p( -17,    9), p(  -7,   15), p( -13,   24), p(  -4,   13), p(  -6,   11), p(  -4,   11), p( -11,    8),
    p(  -3,   15), p(  -2,   16), p(  -6,   24), p(  11,   24), p(  13,   26), p(   7,   21), p(  11,   11), p(  -7,   10),
    p(  16,    4), p(  13,   13), p(  31,   21), p(  31,   35), p(  27,   21), p(  20,   17), p(   8,   10), p(  11,   12),
    p(  -3,    4), p(  10,    5), p(   1,   14), p(  20,   21), p(  -4,   17), p(  13,   13), p(  -2,    1), p(  -3,    3),
    p(  -4,   -8), p(   0,    0), p(   9,    4), p(   5,    8), p(   9,   11), p(  -1,    3), p(  -1,   -2), p(  -5,   -7),
    // king, fitted, rank 1 to rank 8, a-file first
    p(  -3,  -33), p(   2,  -24), p( -11,  -16), p( -25,  -18), p( -21,  -16), p( -18,  -26), p(  22,  -34), p(  21,  -49),
    p( -14,  -16), p( -17,   -7), p( -34,   -2), p( -32,   -7), p( -43,   -1), p( -37,    2), p( -29,   -8), p(  -8,  -19),
    p( -68,   -8), p( -54,    8), p( -66,   10), p( -84,    5), p( -66,    3), p( -71,   13), p( -57,    1), p( -66,   -5),
    p( -83,   -5), p( -76,   16), p( -83,   10), p( -78,    6), p( -84,    5), p( -84,   14), p( -87,    5), p( -86,   -1),
    p(-108,    2), p(-106,   18), p(-102,   16), p(-104,   15), p(-107,   10), p(-107,   24), p(-109,   22), p(-111,    6),
    p(-134,   -1), p(-131,   28), p(-132,   36), p(-133,   29), p(-131,   34), p(-132,   34), p(-130,   31), p(-134,    7),
    p(-160,   -5), p(-160,    7), p(-160,   13), p(-160,   21), p(-160,   18), p(-160,   11), p(-160,    9), p(-160,   -6),
    p(-185,  -16), p(-185,  -10), p(-185,    0), p(-185,    3), p(-185,    3), p(-185,   -4), p(-185,  -11), p(-185,  -16),
    // pawn structure, fitted: passed on ranks 2 to 7, isolated, doubled, connected
    p(   7,   -5), p(  -3,   12), p(  -7,   35), p(  15,   47), p(  41,   40), p(  45,   22),
    p(  -6,   -6), p(  -6,  -20), p(   7,    0),
    // mobility by count of usable squares, fitted: knight 0 to 8, bishop 0 to 13, rook 0 to 14,
    // queen 0 to 27
    p( -27,  -10), p( -11,   -5), p(  -7,   -8), p(  -5,    4), p(  -1,    9), p(   1,   17), p(  10,   17), p(  20,   -3),
    p(  20,  -21),
    p( -43,  -74), p( -21,  -35), p(  -9,  -14), p(  -4,    1), p(   1,    4), p(   5,   14), p(  10,   18), p(  21,   15),
    p(  24,   19), p(  27,   17), p(  24,   19), p(   4,   23), p(  -9,    5), p( -30,  -13),
    p( -51,  -61), p( -37,  -38), p( -34,  -28), p( -33,   -5), p( -23,    2), p( -18,   11), p(  -9,   13), p(  -3,   18),
    p(   7,   17), p(  16,   14), p(  27,   12), p(  29,   22), p(  31,   19), p(  31,   16), p(  66,  -12),
    p( -64,  -34), p( -36,  -40), p( -26,  -35), p( -17,  -44), p( -15,  -26), p( -15,  -13), p( -13,  -11), p( -12,    0),
    p(  -9,   11), p(  -5,   17), p(  -2,   16), p(   2,   27), p(  10,   31), p(  10,   32), p(  20,   27), p(  25,   32),
    p(  25,   30), p(  30,   28), p(  27,   23), p(  30,   19), p(  22,    9), p(  13,  -10), p(   0,  -10), p(   0,  -25),
    p(   0,  -25), p(   0,  -28), p(   0,    0), p(   0,    0),
    // king safety, fitted: attackers on the enemy king's zone 0 to 4, then pawns shielding the king
    // 0 to 4, four or more sharing the last of each
    p( -41,    0), p( -40,    6), p( -10,  -23), p(  32,    2), p(  60,   15),
    p( -48,   28), p( -16,    8), p(   8,  -10), p(  33,  -26), p(  22,    0),
    // not yet fitted: a rook on an open file, then a semi-open one; the bishop pair; tempo
    p(   0,    0), p(   0,    0),
    p(   0,    0),
    p(   0,    0),
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
    } else if index >= TEMPO {
        "tempo".to_string()
    } else if index >= BISHOP_PAIR {
        "bishoppair".to_string()
    } else if index >= ROOK_FILE {
        ["rookfile.open", "rookfile.semiopen"][index - ROOK_FILE].to_string()
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
/// `phase`, then scaled by `rule`'s entry in [`RULE_SCALES`].
#[derive(Clone, Debug)]
pub struct Trace {
    pub coefficients: [i32; WEIGHT_COUNT],
    pub phase: i32,
    pub rule: Option<Rule>,
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
    rooks_and_bishops(board, Colour::White, 1, sink);
    rooks_and_bishops(board, Colour::Black, -1, sink);
    // Inside the walk and not added after `evaluate`'s flip, or the trace could not see it.
    let mover = match board.side_to_move() {
        Colour::White => 1,
        Colour::Black => -1,
    };
    sink.add(TEMPO, mover);
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

const DARK: Bitboard = Bitboard(0xAA55_AA55_AA55_AA55);

#[inline(always)]
fn rooks_and_bishops<S: Sink>(board: &Board, colour: Colour, sign: i32, sink: &mut S) {
    let own = board.pieces(colour, PieceType::Pawn);
    let pawns = board.by_type(PieceType::Pawn);
    for sq in board.pieces(colour, PieceType::Rook) {
        let file = sq.file().bb();
        if (own & file).is_empty() {
            sink.add(ROOK_FILE + usize::from((pawns & file).any()), sign);
        }
    }
    let bishops = board.pieces(colour, PieceType::Bishop);
    if (bishops & DARK).any() && (bishops & !DARK).any() {
        sink.add(BISHOP_PAIR, sign);
    }
}

#[inline(always)]
fn shield<S: Sink>(board: &Board, colour: Colour, sign: i32, sink: &mut S) {
    let king = board.king_square(colour);
    let ahead = PASSED_MASKS[colour.index()][king.index()];
    let count = (board.pieces(colour, PieceType::Pawn) & ahead).count() as usize;
    sink.add(SHIELD + count.min(SHIELD_LEN - 1), sign);
}

// --- rules -----------------------------------------------------------------

/// A recognised position's evaluation is its rule's scale over this.
pub const SCALE_FULL: i32 = 64;

/// A draw the table cannot see, recognised from the board and applied as a scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// A king and pawn against a king, held by the defending king.
    DrawnKpk,
    /// Bishops and pawns against a bare king, every pawn on one rook file, no bishop of its
    /// promotion square's colour, and the defending king on that square or beside it.
    WrongBishop,
}

impl Rule {
    pub const ALL: [Rule; 2] = [Rule::DrawnKpk, Rule::WrongBishop];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Rule::DrawnKpk => "kpk",
            Rule::WrongBishop => "wrongbishop",
        }
    }
}

/// Over [`SCALE_FULL`], by [`Rule::index`]; at full a rule changes no evaluation.
pub const RULE_SCALES: [i32; 2] = [SCALE_FULL, SCALE_FULL];

/// Neither rule holds with a knight, rook or queen on the board, the common exit.
#[must_use]
pub fn rule(board: &Board) -> Option<Rule> {
    let others = board.by_type(PieceType::Knight)
        | board.by_type(PieceType::Rook)
        | board.by_type(PieceType::Queen);
    if others.any() {
        return None;
    }
    if board.by_type(PieceType::Bishop).is_empty() {
        let lone = board.occupied().count() == 3 && board.by_type(PieceType::Pawn).count() == 1;
        return (lone && !kpk_wins(board)).then_some(Rule::DrawnKpk);
    }
    Colour::ALL
        .into_iter()
        .any(|strong| wrong_bishop(board, strong))
        .then_some(Rule::WrongBishop)
}

/// The pawn's side made White and its pawn moved to files a to d, as the table is kept.
fn kpk_wins(board: &Board) -> bool {
    let strong = if board.pieces(Colour::White, PieceType::Pawn).any() {
        Colour::White
    } else {
        Colour::Black
    };
    let Some(pawn) = board.by_type(PieceType::Pawn).lsb() else {
        return false;
    };
    let mut squares = [
        board.king_square(strong),
        board.king_square(strong.flip()),
        pawn,
    ];
    if strong == Colour::Black {
        squares = squares.map(Square::flip_vertical);
    }
    if squares[2].file().index() >= 4 {
        squares = squares.map(|sq| Square::new(sq.index() as u8 ^ 7));
    }
    kpk::wins(
        board.side_to_move() == strong,
        squares[0],
        squares[1],
        squares[2],
    )
}

fn wrong_bishop(board: &Board, strong: Colour) -> bool {
    let weak = strong.flip();
    let pawns = board.pieces(strong, PieceType::Pawn);
    if board.by_colour(weak).count() != 1 || pawns.is_empty() {
        return false;
    }
    let file = if (pawns & !Bitboard::FILE_A).is_empty() {
        File::new(0)
    } else if (pawns & !Bitboard::FILE_H).is_empty() {
        File::new(7)
    } else {
        return false;
    };
    let last = match strong {
        Colour::White => Rank::new(7),
        Colour::Black => Rank::new(0),
    };
    let corner = Square::from_file_rank(file, last);
    let right = if DARK.contains(corner) { DARK } else { !DARK };
    (board.pieces(strong, PieceType::Bishop) & right).is_empty()
        && attacks::king_attacks(corner)
            .with(corner)
            .contains(board.king_square(weak))
}

/// From the side to move's point of view, strictly inside `(-MAX_EVAL, MAX_EVAL)`.
#[must_use]
pub fn evaluate(board: &Board) -> Score {
    let mut sum = Sum { mg: 0, eg: 0 };
    let phase = terms(board, &mut sum);
    let scale = rule(board).map_or(SCALE_FULL, |r| RULE_SCALES[r.index()]);
    // One truncating division, so a scaled position stays symmetric under negation and the tuner
    // can reproduce it exactly.
    let white = (sum.mg * phase + sum.eg * (PHASE_MAX - phase)) * scale / (PHASE_MAX * SCALE_FULL);
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
        rule: rule(board),
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
