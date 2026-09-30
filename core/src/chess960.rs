// SPDX-License-Identifier: GPL-3.0-or-later

//! Chess960 start positions by their standard number, and the double-Fischer start that gives each
//! side its own array. 518 is the orthodox array, so `dfrc_fen(518, 518)` is the standard start
//! with its castling rights spelled by rook file.

use alloc::format;
use alloc::string::String;

/// How many start arrays there are, numbered from zero.
pub const ARRAYS: u32 = 960;

/// The ten ways two knights fill five free squares, in the numbering's order.
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

/// Puts `piece` on the `nth` square of `rank` still empty, counting from the a-file.
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

/// The back rank of array `n`, a-file first, as the lowercase letters a FEN uses.
///
/// # Panics
///
/// If `n` is not below [`ARRAYS`]. Every number the decoding reads is then in range by
/// construction.
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
    // Rook, king, rook into the three squares left, which is what puts the king between them.
    place(&mut rank, 0, b'r');
    place(&mut rank, 0, b'k');
    place(&mut rank, 0, b'r');
    rank
}

/// The start with White on array `white` and Black on array `black`, castling rights in
/// Shredder notation, king side first for each colour.
///
/// # Panics
///
/// If either number is not below [`ARRAYS`]. The panic names the number.
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
