// SPDX-License-Identifier: GPL-3.0-or-later

//! Static evaluation: material and piece-square tables, tapered.
//!
//! **Tapered**, with a middlegame and an endgame value for every term,
//! blended by the phase of the game. Not because this evaluation is
//! serious -- it is the seed for the first self-play net and will be
//! rewritten before then -- but because a single table is actively wrong
//! for two of the six pieces: a king wants the corner while queens are on
//! the board and the centre once they are gone, and a pawn's advance is
//! worth little until it is. One table forces a compromise that either
//! walks the king out early or never brings it in, and the blend costs
//! three integer operations per evaluation.
//!
//! Everything is integer so evaluation remains deterministic. The blend is
//! `(mg * phase + eg * (PHASE_MAX - phase)) / PHASE_MAX` with Rust's
//! truncating division, which is symmetric under negation -- so the
//! evaluation of a position and of its colour-mirror are exact negatives,
//! which `tests/eval.rs` asserts. A flooring division would be off by one
//! between them.
//!
//! The tables are built at compile time from a handful of named terms --
//! distance from the centre, rank, file, the long diagonals -- rather than
//! written out as 64 numbers each. The numbers are derived here rather than
//! copied from another engine; they are the shape of the idea, which is
//! all this stage needs. Tables are from White's point of view with A1 = 0,
//! and Black's squares are looked up through `flip_vertical`.

use cadence_core::position::Board;
use cadence_core::{Colour, PieceType, Square};

use crate::score::{MAX_EVAL, Score};

/// The game phase scale. `PHASE_MAX` is the start position's full
/// complement of minor and major pieces; zero is a pawn ending.
pub const PHASE_MAX: i32 = 24;

/// Phase weight per piece type: knights and bishops one, rooks two, queens
/// four. Two of each minor, two rooks and a queen per side is 24.
const PHASE_WEIGHT: [i32; 6] = [0, 1, 1, 2, 4, 0];
const _: () = assert!(
    2 * (2 * PHASE_WEIGHT[1] + 2 * PHASE_WEIGHT[2] + 2 * PHASE_WEIGHT[3] + PHASE_WEIGHT[4])
        == PHASE_MAX
);

/// Material, in centipawns, by piece type; middlegame and endgame.
const MATERIAL_MG: [i32; 6] = [100, 320, 330, 500, 900, 0];
const MATERIAL_EG: [i32; 6] = [110, 300, 310, 520, 920, 0];

/// The piece-square tables, `[piece type][square]`, White's point of view.
static PST_MG: [[i32; 64]; 6] = build_tables(true);
static PST_EG: [[i32; 64]; 6] = build_tables(false);

// --- the tables ------------------------------------------------------------

// Squares are `i32` here so that the arithmetic is signed throughout; the
// only casts are back to `usize` to index a table, which cannot lose anything
// on `0..64`.

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
    // Advance is worth a little in the middlegame and a lot in the ending.
    // Pawns never stand on the first or last rank; those entries are zero.
    const RANK_MG: [i32; 8] = [0, 0, 0, 4, 8, 16, 30, 0];
    const RANK_EG: [i32; 8] = [0, 0, 4, 10, 20, 40, 70, 0];
    let (f, r) = (file_of(sq), rank_of(sq));
    if mg {
        // A pawn on the central files in the middle of the board, where it
        // takes space, is worth a little more in the middlegame.
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
    // Middlegame: the back rank, behind the castling files, and nowhere
    // else. Endgame: the centre.
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

const fn build_tables(mg: bool) -> [[i32; 64]; 6] {
    let mut t = [[0; 64]; 6];
    let mut sq: i32 = 0;
    while sq < 64 {
        let i = sq as usize;
        t[0][i] = pawn(sq, mg);
        t[1][i] = knight(sq, mg);
        t[2][i] = bishop(sq, mg);
        t[3][i] = rook(sq, mg);
        t[4][i] = queen(sq, mg);
        t[5][i] = king(sq, mg);
        sq += 1;
    }
    t
}

// --- the evaluation --------------------------------------------------------

/// The game phase of `board`, `0..=PHASE_MAX`: the phase weights of every
/// piece on the board, both colours, saturating at `PHASE_MAX`. Pawns and
/// kings do not count.
#[must_use]
pub fn phase(board: &Board) -> i32 {
    let mut phase = 0;
    for pt in PieceType::ALL {
        let n = board.by_type(pt).count();
        phase += PHASE_WEIGHT[pt.index()] * i32::try_from(n).unwrap_or(i32::MAX / 8);
    }
    phase.min(PHASE_MAX)
}

/// The static evaluation of `board` from the side to move's point of view,
/// in centipawns, strictly inside `(-MAX_EVAL, MAX_EVAL)`.
#[must_use]
pub fn evaluate(board: &Board) -> Score {
    let mut mg = 0;
    let mut eg = 0;
    let mut phase = 0;
    for pt in PieceType::ALL {
        let i = pt.index();
        for sq in board.pieces(Colour::White, pt) {
            mg += MATERIAL_MG[i] + PST_MG[i][sq.index()];
            eg += MATERIAL_EG[i] + PST_EG[i][sq.index()];
            phase += PHASE_WEIGHT[i];
        }
        for sq in board.pieces(Colour::Black, pt) {
            let s = sq.flip_vertical().index();
            mg -= MATERIAL_MG[i] + PST_MG[i][s];
            eg -= MATERIAL_EG[i] + PST_EG[i][s];
            phase += PHASE_WEIGHT[i];
        }
    }
    let phase = phase.min(PHASE_MAX);
    // Truncating division: symmetric under negation, so the mirror of a
    // position evaluates to the exact negative.
    let white = (mg * phase + eg * (PHASE_MAX - phase)) / PHASE_MAX;
    // A position with absurd material -- `from_fen` accepts sixty queens --
    // must still not reach the mate scale.
    let white = white.clamp(-MAX_EVAL + 1, MAX_EVAL - 1);
    match board.side_to_move() {
        Colour::White => white,
        Colour::Black => -white,
    }
}

/// The middlegame and endgame piece-square values of a piece of `pt` on
/// `sq`, from the point of view of the colour that owns it. For inspection
/// and tests; the evaluation reads the tables directly.
#[must_use]
pub fn piece_square(colour: Colour, pt: PieceType, sq: Square) -> (i32, i32) {
    let s = match colour {
        Colour::White => sq.index(),
        Colour::Black => sq.flip_vertical().index(),
    };
    (PST_MG[pt.index()][s], PST_EG[pt.index()][s])
}
