// SPDX-License-Identifier: GPL-3.0-or-later

//! A set of squares.
//!
//! No chess knowledge above "bits": the shifts know which file wraps, and
//! nothing else. Anything that knows what a knight is lives in `attacks`.

use core::fmt;
use core::mem::{align_of, size_of};
use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, Not};

use crate::types::{Colour, File, Rank, Square};

/// Bit `n` is the square with index `n`, so bit 0 is A1 and bit 63 is H8.
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
#[repr(transparent)]
pub struct Bitboard(pub u64);

const _: () = assert!(size_of::<Bitboard>() == 8);
const _: () = assert!(align_of::<Bitboard>() == 8);

impl Bitboard {
    pub const EMPTY: Bitboard = Bitboard(0);
    pub const FULL: Bitboard = Bitboard(!0);

    pub const FILE_A: Bitboard = Bitboard(0x0101_0101_0101_0101);
    pub const FILE_B: Bitboard = Bitboard(Self::FILE_A.0 << 1);
    pub const FILE_C: Bitboard = Bitboard(Self::FILE_A.0 << 2);
    pub const FILE_D: Bitboard = Bitboard(Self::FILE_A.0 << 3);
    pub const FILE_E: Bitboard = Bitboard(Self::FILE_A.0 << 4);
    pub const FILE_F: Bitboard = Bitboard(Self::FILE_A.0 << 5);
    pub const FILE_G: Bitboard = Bitboard(Self::FILE_A.0 << 6);
    pub const FILE_H: Bitboard = Bitboard(Self::FILE_A.0 << 7);

    pub const RANK_1: Bitboard = Bitboard(0xFF);
    pub const RANK_2: Bitboard = Bitboard(Self::RANK_1.0 << 8);
    pub const RANK_3: Bitboard = Bitboard(Self::RANK_1.0 << 16);
    pub const RANK_4: Bitboard = Bitboard(Self::RANK_1.0 << 24);
    pub const RANK_5: Bitboard = Bitboard(Self::RANK_1.0 << 32);
    pub const RANK_6: Bitboard = Bitboard(Self::RANK_1.0 << 40);
    pub const RANK_7: Bitboard = Bitboard(Self::RANK_1.0 << 48);
    pub const RANK_8: Bitboard = Bitboard(Self::RANK_1.0 << 56);

    /// Every square on `file`.
    #[inline]
    #[must_use]
    pub const fn file(file: File) -> Bitboard {
        Bitboard(Self::FILE_A.0 << (file as u8))
    }

    /// Every square on `rank`.
    #[inline]
    #[must_use]
    pub const fn rank(rank: Rank) -> Bitboard {
        Bitboard(Self::RANK_1.0 << (8 * rank as u8))
    }

    // --- queries ----------------------------------------------------------

    /// The number of squares in the set.
    #[inline]
    #[must_use]
    pub const fn count(self) -> u32 {
        self.0.count_ones()
    }

    #[inline]
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// At least one square. The negation of [`Bitboard::is_empty`], named so
    /// that `if bb.any()` reads as it means.
    #[inline]
    #[must_use]
    pub const fn any(self) -> bool {
        self.0 != 0
    }

    /// At least two squares.
    #[inline]
    #[must_use]
    pub const fn more_than_one(self) -> bool {
        self.0 & self.0.wrapping_sub(1) != 0
    }

    #[inline]
    #[must_use]
    pub const fn contains(self, sq: Square) -> bool {
        self.0 & sq.bb().0 != 0
    }

    /// The lowest square in the set, without removing it.
    #[inline]
    #[must_use]
    pub const fn lsb(self) -> Option<Square> {
        if self.0 == 0 {
            None
        } else {
            Some(Square::new(self.0.trailing_zeros() as u8))
        }
    }

    // --- construction -----------------------------------------------------

    /// This set with `sq` added.
    #[inline]
    #[must_use]
    pub const fn with(self, sq: Square) -> Bitboard {
        Bitboard(self.0 | sq.bb().0)
    }

    /// This set with `sq` removed.
    #[inline]
    #[must_use]
    pub const fn without(self, sq: Square) -> Bitboard {
        Bitboard(self.0 & !sq.bb().0)
    }

    #[inline]
    pub fn set(&mut self, sq: Square) {
        self.0 |= sq.bb().0;
    }

    #[inline]
    pub fn clear(&mut self, sq: Square) {
        self.0 &= !sq.bb().0;
    }

    #[inline]
    pub fn toggle(&mut self, sq: Square) {
        self.0 ^= sq.bb().0;
    }

    /// Removes the lowest square from the set and returns it.
    ///
    /// Deliberately not `const`: it takes `&mut self`, and it is the one place
    /// a [`Square`] is built from `trailing_zeros()`, which is the whole
    /// reason `Square` is a newtype rather than an enum.
    #[inline]
    pub fn pop_lsb(&mut self) -> Option<Square> {
        if self.0 == 0 {
            return None;
        }
        let sq = Square::new(self.0.trailing_zeros() as u8);
        self.0 &= self.0 - 1;
        Some(sq)
    }

    // --- shifts -----------------------------------------------------------
    //
    // The only place the file wrap is known. East/west shifts mask off the
    // edge file first, so a set never wraps onto the next rank.

    #[inline]
    #[must_use]
    pub const fn north(self) -> Bitboard {
        Bitboard(self.0 << 8)
    }

    #[inline]
    #[must_use]
    pub const fn south(self) -> Bitboard {
        Bitboard(self.0 >> 8)
    }

    #[inline]
    #[must_use]
    pub const fn east(self) -> Bitboard {
        Bitboard((self.0 & !Self::FILE_H.0) << 1)
    }

    #[inline]
    #[must_use]
    pub const fn west(self) -> Bitboard {
        Bitboard((self.0 & !Self::FILE_A.0) >> 1)
    }

    #[inline]
    #[must_use]
    pub const fn north_east(self) -> Bitboard {
        Bitboard((self.0 & !Self::FILE_H.0) << 9)
    }

    #[inline]
    #[must_use]
    pub const fn north_west(self) -> Bitboard {
        Bitboard((self.0 & !Self::FILE_A.0) << 7)
    }

    #[inline]
    #[must_use]
    pub const fn south_east(self) -> Bitboard {
        Bitboard((self.0 & !Self::FILE_H.0) >> 7)
    }

    #[inline]
    #[must_use]
    pub const fn south_west(self) -> Bitboard {
        Bitboard((self.0 & !Self::FILE_A.0) >> 9)
    }

    /// One rank towards the opponent: north for White, south for Black.
    #[inline]
    #[must_use]
    pub const fn forward(self, c: Colour) -> Bitboard {
        match c {
            Colour::White => self.north(),
            Colour::Black => self.south(),
        }
    }
}

// --- operators -------------------------------------------------------------

impl BitAnd for Bitboard {
    type Output = Bitboard;
    #[inline]
    fn bitand(self, rhs: Bitboard) -> Bitboard {
        Bitboard(self.0 & rhs.0)
    }
}

impl BitOr for Bitboard {
    type Output = Bitboard;
    #[inline]
    fn bitor(self, rhs: Bitboard) -> Bitboard {
        Bitboard(self.0 | rhs.0)
    }
}

impl BitXor for Bitboard {
    type Output = Bitboard;
    #[inline]
    fn bitxor(self, rhs: Bitboard) -> Bitboard {
        Bitboard(self.0 ^ rhs.0)
    }
}

impl Not for Bitboard {
    type Output = Bitboard;
    #[inline]
    fn not(self) -> Bitboard {
        Bitboard(!self.0)
    }
}

impl BitAndAssign for Bitboard {
    #[inline]
    fn bitand_assign(&mut self, rhs: Bitboard) {
        self.0 &= rhs.0;
    }
}

impl BitOrAssign for Bitboard {
    #[inline]
    fn bitor_assign(&mut self, rhs: Bitboard) {
        self.0 |= rhs.0;
    }
}

impl BitXorAssign for Bitboard {
    #[inline]
    fn bitxor_assign(&mut self, rhs: Bitboard) {
        self.0 ^= rhs.0;
    }
}

// --- iteration -------------------------------------------------------------

/// Squares of a set, lowest first.
pub struct Squares(Bitboard);

impl Iterator for Squares {
    type Item = Square;

    #[inline]
    fn next(&mut self) -> Option<Square> {
        self.0.pop_lsb()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.0.count() as usize;
        (n, Some(n))
    }
}

impl ExactSizeIterator for Squares {}

impl IntoIterator for Bitboard {
    type Item = Square;
    type IntoIter = Squares;

    #[inline]
    fn into_iter(self) -> Squares {
        Squares(self)
    }
}

// --- display ---------------------------------------------------------------

/// An 8×8 grid, rank 8 at the top, `x` for a set square and `.` for a clear
/// one, followed by the hex value.
impl fmt::Debug for Bitboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for r in (0..8).rev() {
            for fl in 0..8 {
                let sq = Square::new(r * 8 + fl);
                f.write_str(if self.contains(sq) { "x " } else { ". " })?;
            }
            f.write_str("\n")?;
        }
        write!(f, "0x{:016x}", self.0)
    }
}
