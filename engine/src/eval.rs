// SPDX-License-Identifier: GPL-3.0-or-later

//! Static evaluation: material and piece-square tables, tapered. Integer throughout and
//! symmetric by construction: the tables are written from White's side and read through a
//! vertical flip for Black, so the evaluation of a position and of its mirror differ only in
//! sign.

use cadence_core::position::Board;
use cadence_core::{Bitboard, Colour, PieceType, Square, attacks};

use crate::score::{MAX_EVAL, Score};

/// The game phase scale. `PHASE_MAX` is the start position's full complement of minor and major
/// pieces; zero is a pawn ending.
pub const PHASE_MAX: i32 = 24;

/// Phase weight per piece type: knights and bishops one, rooks two, queens four. Two of each
/// minor, two rooks and a queen per side is 24.
const PHASE_WEIGHT: [i32; 6] = [0, 1, 1, 2, 4, 0];
const _: () = assert!(
    2 * (2 * PHASE_WEIGHT[1] + 2 * PHASE_WEIGHT[2] + 2 * PHASE_WEIGHT[3] + PHASE_WEIGHT[4])
        == PHASE_MAX
);

/// A middlegame and an endgame value: the unit every weight of the evaluation is stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pair {
    pub mg: i32,
    pub eg: i32,
}

/// Where material starts in [`WEIGHTS`]: one entry per piece type, by `PieceType::index`.
pub const MATERIAL: usize = 0;

/// Where the piece-square tables start in [`WEIGHTS`]: `64 * piece type + square`, White's
/// point of view.
pub const PST: usize = MATERIAL + 6;

/// Where the passed-pawn weights start in [`WEIGHTS`]: one per rank from its own side, second to
/// seventh.
pub const PASSED: usize = PST + 6 * 64;

/// A pawn with no friendly pawn on an adjacent file.
pub const ISOLATED: usize = PASSED + 6;

/// Each pawn beyond the first of its colour on a file.
pub const DOUBLED: usize = ISOLATED + 1;

/// A pawn defended by a friendly pawn, or beside one on its rank.
pub const CONNECTED: usize = DOUBLED + 1;

/// Where the mobility tables start in [`WEIGHTS`]: one entry per count of usable squares, for the
/// knight, bishop, rook and queen in turn.
pub const MOBILITY: usize = CONNECTED + 1;

/// How many counts each piece type's mobility table holds, by `PieceType::index`. The pawn and the
/// king have none.
pub const MOBILITY_LEN: [usize; 6] = [0, 9, 14, 15, 28, 0];

/// Where each piece type's mobility table starts, counted from [`MOBILITY`]; the last entry is
/// their total.
pub const MOBILITY_OFFSET: [usize; 6] = {
    let mut out = [0; 6];
    let mut i = 1;
    while i < 6 {
        out[i] = out[i - 1] + MOBILITY_LEN[i - 1];
        i += 1;
    }
    out
};

/// How many weights the evaluation reads.
pub const WEIGHT_COUNT: usize = MOBILITY + MOBILITY_OFFSET[5] + MOBILITY_LEN[5];

/// Every number the evaluation reads, in one table a tuner can address by index. Every weight is
/// fitted by `cadence texel` to self-play results and carries no reason beyond the data.
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
    p(   3,  -30), p(  27,  -36), p(   7,  -22), p( -13,  -23), p(  -8,  -19), p(  -4,  -32), p(  48,  -43), p(  30,  -44),
    p( -18,  -19), p( -24,   -6), p( -41,    0), p( -46,   -1), p( -56,    4), p( -49,    4), p( -39,   -7), p( -13,  -18),
    p( -72,   -8), p( -57,    5), p( -68,    9), p( -89,    3), p( -70,    2), p( -73,   11), p( -62,   -2), p( -70,   -5),
    p( -85,   -6), p( -79,   13), p( -85,   10), p( -80,    6), p( -85,    4), p( -86,   14), p( -89,    4), p( -87,    0),
    p(-109,    2), p(-107,   19), p(-104,   18), p(-105,   18), p(-108,   13), p(-108,   26), p(-109,   23), p(-111,    7),
    p(-135,    0), p(-131,   29), p(-132,   37), p(-132,   31), p(-131,   35), p(-132,   36), p(-131,   33), p(-133,   10),
    p(-160,   -3), p(-160,    8), p(-160,   13), p(-160,   21), p(-160,   18), p(-160,   11), p(-160,   10), p(-160,   -5),
    p(-185,  -16), p(-185,   -9), p(-185,    1), p(-185,    3), p(-185,    3), p(-185,   -3), p(-185,  -10), p(-185,  -16),
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
];

const fn p(mg: i32, eg: i32) -> Pair {
    Pair { mg, eg }
}

/// The name a weight is reported under: `material.knight` or `pst.knight.d4`. For the tuner
/// and its output; nothing on a search path reads it.
///
/// # Panics
///
/// If `index` is not below [`WEIGHT_COUNT`]. That is a caller naming a weight that does not exist.
#[must_use]
pub fn weight_name(index: usize) -> String {
    const NAMES: [&str; 6] = ["pawn", "knight", "bishop", "rook", "queen", "king"];
    assert!(index < WEIGHT_COUNT, "weight {index} of {WEIGHT_COUNT}");
    if index < PST {
        format!("material.{}", NAMES[index - MATERIAL])
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

/// The game phase of `board`, `0..=PHASE_MAX`: the phase weights of every piece on the board,
/// both colours, saturating at `PHASE_MAX`. Pawns and kings do not count.
#[must_use]
pub fn phase(board: &Board) -> i32 {
    let mut phase = 0;
    for pt in PieceType::ALL {
        let n = board.by_type(pt).count();
        phase += PHASE_WEIGHT[pt.index()] * i32::try_from(n).unwrap_or(i32::MAX / 8);
    }
    phase.min(PHASE_MAX)
}

/// What the evaluation's walk over the board reports to: each weight it reads, and how many
/// times, from White's point of view. The search sums through [`evaluate`] and the tuner records
/// through [`trace`], and both are one walk, so the two cannot disagree about what is evaluated.
pub trait Sink {
    /// Counts weight `index` `count` times, negative for Black.
    fn add(&mut self, index: usize, count: i32);
}

/// The sink the search evaluates with: every reported weight, summed.
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

/// The sink a tuner reads: the net count of each weight over both colours, and the phase. The
/// evaluation before its clamp is these coefficients dotted with [`WEIGHTS`], blended by `phase`.
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

/// Every term of the evaluation, reported to `sink`; returns the phase, `0..=PHASE_MAX`.
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
    mobility(board, Colour::White, 1, sink);
    mobility(board, Colour::Black, -1, sink);
    phase.min(PHASE_MAX)
}

/// The squares a pawn of each colour on each square must find free of enemy pawns to be passed:
/// ahead of it on its own file and both neighbours. Built at compile time.
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

/// The files beside each file.
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

/// `colour`'s pawn-structure terms, reported with `sign`, one for White and minus one for Black.
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

/// `colour`'s mobility, reported with `sign`: for each knight, bishop, rook and queen, how many of
/// the squares it attacks hold none of its own pieces and are not attacked by an enemy pawn.
#[inline(always)]
fn mobility<S: Sink>(board: &Board, colour: Colour, sign: i32, sink: &mut S) {
    let occupied = board.occupied();
    let enemy_pawns = board.pieces(colour.flip(), PieceType::Pawn);
    let area = !board.by_colour(colour) & !attacks::pawn_attacks_bb(colour.flip(), enemy_pawns);
    for pt in [
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
    ] {
        let table = MOBILITY + MOBILITY_OFFSET[pt.index()];
        for sq in board.pieces(colour, pt) {
            let reach = match pt {
                PieceType::Knight => attacks::knight_attacks(sq),
                PieceType::Bishop => attacks::bishop_attacks(sq, occupied),
                PieceType::Rook => attacks::rook_attacks(sq, occupied),
                _ => attacks::queen_attacks(sq, occupied),
            };
            sink.add(table + (reach & area).count() as usize, sign);
        }
    }
}

/// The static evaluation of `board` from the side to move's point of view, in centipawns,
/// strictly inside `(-MAX_EVAL, MAX_EVAL)`.
#[must_use]
pub fn evaluate(board: &Board) -> Score {
    let mut sum = Sum { mg: 0, eg: 0 };
    let phase = terms(board, &mut sum);
    // Truncating division: symmetric under negation, so the mirror of a position evaluates to
    // the exact negative.
    let white = (sum.mg * phase + sum.eg * (PHASE_MAX - phase)) / PHASE_MAX;
    // A position with absurd material -- `from_fen` accepts sixty queens -- must still not
    // reach the mate scale.
    let white = white.clamp(-MAX_EVAL + 1, MAX_EVAL - 1);
    match board.side_to_move() {
        Colour::White => white,
        Colour::Black => -white,
    }
}

/// The coefficient of every weight in the evaluation of `board`, from White's point of view
/// whichever side is to move. For the tuner and its gate; the search never calls it.
#[must_use]
pub fn trace(board: &Board) -> Trace {
    let mut t = Trace {
        coefficients: [0; WEIGHT_COUNT],
        phase: 0,
    };
    t.phase = terms(board, &mut t);
    t
}

/// The middlegame and endgame piece-square values of a piece of `pt` on `sq`, from the point of
/// view of the colour that owns it. For inspection and tests; the evaluation reads [`WEIGHTS`]
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
