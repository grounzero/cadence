// SPDX-License-Identifier: GPL-3.0-or-later

use core::fmt;
use core::mem::{align_of, size_of};

use crate::bitboard::Bitboard;

// ---------------------------------------------------------------------------
// Colour, PieceType, Piece
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Colour {
    White = 0,
    Black = 1,
}

impl Colour {
    pub const ALL: [Colour; 2] = [Colour::White, Colour::Black];

    #[inline]
    #[must_use]
    pub const fn flip(self) -> Colour {
        match self {
            Colour::White => Colour::Black,
            Colour::Black => Colour::White,
        }
    }

    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PieceType {
    Pawn = 0,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

impl PieceType {
    pub const ALL: [PieceType; 6] = [
        PieceType::Pawn,
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
        PieceType::King,
    ];

    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn to_char(self) -> char {
        match self {
            PieceType::Pawn => 'p',
            PieceType::Knight => 'n',
            PieceType::Bishop => 'b',
            PieceType::Rook => 'r',
            PieceType::Queen => 'q',
            PieceType::King => 'k',
        }
    }
}

/// Dense and colour-major: `Option<Piece>` must stay one byte, which keeps `StateInfo` on one cache
/// line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Piece {
    WPawn = 0,
    WKnight,
    WBishop,
    WRook,
    WQueen,
    WKing,
    BPawn = 6,
    BKnight,
    BBishop,
    BRook,
    BQueen,
    BKing,
}

impl Piece {
    pub const ALL: [Piece; 12] = [
        Piece::WPawn,
        Piece::WKnight,
        Piece::WBishop,
        Piece::WRook,
        Piece::WQueen,
        Piece::WKing,
        Piece::BPawn,
        Piece::BKnight,
        Piece::BBishop,
        Piece::BRook,
        Piece::BQueen,
        Piece::BKing,
    ];

    /// From [`Piece::ALL`], because a const enum from its discriminant needs a transmute.
    #[inline]
    #[must_use]
    pub const fn new(c: Colour, pt: PieceType) -> Piece {
        Piece::ALL[c.index() * 6 + pt.index()]
    }

    #[inline]
    #[must_use]
    pub const fn colour(self) -> Colour {
        if (self as u8) < 6 {
            Colour::White
        } else {
            Colour::Black
        }
    }

    #[inline]
    #[must_use]
    pub const fn piece_type(self) -> PieceType {
        PieceType::ALL[(self as usize) % 6]
    }

    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn to_char(self) -> char {
        let lower = self.piece_type().to_char();
        match self.colour() {
            Colour::White => lower.to_ascii_uppercase(),
            Colour::Black => lower,
        }
    }

    #[must_use]
    pub const fn from_char(c: char) -> Option<Piece> {
        Some(match c {
            'P' => Piece::WPawn,
            'N' => Piece::WKnight,
            'B' => Piece::WBishop,
            'R' => Piece::WRook,
            'Q' => Piece::WQueen,
            'K' => Piece::WKing,
            'p' => Piece::BPawn,
            'n' => Piece::BKnight,
            'b' => Piece::BBishop,
            'r' => Piece::BRook,
            'q' => Piece::BQueen,
            'k' => Piece::BKing,
            _ => return None,
        })
    }
}

/// Its own type, so a promotion to a king or pawn cannot be constructed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PromoPiece {
    Knight = 0,
    Bishop,
    Rook,
    Queen,
}

impl PromoPiece {
    pub const ALL: [PromoPiece; 4] = [
        PromoPiece::Knight,
        PromoPiece::Bishop,
        PromoPiece::Rook,
        PromoPiece::Queen,
    ];

    #[inline]
    #[must_use]
    pub const fn piece_type(self) -> PieceType {
        match self {
            PromoPiece::Knight => PieceType::Knight,
            PromoPiece::Bishop => PieceType::Bishop,
            PromoPiece::Rook => PieceType::Rook,
            PromoPiece::Queen => PieceType::Queen,
        }
    }

    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[must_use]
    pub const fn to_char(self) -> char {
        self.piece_type().to_char()
    }

    #[must_use]
    pub const fn from_char(c: char) -> Option<PromoPiece> {
        Some(match c {
            'n' => PromoPiece::Knight,
            'b' => PromoPiece::Bishop,
            'r' => PromoPiece::Rook,
            'q' => PromoPiece::Queen,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// File, Rank, Square, OptSquare
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum File {
    A = 0,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
}

impl File {
    pub const ALL: [File; 8] = [
        File::A,
        File::B,
        File::C,
        File::D,
        File::E,
        File::F,
        File::G,
        File::H,
    ];

    /// # Panics
    ///
    /// In debug builds, if `index > 7`.
    #[inline]
    #[must_use]
    pub const fn new(index: u8) -> File {
        debug_assert!(index < 8);
        File::ALL[index as usize]
    }

    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[inline]
    #[must_use]
    pub const fn bb(self) -> Bitboard {
        Bitboard(Bitboard::FILE_A.0 << (self as u8))
    }

    #[must_use]
    pub const fn to_char(self) -> char {
        (b'a' + self as u8) as char
    }

    /// Lowercase only.
    #[must_use]
    pub const fn from_char(c: char) -> Option<File> {
        if c.is_ascii_lowercase() && (c as u32) < ('a' as u32 + 8) {
            Some(File::ALL[(c as u32 - 'a' as u32) as usize])
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Rank {
    One = 0,
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
}

impl Rank {
    pub const ALL: [Rank; 8] = [
        Rank::One,
        Rank::Two,
        Rank::Three,
        Rank::Four,
        Rank::Five,
        Rank::Six,
        Rank::Seven,
        Rank::Eight,
    ];

    /// # Panics
    ///
    /// In debug builds, if `index > 7`.
    #[inline]
    #[must_use]
    pub const fn new(index: u8) -> Rank {
        debug_assert!(index < 8);
        Rank::ALL[index as usize]
    }

    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    #[inline]
    #[must_use]
    pub const fn bb(self) -> Bitboard {
        Bitboard(Bitboard::RANK_1.0 << (8 * self as u8))
    }

    /// The rank from `c`'s side: `Rank::Eight.relative(c)` is its promotion rank.
    #[inline]
    #[must_use]
    pub const fn relative(self, c: Colour) -> Rank {
        // `^ 7` mirrors; times the colour index, White is unchanged.
        Rank::ALL[self.index() ^ (7 * c.index())]
    }

    #[must_use]
    pub const fn to_char(self) -> char {
        (b'1' + self as u8) as char
    }

    #[must_use]
    pub const fn from_char(c: char) -> Option<Rank> {
        if ('1' as u32) <= (c as u32) && (c as u32) < ('1' as u32 + 8) {
            Some(Rank::ALL[(c as u32 - '1' as u32) as usize])
        } else {
            None
        }
    }
}

/// A1 = 0, H8 = 63, rank-major: magics, pawn shifts, `sq ^ 56` and the feature index depend on it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Square(u8);

macro_rules! squares {
    ($($name:ident = $index:literal),* $(,)?) => {
        impl Square {
            $(pub const $name: Square = Square($index);)*
        }
    };
}

squares! {
    A1 = 0, B1 = 1, C1 = 2, D1 = 3, E1 = 4, F1 = 5, G1 = 6, H1 = 7,
    A2 = 8, B2 = 9, C2 = 10, D2 = 11, E2 = 12, F2 = 13, G2 = 14, H2 = 15,
    A3 = 16, B3 = 17, C3 = 18, D3 = 19, E3 = 20, F3 = 21, G3 = 22, H3 = 23,
    A4 = 24, B4 = 25, C4 = 26, D4 = 27, E4 = 28, F4 = 29, G4 = 30, H4 = 31,
    A5 = 32, B5 = 33, C5 = 34, D5 = 35, E5 = 36, F5 = 37, G5 = 38, H5 = 39,
    A6 = 40, B6 = 41, C6 = 42, D6 = 43, E6 = 44, F6 = 45, G6 = 46, H6 = 47,
    A7 = 48, B7 = 49, C7 = 50, D7 = 51, E7 = 52, F7 = 53, G7 = 54, H7 = 55,
    A8 = 56, B8 = 57, C8 = 58, D8 = 59, E8 = 60, F8 = 61, G8 = 62, H8 = 63,
}

impl Square {
    /// `const` because the feature-index pins call it.
    ///
    /// # Panics
    ///
    /// In debug builds, if `index > 63`; release trusts callers, whose indices are in range by
    /// construction.
    #[inline]
    #[must_use]
    pub const fn new(index: u8) -> Square {
        debug_assert!(index < 64);
        Square(index)
    }

    #[inline]
    #[must_use]
    pub const fn from_file_rank(file: File, rank: Rank) -> Square {
        Square((rank as u8) * 8 + (file as u8))
    }

    #[inline]
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    #[inline]
    #[must_use]
    pub const fn bb(self) -> Bitboard {
        Bitboard(1u64 << self.0)
    }

    #[inline]
    #[must_use]
    pub const fn file(self) -> File {
        File::ALL[(self.0 & 7) as usize]
    }

    #[inline]
    #[must_use]
    pub const fn rank(self) -> Rank {
        Rank::ALL[(self.0 >> 3) as usize]
    }

    /// The feature index's perspective flip for Black.
    #[inline]
    #[must_use]
    pub const fn flip_vertical(self) -> Square {
        const FLIP: u8 = 56;
        Square(self.0 ^ FLIP)
    }

    pub fn all() -> impl Iterator<Item = Square> {
        (0..64u8).map(Square::new)
    }

    #[must_use]
    pub fn from_algebraic(s: &str) -> Option<Square> {
        let mut chars = s.chars();
        let file = File::from_char(chars.next()?)?;
        let rank = Rank::from_char(chars.next()?)?;
        if chars.next().is_some() {
            return None;
        }
        Some(Square::from_file_rank(file, rank))
    }
}

impl fmt::Display for Square {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.file().to_char(), self.rank().to_char())
    }
}

impl fmt::Debug for Square {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Absence as its own one-byte type, with no index or arithmetic, so a missing square cannot reach
/// square arithmetic.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct OptSquare(u8);

impl OptSquare {
    pub const NONE: OptSquare = OptSquare(64);

    #[inline]
    #[must_use]
    pub const fn some(sq: Square) -> OptSquare {
        OptSquare(sq.0)
    }

    #[inline]
    #[must_use]
    pub const fn from_option(sq: Option<Square>) -> OptSquare {
        match sq {
            Some(sq) => OptSquare::some(sq),
            None => OptSquare::NONE,
        }
    }

    #[inline]
    #[must_use]
    pub const fn get(self) -> Option<Square> {
        if self.0 < 64 {
            Some(Square(self.0))
        } else {
            None
        }
    }

    #[inline]
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0 >= 64
    }

    #[inline]
    #[must_use]
    pub const fn is_some(self) -> bool {
        self.0 < 64
    }
}

impl fmt::Debug for OptSquare {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.get() {
            Some(sq) => fmt::Display::fmt(&sq, f),
            None => f.write_str("-"),
        }
    }
}

// --- layout guards --------------------------------------------------------
// A newtype, not a 64-variant enum, so `pop_lsb` builds one from `trailing_zeros` without a
// transmute; hence `Option<Square>` is two bytes and absence has its own type.
const _: () = assert!(size_of::<Square>() == 1);
const _: () = assert!(align_of::<Square>() == 1);
const _: () = assert!(size_of::<Option<Square>>() == 2);
const _: () = assert!(size_of::<OptSquare>() == 1);
const _: () = assert!(align_of::<OptSquare>() == 1);

// The niche that keeps the mailbox at 64 bytes and `StateInfo` at 64.
const _: () = assert!(size_of::<Piece>() == 1);
const _: () = assert!(size_of::<Option<Piece>>() == 1);
const _: () = assert!(size_of::<Colour>() == 1);
const _: () = assert!(size_of::<PieceType>() == 1);
const _: () = assert!(size_of::<File>() == 1);
const _: () = assert!(size_of::<Rank>() == 1);
const _: () = assert!(size_of::<PromoPiece>() == 1);
