// SPDX-License-Identifier: GPL-3.0-or-later

//! Every reversible move of a piece other than a pawn, found by the change it makes to the key, so
//! a position one move from an earlier one is found by a lookup and not by generating moves.

use crate::types::{Piece, Square};
use crate::zobrist;

/// A power of two over twice the moves, which two-choice placement needs to settle.
const SLOTS: usize = 8192;

/// Each pair of squares a knight, bishop, rook, queen or king joins on an empty board, per colour.
const MOVES: usize = 3_668;

struct Table {
    /// Zero where empty: no move's change to the key is zero.
    keys: [u64; SLOTS],
    /// The piece's index, then the two squares, the lower first.
    moves: [(u8, u8, u8); SLOTS],
}

const fn first(key: u64) -> usize {
    (key as usize) & (SLOTS - 1)
}

const fn second(key: u64) -> usize {
    ((key >> 16) as usize) & (SLOTS - 1)
}

/// On an empty board, so the squares between are checked against the position when it is asked.
const fn joins(kind: usize, a: usize, b: usize) -> bool {
    let df = (a % 8).abs_diff(b % 8);
    let dr = (a / 8).abs_diff(b / 8);
    match kind {
        1 => (df == 1 && dr == 2) || (df == 2 && dr == 1),
        2 => df == dr,
        3 => df == 0 || dr == 0,
        4 => df == dr || df == 0 || dr == 0,
        5 => df <= 1 && dr <= 1,
        _ => false,
    }
}

#[allow(
    clippy::large_stack_arrays,
    reason = "only ever const-evaluated; the arrays land in .rodata, not on a stack"
)]
const fn build() -> Table {
    let (pieces, side) = zobrist::piece_and_side_keys();
    let mut table = Table {
        keys: [0; SLOTS],
        moves: [(0, 0, 0); SLOTS],
    };
    let mut count = 0;
    let mut piece = 0;
    while piece < 12 {
        let mut a = 0;
        while a < 64 {
            let mut b = a + 1;
            while b < 64 {
                if joins(piece % 6, a, b) {
                    let mut key = pieces[piece][a] ^ pieces[piece][b] ^ side;
                    assert!(key != 0, "a move that leaves the key unchanged");
                    let mut mv = (piece as u8, a as u8, b as u8);
                    let mut slot = first(key);
                    let mut steps = 0;
                    // A displaced entry moves to its other slot, until one lands in an empty one.
                    loop {
                        let (k, m) = (table.keys[slot], table.moves[slot]);
                        table.keys[slot] = key;
                        table.moves[slot] = mv;
                        if k == 0 {
                            break;
                        }
                        (key, mv) = (k, m);
                        slot = if slot == first(key) {
                            second(key)
                        } else {
                            first(key)
                        };
                        steps += 1;
                        assert!(steps < SLOTS, "the moves do not settle in the table");
                    }
                    count += 1;
                }
                b += 1;
            }
            a += 1;
        }
        piece += 1;
    }
    assert!(count == MOVES, "the reversible moves are miscounted");
    table
}

#[allow(long_running_const_eval)]
static TABLE: Table = build();

/// The piece and the two squares of the reversible move that changes a key by `change`, if one
/// does.
#[inline]
#[must_use]
pub fn find(change: u64) -> Option<(Piece, Square, Square)> {
    let mut slot = first(change);
    if TABLE.keys[slot] != change {
        slot = second(change);
        if TABLE.keys[slot] != change {
            return None;
        }
    }
    let (piece, a, b) = TABLE.moves[slot];
    Some((
        Piece::ALL[usize::from(piece)],
        Square::new(a),
        Square::new(b),
    ))
}
