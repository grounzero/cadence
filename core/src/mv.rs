// SPDX-License-Identifier: GPL-3.0-or-later

use alloc::string::String;
use core::fmt::{self, Write as _};
use core::mem::{align_of, size_of};

use crate::castling::CastleSide;
use crate::types::{File, PromoPiece, Square};

/// `from` in the low bits makes the history index a mask; flags in the top bits make "noisy" one
/// `AND`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Move(u16);

/// For construction only: nothing decodes a `MoveFlag` from a `Move`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum MoveFlag {
    Quiet = 0b0000,
    DoublePush = 0b0001,
    Castle = 0b0010,
    // 0b0011 reserved
    Capture = 0b0100,
    EnPassant = 0b0101,
    // 0b0110, 0b0111 reserved
    PromoN = 0b1000,
    PromoB = 0b1001,
    PromoR = 0b1010,
    PromoQ = 0b1011,
    PromoCapN = 0b1100,
    PromoCapB = 0b1101,
    PromoCapR = 0b1110,
    PromoCapQ = 0b1111,
}

const FROM_TO: u16 = 0x0FFF;
/// Set for every capturing flag and no other.
const CAPTURE_BIT: u16 = 0x4000;
/// Set for every promoting flag and no other.
const PROMOTION_BIT: u16 = 0x8000;
const NOISY_MASK: u16 = CAPTURE_BIT | PROMOTION_BIT;

impl Move {
    /// A pattern no real move has.
    pub const NULL: Move = Move(0);

    #[inline]
    const fn encode(from: Square, to: Square, flag: MoveFlag) -> Move {
        Move((from.index() as u16) | ((to.index() as u16) << 6) | ((flag as u16) << 12))
    }

    #[inline]
    const fn flag_bits(self) -> u16 {
        self.0 >> 12
    }

    // --- constructors -----------------------------------------------------

    #[must_use]
    pub const fn new_quiet(from: Square, to: Square) -> Move {
        Move::encode(from, to, MoveFlag::Quiet)
    }

    #[must_use]
    pub const fn new_double_push(from: Square, to: Square) -> Move {
        Move::encode(from, to, MoveFlag::DoublePush)
    }

    #[must_use]
    pub const fn new_capture(from: Square, to: Square) -> Move {
        Move::encode(from, to, MoveFlag::Capture)
    }

    /// `to` is the ep square, not the captured pawn's.
    #[must_use]
    pub const fn new_en_passant(from: Square, to: Square) -> Move {
        Move::encode(from, to, MoveFlag::EnPassant)
    }

    /// King takes rook: `to` is our own rook's square.
    #[must_use]
    pub const fn new_castle(king_from: Square, rook_from: Square) -> Move {
        Move::encode(king_from, rook_from, MoveFlag::Castle)
    }

    #[must_use]
    pub const fn new_promotion(from: Square, to: Square, p: PromoPiece) -> Move {
        Move(Move::encode(from, to, MoveFlag::PromoN).0 | ((p as u16) << 12))
    }

    #[must_use]
    pub const fn new_promotion_capture(from: Square, to: Square, p: PromoPiece) -> Move {
        Move(Move::encode(from, to, MoveFlag::PromoCapN).0 | ((p as u16) << 12))
    }

    // --- accessors --------------------------------------------------------

    #[inline]
    #[must_use]
    pub const fn from_sq(self) -> Square {
        Square::new((self.0 & 63) as u8)
    }

    /// For castling, the own rook's square.
    #[inline]
    #[must_use]
    pub const fn to_sq(self) -> Square {
        Square::new(((self.0 >> 6) & 63) as u8)
    }

    #[inline]
    #[must_use]
    pub const fn from_to(self) -> usize {
        (self.0 & FROM_TO) as usize
    }

    #[inline]
    #[must_use]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    /// False for castling, whose destination holds a friendly rook.
    #[inline]
    #[must_use]
    pub const fn is_capture(self) -> bool {
        self.0 & CAPTURE_BIT != 0
    }

    #[inline]
    #[must_use]
    pub const fn is_promotion(self) -> bool {
        self.0 & PROMOTION_BIT != 0
    }

    #[inline]
    #[must_use]
    pub const fn is_noisy(self) -> bool {
        self.0 & NOISY_MASK != 0
    }

    #[inline]
    #[must_use]
    pub const fn is_castle(self) -> bool {
        self.flag_bits() == MoveFlag::Castle as u16
    }

    #[inline]
    #[must_use]
    pub const fn is_en_passant(self) -> bool {
        self.flag_bits() == MoveFlag::EnPassant as u16
    }

    #[inline]
    #[must_use]
    pub const fn is_double_push(self) -> bool {
        self.flag_bits() == MoveFlag::DoublePush as u16
    }

    #[inline]
    #[must_use]
    pub const fn promotion_piece(self) -> Option<PromoPiece> {
        if self.is_promotion() {
            Some(PromoPiece::ALL[(self.flag_bits() & 0b0011) as usize])
        } else {
            None
        }
    }

    /// Kingside if the rook's file is higher: the king is always between its rooks while it can
    /// castle.
    #[inline]
    #[must_use]
    pub const fn castle_side(self) -> CastleSide {
        // Files compared as discriminants: a derived `PartialOrd` is not const-callable.
        if (self.to_sq().file() as u8) > (self.from_sq().file() as u8) {
            CastleSide::King
        } else {
            CastleSide::Queen
        }
    }

    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u16 {
        self.0
    }

    /// Any pattern, reserved flags included.
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u16) -> Move {
        Move(bits)
    }

    #[must_use]
    pub fn to_uci_chess960(self) -> String {
        if self.is_null() {
            return String::from("0000");
        }
        let mut out = String::with_capacity(5);
        let _ = write!(out, "{}{}", self.from_sq(), self.to_sq());
        if let Some(p) = self.promotion_piece() {
            out.push(p.to_char());
        }
        out
    }
}

/// Names the flag, so a wrong flag reads as one rather than as a wrong square.
impl fmt::Debug for Move {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            return f.write_str("0000[Null]");
        }
        let name = match self.flag_bits() {
            0b0000 => "Quiet",
            0b0001 => "DoublePush",
            0b0010 => "Castle",
            0b0100 => "Capture",
            0b0101 => "EnPassant",
            0b1000 => "PromoN",
            0b1001 => "PromoB",
            0b1010 => "PromoR",
            0b1011 => "PromoQ",
            0b1100 => "PromoCapN",
            0b1101 => "PromoCapB",
            0b1110 => "PromoCapR",
            0b1111 => "PromoCapQ",
            reserved => return write!(f, "{}[Reserved(0b{reserved:04b})]", self.to_uci_chess960()),
        };
        write!(f, "{}[{name}]", self.to_uci_chess960())
    }
}

// No `Display`, because spelling a castle needs the board, and no `Ord`, because bit order means
// nothing to a sort.

const _: () = assert!(size_of::<Move>() == 2);
const _: () = assert!(size_of::<Option<Move>>() == 4);
const _: () = assert!(FROM_TO == 0x0FFF && NOISY_MASK == 0xC000);

// ---------------------------------------------------------------------------
// UCI
// ---------------------------------------------------------------------------

/// Castles as king-to-destination unless that spelling also names a legal quiet move, or the king
/// does not move.
#[must_use]
pub fn to_uci(m: Move, legal: &MoveList, chess960: bool) -> String {
    if !m.is_castle() || chess960 {
        return m.to_uci_chess960();
    }
    let kf = m.from_sq();
    let kd = castle_king_destination(m);
    if kd == kf || legal.contains(Move::new_quiet(kf, kd)) {
        // "g1g1" is not a UCI string, and "f1g1" would name the quiet move.
        return m.to_uci_chess960();
    }
    let mut out = String::with_capacity(4);
    let _ = write!(out, "{kf}{kd}");
    out
}

fn castle_king_destination(m: Move) -> Square {
    let file = match m.castle_side() {
        CastleSide::King => File::G,
        CastleSide::Queen => File::C,
    };
    Square::from_file_rank(file, m.from_sq().rank())
}

/// Matched against the legal list, which settles promotion, en passant, castling and illegality at
/// once.
#[must_use]
pub fn parse_uci(legal: &MoveList, s: &str) -> Option<Move> {
    if s.len() != 4 && s.len() != 5 {
        return None;
    }
    // The exact king-takes-rook spelling of any legal move wins.
    if let Some(m) = legal.iter().find(|m| m.to_uci_chess960() == s) {
        return Some(m);
    }
    if s.len() == 4 {
        let from = Square::from_algebraic(&s[..2])?;
        let to = Square::from_algebraic(&s[2..])?;
        return legal
            .iter()
            .find(|m| m.is_castle() && m.from_sq() == from && castle_king_destination(*m) == to);
    }
    None
}

// ---------------------------------------------------------------------------
// MoveList
// ---------------------------------------------------------------------------

/// The known maximum is 218.
pub const MAX_MOVES: usize = 256;

#[derive(Clone)]
pub struct MoveList {
    moves: [Move; MAX_MOVES],
    len: u16,
}

impl MoveList {
    #[must_use]
    pub fn new() -> Self {
        MoveList {
            moves: [Move::NULL; MAX_MOVES],
            len: 0,
        }
    }

    #[inline]
    pub fn push(&mut self, m: Move) {
        self.moves[self.len as usize] = m;
        self.len += 1;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn as_slice(&self) -> &[Move] {
        &self.moves[..self.len as usize]
    }

    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [Move] {
        &mut self.moves[..self.len as usize]
    }

    pub fn iter(&self) -> impl Iterator<Item = Move> + '_ {
        self.as_slice().iter().copied()
    }

    #[must_use]
    pub fn contains(&self, m: Move) -> bool {
        self.as_slice().contains(&m)
    }
}

impl Default for MoveList {
    fn default() -> Self {
        Self::new()
    }
}

// `len` is `u16`: a `u8` would wrap to zero at 256.
const _: () = assert!(size_of::<MoveList>() == 514);
const _: () = assert!(align_of::<MoveList>() == 2);
