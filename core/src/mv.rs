// SPDX-License-Identifier: GPL-3.0-or-later

//! Move encoding, the move list, and UCI spelling.
//!
//! What is fixed here is the 16-bit layout and the two properties the rest of
//! the engine reads off it: `is_capture` is one `AND` and is **false for
//! castling**, and `is_noisy` is one `AND` against `0xC000`.

use alloc::string::String;
use core::fmt::{self, Write as _};
use core::mem::{align_of, size_of};

use crate::castling::CastleSide;
use crate::types::{File, PromoPiece, Square};

/// | Bits      | Field  | Notes                                          |
/// |-----------|--------|------------------------------------------------|
/// | `0..=5`   | `from` |                                                |
/// | `6..=11`  | `to`   | **for castling: the own rook's square**        |
/// | `12..=15` | flag   | bit 15 = promotion, bit 14 = capture           |
///
/// `from` occupies the low bits so that the 12-bit butterfly index used by
/// the history heuristic is a mask rather than a shift-and-multiply, and so
/// that "is this move noisy" is one `AND` against `0xC000`.
///
/// The all-zero pattern is the null move. That is sound because `from == to`
/// is unreachable for any real move, castling included: king-takes-rook
/// encoding still puts king and rook on distinct squares. A sentinel is used
/// rather than `Option<Move>` because `Option<Move>` has no niche and costs
/// four bytes (asserted below), so the choice is re-examined if that ever
/// changes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Move(u16);

/// The flag nibble. **A construction vocabulary, not a decoding one**: nothing
/// recovers a `MoveFlag` from a `Move`, and nothing should. Every accessor on
/// `Move` tests the nibble directly, three of the sixteen values are reserved,
/// and no caller wants the enum.
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

/// The 12-bit butterfly index: `from | to << 6`.
const FROM_TO: u16 = 0x0FFF;
/// Bit 14 of the encoding: set for every capturing flag and no other.
const CAPTURE_BIT: u16 = 0x4000;
/// Bit 15 of the encoding: set for every promoting flag and no other.
const PROMOTION_BIT: u16 = 0x8000;
/// Capture or promotion: the qsearch and SEE gate, one `AND`.
const NOISY_MASK: u16 = CAPTURE_BIT | PROMOTION_BIT;

impl Move {
    /// `a1a1`, quiet: a pattern no real move can have.
    pub const NULL: Move = Move(0);

    #[inline]
    const fn encode(from: Square, to: Square, flag: MoveFlag) -> Move {
        Move((from.index() as u16) | ((to.index() as u16) << 6) | ((flag as u16) << 12))
    }

    /// The flag nibble as a number. Private: `MoveFlag` is not decoded.
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

    /// `to` is the destination (the ep square), **not** the captured pawn's
    /// square.
    ///
    #[must_use]
    pub const fn new_en_passant(from: Square, to: Square) -> Move {
        Move::encode(from, to, MoveFlag::EnPassant)
    }

    /// King-takes-rook: `to` is **our own rook's** square.
    ///
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
    ///
    #[inline]
    #[must_use]
    pub const fn to_sq(self) -> Square {
        Square::new(((self.0 >> 6) & 63) as u8)
    }

    /// `from | to << 6`, in `0..4096`. The butterfly index.
    ///
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

    /// **False for castling.** The destination holds a friendly rook and the
    /// `Castle` discriminant has the capture bit clear. SEE, MVV-LVA and
    /// qsearch all read this bit.
    ///
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

    /// `is_capture || is_promotion`, as one `AND`.
    ///
    #[inline]
    #[must_use]
    pub const fn is_noisy(self) -> bool {
        self.0 & NOISY_MASK != 0
    }

    /// True for a castling move, which is encoded king-takes-rook.
    ///
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

    /// Kingside iff the rook stands on a higher file than the king. Derived,
    /// never stored: the king is strictly between its rooks in all 960 start
    /// arrays and rights die when it moves, so the files decide.
    ///
    /// Only meaningful when `is_castle()`.
    ///
    #[inline]
    #[must_use]
    pub const fn castle_side(self) -> CastleSide {
        // Files compared as discriminants: a derived `PartialOrd` is not
        // const-callable.
        if (self.to_sq().file() as u8) > (self.from_sq().file() as u8) {
            CastleSide::King
        } else {
            CastleSide::Queen
        }
    }

    /// The raw encoding, for the transposition table and datagen.
    ///
    #[inline]
    #[must_use]
    pub const fn to_bits(self) -> u16 {
        self.0
    }

    /// The inverse of [`Move::to_bits`]. Any 16-bit pattern is accepted,
    /// including the three reserved flag values; the caller owns what it
    /// stored.
    ///
    #[inline]
    #[must_use]
    pub const fn from_bits(bits: u16) -> Move {
        Move(bits)
    }

    /// The king-takes-rook spelling: `from ++ to ++ promo?`.
    ///
    /// This is what `UCI_Chess960 = true` emits, and it is a pure function of
    /// the move. The standard spelling is not: it has to know whether a quiet
    /// king move to the same square is also legal, so it needs the position,
    /// and it therefore lives on [`to_uci`], not here.
    ///
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

/// `e1h1[Castle]`, `e7e8q[PromoQ]`, `0000[Null]`. The flag nibble is named
/// so a wrong flag reads as a wrong flag rather than as a wrong square, and a
/// reserved nibble is named as reserved rather than misread as a real one.
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

// Deliberately absent, and to stay absent:
//
//   * `Display`: formatting a move needs the board. Castling emits
//     king-takes-rook or king-to-destination depending on `UCI_Chess960`, and
//     the move alone cannot know which.
//   * `Ord`/`PartialOrd`: their existence invites `list.sort()` in the move
//     picker, where ordering by raw bits is meaningless.

const _: () = assert!(size_of::<Move>() == 2);
const _: () = assert!(size_of::<Option<Move>>() == 4);
const _: () = assert!(FROM_TO == 0x0FFF && NOISY_MASK == 0xC000);

// ---------------------------------------------------------------------------
// UCI
// ---------------------------------------------------------------------------

/// Format a move for a GUI.
///
/// Takes the legal move list because the non-960 spelling needs it: castling
/// is emitted as king-to-destination (`e1g1`) unless a quiet king move to
/// that same destination is *also* legal (or the king does not move at all,
/// so `g1g1` would not be a UCI string), in which case it falls back to
/// king-takes-rook. Both are properties of the position rather than of the
/// move. The king-takes-rook spelling alone is pure, and is
/// [`Move::to_uci_chess960`].
///
/// Never a function of a cached `is_standard` flag: such a flag flips as soon
/// as the king leaves its start square, and the engine begins emitting 960
/// spellings to a non-960 GUI mid-game.
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

/// The king's destination for a castling move: the g-file or c-file on its
/// own rank, by the derived side.
fn castle_king_destination(m: Move) -> Square {
    let file = match m.castle_side() {
        CastleSide::King => File::G,
        CastleSide::Queen => File::C,
    };
    Square::from_file_rank(file, m.from_sq().rank())
}

/// Parse a UCI move string against a generated legal move list.
///
/// Matching against the list rather than constructing bits from the string is
/// what disposes of promotion-flag inference, en-passant detection, castling
/// disambiguation and illegal-input rejection all at once. Both spellings are
/// accepted in both modes: the `UCI_Chess960` option governs output only, and
/// GUIs get this wrong often enough that liberality is free insurance.
///
/// The exact king-takes-rook spelling of any legal move wins over the
/// king-to-destination spelling of a castle, so `f1g1` with both a quiet
/// king move and a castle available is the quiet move, which is the only
/// reading under which emission and parsing agree.
#[must_use]
pub fn parse_uci(legal: &MoveList, s: &str) -> Option<Move> {
    if s.len() != 4 && s.len() != 5 {
        return None;
    }
    // The exact king-takes-rook spelling of any legal move wins.
    if let Some(m) = legal.iter().find(|m| m.to_uci_chess960() == s) {
        return Some(m);
    }
    // Then the king-to-destination spelling of a castle.
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

/// The known maximum legal move count is 218. The capacity is rounded up.
pub const MAX_MOVES: usize = 256;

/// A generated move list.
///
/// Carries no scores. The move picker lives in the search and owns a
/// `MoveList` plus a parallel score array, which is what makes staged
/// generation cheap: regenerating quiets does not disturb the scores already
/// computed for the noisy moves.
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

    /// The moves, mutably, for a picker that orders them in place.
    #[must_use]
    pub fn as_mut_slice(&mut self) -> &mut [Move] {
        &mut self.moves[..self.len as usize]
    }

    /// The moves, in generation order.
    pub fn iter(&self) -> impl Iterator<Item = Move> + '_ {
        self.as_slice().iter().copied()
    }

    /// Whether the list contains `m`.
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

// `len` is a `u16`, not a `u8`. With MAX_MOVES = 256 a `u8` length wraps to
// zero at capacity. The 218-move bound makes that unreachable today, which is
// precisely why it would survive review and be found the hard way later.
const _: () = assert!(size_of::<MoveList>() == 514);
const _: () = assert!(align_of::<MoveList>() == 2);
