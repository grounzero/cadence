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
    p( -53,  -62), p( -47,  -47), p( -48,  -37), p( -27,  -34), p( -28,  -30), p( -40,  -39), p( -43,  -46), p( -58,  -70),
    p( -50,  -23), p( -37,  -23), p(  -8,  -27), p(   0,  -26), p(  -5,  -12), p( -28,  -27), p( -33,  -39), p( -50,  -28),
    p( -32,  -38), p( -10,  -29), p(  -1,   -8), p(  10,   -3), p(  10,    2), p(  -4,   -3), p( -14,  -13), p( -40,  -26),
    p(  -3,  -21), p(   9,   -5), p(  26,    1), p(  28,   -5), p(  26,   -5), p(  20,    5), p(  -2,   -3), p(   4,  -14),
    p(  -1,  -24), p(  17,   -3), p(  42,    2), p(  51,   -7), p(  52,   -5), p(  39,    4), p(  10,   -6), p(   8,  -20),
    p( -10,  -14), p(  17,   -1), p(  59,    8), p(  50,   -8), p(  62,    9), p(  38,   -1), p(  21,  -12), p(  -7,  -16),
    p( -49,  -32), p( -11,  -16), p(  17,    0), p(  39,   -3), p(  20,    0), p(  11,   -4), p(  -8,  -12), p( -30,  -29),
    p( -40,  -34), p( -29,  -21), p( -19,  -11), p( -20,  -14), p(  -7,   -9), p( -20,  -17), p( -33,  -23), p( -55,  -41),
    // bishop, fitted, rank 1 to rank 8, a-file first
    p(  -3,  -72), p(  -9,  -41), p(   5,  -30), p(  13,  -25), p(   4,  -23), p(   4,  -31), p( -12,  -40), p(  -1,  -62),
    p(  11,  -31), p(  18,  -22), p(  20,    2), p(  20,   -4), p(  24,   -6), p(  20,   -9), p(  20,  -18), p(  19,  -21),
    p(  12,   -6), p(  15,    0), p(  23,   20), p(  34,    8), p(  40,    5), p(  27,   16), p(  19,   -2), p(  21,  -17),
    p(  16,    4), p(  14,   10), p(  42,    4), p(  54,    9), p(  55,    9), p(  29,   14), p(  22,    6), p(  14,   -3),
    p(  23,   -1), p(  31,   19), p(  43,   17), p(  67,   11), p(  71,    1), p(  61,    0), p(  19,   13), p(  19,   -2),
    p(  17,   -6), p(  48,    7), p(  62,   14), p(  40,    1), p(  60,    5), p(  58,    9), p(  35,    6), p(  24,   -1),
    p(  -3,   -9), p(  29,   12), p(   4,    1), p(  13,   13), p(  12,   -2), p(   9,   10), p(  19,   16), p(  -1,  -10),
    p(  -6,   -7), p(  -5,   -1), p(  -4,   -1), p( -19,  -14), p(  -6,    4), p( -14,   -1), p(  -6,   -4), p(  -8,   -8),
    // rook, fitted, rank 1 to rank 8, a-file first
    p( -75,  -25), p( -46,  -23), p( -27,  -17), p( -23,  -12), p( -25,  -11), p( -25,  -12), p( -38,  -11), p( -69,  -20),
    p( -65,  -11), p( -39,  -23), p( -24,  -13), p( -30,  -12), p( -29,  -14), p( -41,  -12), p( -40,  -19), p( -62,  -10),
    p( -48,  -16), p( -29,  -13), p( -33,   -4), p( -41,   -5), p( -27,  -12), p( -34,   -5), p( -43,   -9), p( -54,  -12),
    p( -29,   -4), p( -32,   -2), p( -20,    4), p( -30,  -10), p( -33,   -6), p( -28,    4), p( -30,   -7), p( -29,   -3),
    p(  -8,   -6), p(  -8,    5), p(  -2,   10), p(  -5,    5), p(   0,   -2), p(   0,    3), p( -29,    1), p( -22,    1),
    p(  -1,    8), p(   5,    8), p(  20,   17), p(  25,    9), p(  22,    9), p(  27,    9), p(  -1,    7), p(   4,   11),
    p(  13,   -1), p(  29,   -1), p(  37,    2), p(  38,    0), p(  36,   -1), p(  46,    1), p(  26,    5), p(  22,    8),
    p(  -1,   19), p(   5,   14), p(   7,   16), p(  12,   18), p(   8,    8), p(  11,   20), p(  10,   22), p(   4,   30),
    // queen, fitted, rank 1 to rank 8, a-file first
    p( -52,  -49), p( -48,  -33), p( -19,  -47), p( -22,  -46), p( -21,  -38), p( -25,  -44), p( -40,  -41), p( -52,  -37),
    p( -39,  -20), p( -20,  -14), p( -23,   -2), p( -12,  -23), p( -11,   -8), p( -12,  -22), p( -25,   -6), p( -26,  -11),
    p( -19,   -4), p( -22,   -2), p(  -7,   11), p( -18,    5), p( -15,    8), p( -11,   10), p(  -9,    5), p( -20,  -11),
    p(  -9,    1), p( -14,   10), p(  -1,   17), p(  -4,   26), p(   6,   15), p(  -1,   12), p(  -1,   12), p( -12,    7),
    p(  -1,   14), p(   2,   16), p(   4,   25), p(  24,   24), p(  26,   25), p(  17,   22), p(  15,   12), p(  -5,    9),
    p(  16,    4), p(  20,   14), p(  43,   23), p(  41,   37), p(  38,   23), p(  31,   19), p(  15,   12), p(  12,   11),
    p(   1,    4), p(  17,    7), p(   8,   16), p(  29,   23), p(   6,   19), p(  22,   16), p(   7,    3), p(   3,    3),
    p(  -5,   -7), p(   3,    2), p(  13,    7), p(  10,   11), p(  15,   14), p(   4,    6), p(   3,   -1), p(  -4,   -6),
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
    // mobility by count of usable squares, at zero until fitted: knight 0 to 8, bishop 0 to 13,
    // rook 0 to 14, queen 0 to 27
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p(   0,    0),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
    p(   0,    0), p(   0,    0), p(   0,    0), p(   0,    0),
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
