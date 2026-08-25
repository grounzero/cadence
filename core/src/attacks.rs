// SPDX-License-Identifier: GPL-3.0-or-later

//! Attack sets: leapers, sliders, and the two 64×64 line tables.
//!
//! Every function here answers "which squares does a piece of this kind on
//! this square attack" with no reference to a position. What is on those
//! squares (friend, foe, nothing) is the caller's business.
//!
//! All tables are const-evaluated. The leapers are built from the shifts in
//! `bitboard`, so a wrap bug there would show up here; the gate checks both
//! against a `(file, rank)` oracle that shares nothing with either.

use crate::bitboard::Bitboard;
use crate::magic;
use crate::types::{Colour, Square};

// ---------------------------------------------------------------------------
// Leapers
// ---------------------------------------------------------------------------

const fn knight_from(bb: Bitboard) -> u64 {
    // Two steps one way, one step the other, in all eight combinations.
    let ns = bb.north().north().0 | bb.south().south().0;
    let ew = bb.east().east().0 | bb.west().west().0;
    Bitboard(ns).east().0 | Bitboard(ns).west().0 | Bitboard(ew).north().0 | Bitboard(ew).south().0
}

const fn king_from(bb: Bitboard) -> u64 {
    let ew = bb.east().0 | bb.west().0;
    let row = Bitboard(ew | bb.0);
    ew | row.north().0 | row.south().0
}

/// Function pointers are not const-callable, hence the flag.
const fn build_leapers(knight: bool) -> [u64; 64] {
    let mut out = [0u64; 64];
    let mut sq = 0;
    while sq < 64 {
        let bb = Bitboard(1u64 << sq);
        out[sq] = if knight {
            knight_from(bb)
        } else {
            king_from(bb)
        };
        sq += 1;
    }
    out
}

static KNIGHT: [u64; 64] = build_leapers(true);
static KING: [u64; 64] = build_leapers(false);

const fn build_pawns() -> [[u64; 64]; 2] {
    let mut out = [[0u64; 64]; 2];
    let mut sq = 0;
    while sq < 64 {
        let bb = Bitboard(1u64 << sq);
        out[0][sq] = bb.north_east().0 | bb.north_west().0;
        out[1][sq] = bb.south_east().0 | bb.south_west().0;
        sq += 1;
    }
    out
}

static PAWN: [[u64; 64]; 2] = build_pawns();

#[inline]
#[must_use]
pub fn knight_attacks(sq: Square) -> Bitboard {
    Bitboard(KNIGHT[sq.index()])
}

#[inline]
#[must_use]
pub fn king_attacks(sq: Square) -> Bitboard {
    Bitboard(KING[sq.index()])
}

/// The two squares a pawn of colour `c` on `sq` attacks (one on an edge
/// file). Attacks, not pushes: a pawn on its promotion rank attacks nothing.
#[inline]
#[must_use]
pub fn pawn_attacks(c: Colour, sq: Square) -> Bitboard {
    Bitboard(PAWN[c.index()][sq.index()])
}

/// Every square attacked by any pawn of colour `c` in `pawns`. The set form
/// of [`pawn_attacks`], two shifts rather than a loop.
#[inline]
#[must_use]
pub fn pawn_attacks_bb(c: Colour, pawns: Bitboard) -> Bitboard {
    match c {
        Colour::White => pawns.north_east() | pawns.north_west(),
        Colour::Black => pawns.south_east() | pawns.south_west(),
    }
}

// ---------------------------------------------------------------------------
// Sliders
// ---------------------------------------------------------------------------

/// Squares a rook on `sq` attacks under `occ`, the first blocker in each
/// direction included.
#[inline]
#[must_use]
pub fn rook_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    magic::rook_attacks(sq, occ)
}

/// Squares a bishop on `sq` attacks under `occ`, the first blocker in each
/// direction included.
#[inline]
#[must_use]
pub fn bishop_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    magic::bishop_attacks(sq, occ)
}

/// `rook_attacks | bishop_attacks`.
#[inline]
#[must_use]
pub fn queen_attacks(sq: Square, occ: Bitboard) -> Bitboard {
    rook_attacks(sq, occ) | bishop_attacks(sq, occ)
}

// ---------------------------------------------------------------------------
// BETWEEN and RAY
// ---------------------------------------------------------------------------
//
// Both are derived from the empty-board slider rays, which is the definition:
// two squares are aligned iff one attacks the other on an empty board, the
// open segment is what each attacks with the other as the only blocker, and
// the line is what both attack on the empty board plus the two of them.

const ROOK_DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const BISHOP_DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];

#[expect(
    clippy::large_stack_arrays,
    reason = "only ever const-evaluated; the arrays land in .rodata, not on a stack"
)]
const fn build_lines() -> ([[u64; 64]; 64], [[u64; 64]; 64]) {
    // Empty-board rays once per square, then the blocked walks only for the
    // pairs that turn out to be aligned.
    let mut rays = [[0u64; 64]; 2];
    let mut sq = 0;
    while sq < 64 {
        rays[0][sq] = magic::slow_attacks(sq as u8, 0, &ROOK_DIRS);
        rays[1][sq] = magic::slow_attacks(sq as u8, 0, &BISHOP_DIRS);
        sq += 1;
    }

    let mut between = [[0u64; 64]; 64];
    let mut ray = [[0u64; 64]; 64];
    let mut a = 0;
    while a < 64 {
        let mut b = 0;
        while b < 64 {
            let bit_a = 1u64 << a;
            let bit_b = 1u64 << b;
            let mut d = 0;
            while d < 2 {
                if a != b && rays[d][a] & bit_b != 0 {
                    let dirs = if d == 0 { &ROOK_DIRS } else { &BISHOP_DIRS };
                    between[a][b] = magic::slow_attacks(a as u8, bit_b, dirs)
                        & magic::slow_attacks(b as u8, bit_a, dirs);
                    ray[a][b] = (rays[d][a] & rays[d][b]) | bit_a | bit_b;
                }
                d += 1;
            }
            b += 1;
        }
        a += 1;
    }
    (between, ray)
}

static LINES: ([[u64; 64]; 64], [[u64; 64]; 64]) = build_lines();

/// The **open** segment strictly between `a` and `b` when they share a rank,
/// file or diagonal; empty otherwise, and empty when `a == b`. Symmetric.
///
/// This is the interposition set: a single slider check on the king at `a`
/// from `b` is blocked by a piece landing on any square of `between(a, b)`.
#[inline]
#[must_use]
pub fn between(a: Square, b: Square) -> Bitboard {
    Bitboard(LINES.0[a.index()][b.index()])
}

/// The **whole line** through `a` and `b`, edge to edge and including both,
/// when they share a rank, file or diagonal; empty otherwise, and empty when
/// `a == b`. Symmetric.
///
/// This is the pin line: a piece pinned on `p` to the king on `k` may move
/// only to squares of `ray(k, p)`.
#[inline]
#[must_use]
pub fn ray(a: Square, b: Square) -> Bitboard {
    Bitboard(LINES.1[a.index()][b.index()])
}

/// Whether `c` lies on the line through `a` and `b`: `ray(a, b)` contains
/// `c`. False when `a` and `b` are not aligned.
#[inline]
#[must_use]
pub fn aligned(a: Square, b: Square, c: Square) -> bool {
    ray(a, b).contains(c)
}
