// SPDX-License-Identifier: GPL-3.0-or-later

//! Array 518 is the orthodox start.

use alloc::format;
use alloc::string::String;

pub const ARRAYS: u32 = 960;

/// In the numbering's order.
const KNIGHTS: [(usize, usize); 10] = [
    (0, 1),
    (0, 2),
    (0, 3),
    (0, 4),
    (1, 2),
    (1, 3),
    (1, 4),
    (2, 3),
    (2, 4),
    (3, 4),
];

fn place(rank: &mut [u8; 8], nth: usize, piece: u8) {
    let square = rank
        .iter()
        .enumerate()
        .filter(|(_, p)| **p == 0)
        .nth(nth)
        .map(|(i, _)| i)
        .expect("the numbering never asks for a square that is not free");
    rank[square] = piece;
}

/// # Panics
///
/// If `n` is not below [`ARRAYS`].
#[must_use]
pub fn back_rank(n: u32) -> [u8; 8] {
    assert!(n < ARRAYS, "{n} is not a Chess960 start array");
    let n = n as usize;
    let mut rank = [0u8; 8];
    rank[2 * (n % 4) + 1] = b'b';
    rank[2 * (n / 4 % 4)] = b'b';
    let n = n / 16;
    place(&mut rank, n % 6, b'q');
    // The higher knight first, so that placing it leaves the lower one's count unchanged.
    let (low, high) = KNIGHTS[n / 6];
    place(&mut rank, high, b'n');
    place(&mut rank, low, b'n');
    // R K R into what is left, which puts the king between its rooks.
    place(&mut rank, 0, b'r');
    place(&mut rank, 0, b'k');
    place(&mut rank, 0, b'r');
    rank
}

/// Castling rights in Shredder notation.
///
/// # Panics
///
/// If either number is not below [`ARRAYS`].
#[must_use]
pub fn dfrc_fen(white: u32, black: u32) -> String {
    let w = back_rank(white);
    let b = back_rank(black);
    let rooks = |rank: &[u8; 8]| {
        let mut files = (0u8..8).filter(|&f| rank[usize::from(f)] == b'r');
        let queen_side = files.next().expect("two rooks");
        let king_side = files.next().expect("two rooks");
        (b'a' + king_side, b'a' + queen_side)
    };
    let (wk, wq) = rooks(&w);
    let (bk, bq) = rooks(&b);
    let upper: String = w
        .iter()
        .map(|p| char::from(p.to_ascii_uppercase()))
        .collect();
    let lower: String = b.iter().map(|&p| char::from(p)).collect();
    format!(
        "{lower}/pppppppp/8/8/8/8/PPPPPPPP/{upper} w {}{}{}{} - 0 1",
        char::from(wk.to_ascii_uppercase()),
        char::from(wq.to_ascii_uppercase()),
        char::from(bk),
        char::from(bq),
    )
}
