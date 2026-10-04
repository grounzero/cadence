// SPDX-License-Identifier: GPL-3.0-or-later

//! Positions made deterministically, for properties about every position rather than particular
//! ones. The RNG exists so a failure is reproducible from a seed.

use cadence_core::position::Board;
use cadence_core::types::{Colour, PieceType};
use cadence_core::{FenStyle, Move, generate_legal};

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// splitmix64, not `HashMap`'s `RandomState`: a property test that cannot be re-run on the seed
/// that failed finds a bug once.
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        usize::try_from(self.next_u64() % n as u64).expect("fits")
    }
}

// ---------------------------------------------------------------------------
// The 960 back ranks
// ---------------------------------------------------------------------------

/// In the order Scharnagl's numbering uses.
const KNIGHT_PLACEMENTS: [(usize, usize); 10] = [
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

/// Verified against every index in the corpus's DFRC block by `scharnagl_matches_the_corpus`. 518
/// is the standard array.
#[must_use]
pub fn scharnagl(n: u32) -> String {
    assert!(n < 960, "{n} is not a Chess960 start position");
    let mut rank: [Option<char>; 8] = [None; 8];

    let (q, r) = (n / 4, n % 4);
    rank[2 * r as usize + 1] = Some('b'); // light-squared bishop
    let (q, r) = (q / 4, q % 4);
    rank[2 * r as usize] = Some('b'); // dark-squared bishop
    let (q, r) = (q / 6, q % 6);
    free_squares(&rank)[r as usize].clone_into_rank(&mut rank, 'q');

    let (n1, n2) = KNIGHT_PLACEMENTS[q as usize];
    let free = free_squares(&rank);
    let (k1, k2) = (free[n1], free[n2]);
    k1.clone_into_rank(&mut rank, 'n');
    k2.clone_into_rank(&mut rank, 'n');

    // Rook, king, rook in that order, which puts the king strictly between its rooks in all 960
    // arrays.
    let free = free_squares(&rank);
    free[0].clone_into_rank(&mut rank, 'r');
    free[1].clone_into_rank(&mut rank, 'k');
    free[2].clone_into_rank(&mut rank, 'r');

    rank.iter()
        .map(|c| c.expect("every square filled"))
        .collect()
}

#[derive(Clone, Copy)]
pub struct FreeSquare(usize);

impl FreeSquare {
    fn clone_into_rank(self, rank: &mut [Option<char>; 8], piece: char) {
        assert!(rank[self.0].is_none(), "square {} already taken", self.0);
        rank[self.0] = Some(piece);
    }
}

fn free_squares(rank: &[Option<char>; 8]) -> Vec<FreeSquare> {
    rank.iter()
        .enumerate()
        .filter(|(_, c)| c.is_none())
        .map(|(i, _)| FreeSquare(i))
        .collect()
}

/// Castling rights in Shredder notation.
#[must_use]
pub fn dfrc_start_fen(wid: u32, bid: u32) -> String {
    let white = scharnagl(wid);
    let black = scharnagl(bid);
    let rook_files = |rank: &str| -> Vec<usize> {
        rank.char_indices()
            .filter(|(_, c)| *c == 'r')
            .map(|(i, _)| i)
            .collect()
    };
    let w = rook_files(&white);
    let b = rook_files(&black);
    assert_eq!(w.len(), 2, "array {wid} does not have two rooks");
    assert_eq!(b.len(), 2, "array {bid} does not have two rooks");

    let file_char = |i: usize| (b'a' + u8::try_from(i).expect("file fits")) as char;
    // King side, the higher file, first for each colour.
    let rights = format!(
        "{}{}{}{}",
        file_char(w[1]).to_ascii_uppercase(),
        file_char(w[0]).to_ascii_uppercase(),
        file_char(b[1]),
        file_char(b[0]),
    );
    format!(
        "{black}/pppppppp/8/8/8/8/PPPPPPPP/{} w {rights} - 0 1",
        white.to_ascii_uppercase()
    )
}

/// White and Black arrays equal.
#[must_use]
pub fn all_960_start_fens() -> Vec<String> {
    (0..960).map(|n| dfrc_start_fen(n, n)).collect()
}

// ---------------------------------------------------------------------------
// Board fingerprint
// ---------------------------------------------------------------------------

/// The mailbox and the piece bitboards are captured separately, because the failure hunted is the
/// two disagreeing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Fingerprint {
    pub mailbox: Vec<Option<String>>,
    pub bitboards: Vec<u64>,
    pub occupied: u64,
    pub key: u64,
    pub shredder_fen: String,
    pub xfen: String,
}

#[must_use]
pub fn fingerprint(board: &Board) -> Fingerprint {
    let mut mailbox = Vec::with_capacity(64);
    for i in 0..64u8 {
        let sq = cadence_core::types::Square::new(i);
        mailbox.push(board.piece_at(sq).map(|p| format!("{p:?}")));
    }
    let mut bitboards = Vec::with_capacity(12);
    for c in [Colour::White, Colour::Black] {
        for pt in [
            PieceType::Pawn,
            PieceType::Knight,
            PieceType::Bishop,
            PieceType::Rook,
            PieceType::Queen,
            PieceType::King,
        ] {
            bitboards.push(board.pieces(c, pt).0);
        }
    }
    Fingerprint {
        mailbox,
        bitboards,
        occupied: board.occupied().0,
        key: board.key(),
        shredder_fen: board.to_fen(FenStyle::Shredder),
        xfen: board.to_fen(FenStyle::XFen),
    }
}

/// A `Vec`, so the list can outlive the borrow.
#[must_use]
pub fn legal(board: &Board) -> Vec<Move> {
    generate_legal(board).as_slice().to_vec()
}

/// Every full, ordinary corpus board, plus a spread of start arrays.
#[must_use]
pub fn walk_seeds() -> Vec<String> {
    let mut out: Vec<String> = super::standard_positions()
        .into_iter()
        .map(|p| p.fen)
        .collect();
    out.extend(super::dfrc_arrays().into_iter().map(|a| a.fen));
    out
}
