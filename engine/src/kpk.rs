// SPDX-License-Identifier: GPL-3.0-or-later

//! Every king and pawn against king position, solved once by retrograde analysis, a bit each. The
//! pawn's side is White with its pawn on files a to d; the caller mirrors into that.

use std::sync::LazyLock;

use cadence_core::{Colour, Square, attacks};

const PAWN_SQUARES: usize = 24;
const SIZE: usize = 2 * 64 * 64 * PAWN_SQUARES;

static TABLE: LazyLock<Box<[u64]>> = LazyLock::new(generate);

/// Whether White wins with best play, White having the king and the pawn.
///
/// # Panics
///
/// If `pawn` is not on files a to d and ranks 2 to 7.
#[must_use]
pub fn wins(white_to_move: bool, white_king: Square, black_king: Square, pawn: Square) -> bool {
    let i = index(
        white_to_move,
        white_king.index(),
        black_king.index(),
        pawn_index(pawn),
    );
    (TABLE[i / 64] >> (i % 64)) & 1 == 1
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Value {
    Invalid,
    Unknown,
    Draw,
    Win,
}

fn index(white_to_move: bool, wk: usize, bk: usize, p: usize) -> usize {
    ((usize::from(!white_to_move) * 64 + wk) * 64 + bk) * PAWN_SQUARES + p
}

fn pawn_index(sq: Square) -> usize {
    let (file, rank) = (sq.file().index(), sq.rank().index());
    assert!(file < 4 && (1..7).contains(&rank), "pawn on {sq}");
    (rank - 1) * 4 + file
}

fn pawn_square(p: usize) -> Square {
    Square::new(u8::try_from((p / 4 + 1) * 8 + p % 4).unwrap_or(0))
}

fn distance(a: Square, b: Square) -> usize {
    let (fa, ra) = (a.file().index(), a.rank().index());
    let (fb, rb) = (b.file().index(), b.rank().index());
    fa.abs_diff(fb).max(ra.abs_diff(rb))
}

/// Black's king moves, a pawn capture among them only where the White king does not guard it.
fn black_moves(wk: Square, bk: Square, pawn: Square) -> impl Iterator<Item = Square> {
    let reach = attacks::king_attacks(bk)
        & !attacks::king_attacks(wk)
        & !attacks::pawn_attacks(Colour::White, pawn);
    reach.into_iter()
}

fn decode(i: usize) -> (bool, Square, Square, Square) {
    let p = i % PAWN_SQUARES;
    let bk = i / PAWN_SQUARES % 64;
    let wk = i / (PAWN_SQUARES * 64) % 64;
    let square = |s: usize| Square::new(u8::try_from(s).unwrap_or(0));
    (i < SIZE / 2, square(wk), square(bk), pawn_square(p))
}

/// A promotion counts only where the new queen cannot be taken, which is exact for this ending.
fn initial(i: usize) -> Value {
    let (white_to_move, wk, bk, pawn) = decode(i);
    if wk == bk || wk == pawn || bk == pawn || distance(wk, bk) <= 1 {
        return Value::Invalid;
    }
    let checked = attacks::pawn_attacks(Colour::White, pawn).contains(bk);
    if white_to_move {
        if checked {
            return Value::Invalid;
        }
        if pawn.rank().index() == 6 {
            let queen = Square::new(u8::try_from(pawn.index() + 8).unwrap_or(0));
            if wk != queen && bk != queen && (distance(bk, queen) > 1 || distance(wk, queen) == 1) {
                return Value::Win;
            }
        }
        return Value::Unknown;
    }
    let mut moves = black_moves(wk, bk, pawn).peekable();
    if moves.peek().is_none() {
        return if checked { Value::Win } else { Value::Draw };
    }
    if moves.any(|to| to == pawn) {
        return Value::Draw;
    }
    Value::Unknown
}

fn step(v: &[Value], i: usize) -> Value {
    let (white_to_move, wk, bk, pawn) = decode(i);
    let p = pawn_index(pawn);
    // White wins by any move that wins and draws only if every move draws; Black the reverse.
    let (good, bad) = if white_to_move {
        (Value::Win, Value::Draw)
    } else {
        (Value::Draw, Value::Win)
    };
    let (mut found, mut all) = (false, true);
    let mut see = |n: Value| {
        found |= n == good;
        all &= n == bad;
    };
    if white_to_move {
        let reach = attacks::king_attacks(wk) & !attacks::king_attacks(bk) & !pawn.bb();
        for to in reach {
            see(v[index(false, to.index(), bk.index(), p)]);
        }
        let rank = pawn.rank().index();
        let one = pawn.index() + 8;
        if rank < 6 && one != wk.index() && one != bk.index() {
            see(v[index(false, wk.index(), bk.index(), p + 4)]);
            let two = one + 8;
            if rank == 1 && two != wk.index() && two != bk.index() {
                see(v[index(false, wk.index(), bk.index(), p + 8)]);
            }
        }
    } else {
        for to in black_moves(wk, bk, pawn) {
            see(v[index(true, wk.index(), to.index(), p)]);
        }
    }
    if found {
        good
    } else if all {
        bad
    } else {
        Value::Unknown
    }
}

/// Iterated until nothing changes; what is still unknown then is a draw by repetition.
fn generate() -> Box<[u64]> {
    let mut v: Vec<Value> = (0..SIZE).map(initial).collect();
    loop {
        let mut changed = false;
        for i in 0..SIZE {
            if v[i] == Value::Unknown {
                let r = step(&v, i);
                if r != Value::Unknown {
                    v[i] = r;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut bits = vec![0u64; SIZE / 64].into_boxed_slice();
    for (i, value) in v.iter().enumerate() {
        if *value == Value::Win {
            bits[i / 64] |= 1 << (i % 64);
        }
    }
    bits
}
