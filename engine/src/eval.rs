// SPDX-License-Identifier: GPL-3.0-or-later

//! Static evaluation: material and piece-square tables, tapered. Integer throughout and
//! symmetric by construction: the tables are written from White's side and read through a
//! vertical flip for Black, so the evaluation of a position and of its mirror differ only in
//! sign.

use cadence_core::position::Board;
use cadence_core::{Colour, PieceType, Square};

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

/// Material, in centipawns, by piece type; middlegame and endgame.
const MATERIAL_MG: [i32; 6] = [100, 320, 330, 500, 900, 0];
const MATERIAL_EG: [i32; 6] = [110, 300, 310, 520, 920, 0];

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

/// How many weights the evaluation reads.
pub const WEIGHT_COUNT: usize = PST + 6 * 64;

/// Every number the evaluation reads, in one table a tuner can address by index. Built at
/// compile time from the material values and the named shapes below.
pub static WEIGHTS: [Pair; WEIGHT_COUNT] = build_weights();

// --- the tables ------------------------------------------------------------

// Squares are `i32` here so that the arithmetic is signed throughout; the only casts are back
// to `usize` to index a table, which cannot lose anything on `0..64`.

/// File and rank distance from the centre, each `0..=3`.
const fn centre_distance(sq: i32) -> (i32, i32) {
    let (f, r) = (file_of(sq), rank_of(sq));
    let fd = if f < 4 { 3 - f } else { f - 4 };
    let rd = if r < 4 { 3 - r } else { r - 4 };
    (fd, rd)
}

const fn file_of(sq: i32) -> i32 {
    sq % 8
}

const fn rank_of(sq: i32) -> i32 {
    sq / 8
}

/// On a1-h8 or h1-a8.
const fn on_long_diagonal(sq: i32) -> bool {
    let (f, r) = (file_of(sq), rank_of(sq));
    f == r || f + r == 7
}

const fn pawn(sq: i32, mg: bool) -> i32 {
    // Advance is worth a little in the middlegame and a lot in the ending. Pawns never stand on
    // the first or last rank; those entries are zero.
    const RANK_MG: [i32; 8] = [0, 0, 0, 4, 8, 16, 30, 0];
    const RANK_EG: [i32; 8] = [0, 0, 4, 10, 20, 40, 70, 0];
    let (f, r) = (file_of(sq), rank_of(sq));
    if mg {
        // A pawn on the central files in the middle of the board, where it takes space, is
        // worth a little more in the middlegame.
        let centre = if r >= 2 && r <= 4 {
            match f {
                3 | 4 => 6,
                2 | 5 => 2,
                _ => 0,
            }
        } else {
            0
        };
        RANK_MG[r as usize] + centre
    } else {
        RANK_EG[r as usize]
    }
}

const fn knight(sq: i32, mg: bool) -> i32 {
    // Centralisation: the Manhattan distance from the centre, `0..=6`.
    let (fd, rd) = centre_distance(sq);
    let md = fd + rd;
    if mg { 24 - 8 * md } else { 16 - 6 * md }
}

const fn bishop(sq: i32, mg: bool) -> i32 {
    let (fd, rd) = centre_distance(sq);
    let md = fd + rd;
    let diagonal = if on_long_diagonal(sq) { 5 } else { 0 };
    if mg {
        10 - 3 * md + diagonal
    } else {
        8 - 3 * md
    }
}

const fn rook(sq: i32, mg: bool) -> i32 {
    // The seventh rank, and a little for the central files.
    const FILE_MG: [i32; 8] = [-2, 0, 2, 4, 4, 2, 0, -2];
    let (f, r) = (file_of(sq), rank_of(sq));
    let seventh = if r == 6 { if mg { 20 } else { 10 } } else { 0 };
    if mg {
        seventh + FILE_MG[f as usize]
    } else {
        seventh
    }
}

const fn queen(sq: i32, mg: bool) -> i32 {
    let (fd, rd) = centre_distance(sq);
    let md = fd + rd;
    if mg { 2 - 2 * md } else { 10 - 3 * md }
}

const fn king(sq: i32, mg: bool) -> i32 {
    // Middlegame: the back rank, behind the castling files, and nowhere else. Endgame: the
    // centre.
    const FILE_MG: [i32; 8] = [0, 20, 15, -10, -10, 0, 20, 10];
    let (f, r) = (file_of(sq), rank_of(sq));
    if mg {
        if r == 0 {
            FILE_MG[f as usize]
        } else {
            -10 - 25 * r
        }
    } else {
        let (fd, rd) = centre_distance(sq);
        20 - 6 * (fd + rd)
    }
}

const fn build_weights() -> [Pair; WEIGHT_COUNT] {
    let mut w = [Pair { mg: 0, eg: 0 }; WEIGHT_COUNT];
    let mut pt = 0;
    while pt < 6 {
        w[MATERIAL + pt] = Pair {
            mg: MATERIAL_MG[pt],
            eg: MATERIAL_EG[pt],
        };
        pt += 1;
    }
    let mut sq: i32 = 0;
    while sq < 64 {
        let i = sq as usize;
        w[PST + i] = Pair {
            mg: pawn(sq, true),
            eg: pawn(sq, false),
        };
        w[PST + 64 + i] = Pair {
            mg: knight(sq, true),
            eg: knight(sq, false),
        };
        w[PST + 2 * 64 + i] = Pair {
            mg: bishop(sq, true),
            eg: bishop(sq, false),
        };
        w[PST + 3 * 64 + i] = Pair {
            mg: rook(sq, true),
            eg: rook(sq, false),
        };
        w[PST + 4 * 64 + i] = Pair {
            mg: queen(sq, true),
            eg: queen(sq, false),
        };
        w[PST + 5 * 64 + i] = Pair {
            mg: king(sq, true),
            eg: king(sq, false),
        };
        sq += 1;
    }
    w
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
    phase.min(PHASE_MAX)
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
