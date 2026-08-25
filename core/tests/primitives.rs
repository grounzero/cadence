// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `types` and `bitboard`.
//!
//! Property tests over the whole domain rather than examples: there are only
//! 64 squares, 12 pieces and 8 directions, so every claim here is checked
//! for every value. The oracle is `(file, rank)` arithmetic written out in
//! plain integers, which is the definition the LERF layout is supposed to
//! satisfy (`index == rank * 8 + file`, `flip == sq ^ 56`), and nothing from
//! the crate is trusted to test itself.

use cadence_core::bitboard::Bitboard;
use cadence_core::types::{Colour, File, OptSquare, Piece, PieceType, PromoPiece, Rank, Square};

// ---------------------------------------------------------------------------
// Square <-> (File, Rank), the LERF invariant
// ---------------------------------------------------------------------------

/// A1 = 0, H8 = 63, `index == rank * 8 + file`, and the round trip through
/// `(File, Rank)` is the identity in both directions.
#[test]
fn square_index_is_rank_major_with_a1_at_zero() {
    for i in 0..64u8 {
        let sq = Square::new(i);
        assert_eq!(sq.index(), usize::from(i));
        assert_eq!(sq.file().index(), usize::from(i % 8), "{sq}");
        assert_eq!(sq.rank().index(), usize::from(i / 8), "{sq}");
        assert_eq!(Square::from_file_rank(sq.file(), sq.rank()), sq);
    }
    for f in File::ALL {
        for r in Rank::ALL {
            let sq = Square::from_file_rank(f, r);
            assert_eq!(sq.file(), f);
            assert_eq!(sq.rank(), r);
            assert_eq!(sq.index(), r.index() * 8 + f.index());
        }
    }
    assert_eq!(Square::A1.index(), 0);
    assert_eq!(Square::H1.index(), 7);
    assert_eq!(Square::A8.index(), 56);
    assert_eq!(Square::H8.index(), 63);
    assert_eq!(Square::E4.index(), 28);
}

/// The named constants agree with the constructor and with their own names.
#[test]
fn square_constants_match_their_names() {
    let named = [
        (Square::A1, "a1"),
        (Square::B1, "b1"),
        (Square::H1, "h1"),
        (Square::A2, "a2"),
        (Square::D4, "d4"),
        (Square::E4, "e4"),
        (Square::E5, "e5"),
        (Square::G6, "g6"),
        (Square::A8, "a8"),
        (Square::H8, "h8"),
    ];
    for (sq, name) in named {
        assert_eq!(sq.to_string(), name);
        assert_eq!(format!("{sq:?}"), name, "Debug prints the algebraic name");
        assert_eq!(Square::from_algebraic(name), Some(sq));
    }
    for i in 0..64u8 {
        let sq = Square::new(i);
        assert_eq!(Square::from_algebraic(&sq.to_string()), Some(sq));
    }
    for bad in ["", "e", "e44", "i1", "a9", "E4", "44", "ee"] {
        assert_eq!(Square::from_algebraic(bad), None, "`{bad}` is not a square");
    }
    let all: Vec<Square> = Square::all().collect();
    assert_eq!(all.len(), 64);
    for (i, sq) in all.iter().enumerate() {
        assert_eq!(sq.index(), i);
    }
}

/// `flip_vertical == sq ^ 56`: same file, mirrored rank, an involution.
#[test]
fn flip_vertical_is_xor_56() {
    const FLIP: u8 = 56;
    for i in 0..64u8 {
        let sq = Square::new(i);
        let flipped = sq.flip_vertical();
        assert_eq!(flipped.index(), usize::from(i ^ FLIP), "{sq}");
        assert_eq!(flipped.file(), sq.file(), "{sq}: the file must not change");
        assert_eq!(
            flipped.rank().index(),
            7 - sq.rank().index(),
            "{sq}: the rank must mirror"
        );
        assert_eq!(
            flipped.flip_vertical(),
            sq,
            "{sq}: flipping twice is the identity"
        );
    }
    assert_eq!(Square::A1.flip_vertical(), Square::A8);
    assert_eq!(Square::E1.flip_vertical(), Square::E8);
    assert_eq!(Square::H4.flip_vertical(), Square::H5);
}

/// `sq.bb()` is exactly bit `index`, so LSB = A1 and MSB = H8.
#[test]
fn square_bitboard_is_its_own_bit() {
    for i in 0..64u8 {
        let sq = Square::new(i);
        assert_eq!(sq.bb().0, 1u64 << i, "{sq}");
        assert_eq!(sq.bb().count(), 1);
        assert!(sq.bb().contains(sq));
        assert_eq!(sq.bb().lsb(), Some(sq));
    }
    assert_eq!(Square::A1.bb().0, 1);
    assert_eq!(Square::H8.bb().0, 1 << 63);
}

// ---------------------------------------------------------------------------
// File and Rank
// ---------------------------------------------------------------------------

#[test]
fn file_and_rank_round_trip_through_index_and_char() {
    for (i, f) in File::ALL.into_iter().enumerate() {
        assert_eq!(f.index(), i);
        assert_eq!(File::new(u8::try_from(i).expect("fits")), f);
        let c = (b'a' + u8::try_from(i).expect("fits")) as char;
        assert_eq!(f.to_char(), c);
        assert_eq!(File::from_char(c), Some(f));
        assert_eq!(f.bb().count(), 8);
        for sq in f.bb() {
            assert_eq!(sq.file(), f);
        }
    }
    for (i, r) in Rank::ALL.into_iter().enumerate() {
        assert_eq!(r.index(), i);
        assert_eq!(Rank::new(u8::try_from(i).expect("fits")), r);
        let c = (b'1' + u8::try_from(i).expect("fits")) as char;
        assert_eq!(r.to_char(), c);
        assert_eq!(Rank::from_char(c), Some(r));
        assert_eq!(r.bb().count(), 8);
        for sq in r.bb() {
            assert_eq!(sq.rank(), r);
        }
    }
    assert_eq!(File::from_char('i'), None);
    assert_eq!(File::from_char('A'), None);
    assert_eq!(Rank::from_char('0'), None);
    assert_eq!(Rank::from_char('9'), None);

    assert_eq!(File::A.bb(), Bitboard::FILE_A);
    assert_eq!(File::H.bb(), Bitboard::FILE_H);
    assert_eq!(Rank::One.bb(), Bitboard::RANK_1);
    assert_eq!(Rank::Eight.bb(), Bitboard::RANK_8);
    assert_eq!(Bitboard::file(File::D), Bitboard::FILE_D);
    assert_eq!(Bitboard::rank(Rank::Four), Bitboard::RANK_4);
}

#[test]
fn rank_relative_is_the_identity_for_white_and_the_mirror_for_black() {
    for r in Rank::ALL {
        assert_eq!(r.relative(Colour::White), r);
        assert_eq!(r.relative(Colour::Black).index(), 7 - r.index());
        assert_eq!(r.relative(Colour::Black).relative(Colour::Black), r);
    }
    assert_eq!(Rank::Eight.relative(Colour::Black), Rank::One);
    assert_eq!(Rank::Two.relative(Colour::Black), Rank::Seven);
}

// ---------------------------------------------------------------------------
// Colour, PieceType, Piece, PromoPiece
// ---------------------------------------------------------------------------

#[test]
fn colour_flip_is_an_involution_and_index_is_the_discriminant() {
    assert_eq!(Colour::White.flip(), Colour::Black);
    assert_eq!(Colour::Black.flip(), Colour::White);
    assert_eq!(Colour::White.index(), 0);
    assert_eq!(Colour::Black.index(), 1);
    assert_eq!(Colour::ALL, [Colour::White, Colour::Black]);
}

/// Colour-major and dense: `Piece::new(c, pt)` is `c * 6 + pt`, and the two
/// accessors invert it for all twelve.
#[test]
fn piece_is_colour_major_and_dense() {
    let mut seen = Vec::new();
    for c in Colour::ALL {
        for pt in PieceType::ALL {
            let p = Piece::new(c, pt);
            assert_eq!(p.colour(), c, "{p:?}");
            assert_eq!(p.piece_type(), pt, "{p:?}");
            assert_eq!(p.index(), c.index() * 6 + pt.index(), "{p:?}");
            assert_eq!(p as u8, u8::try_from(p.index()).expect("fits"));
            seen.push(p);
        }
    }
    assert_eq!(seen, Piece::ALL.to_vec());
    assert_eq!(Piece::WPawn as u8, 0);
    assert_eq!(Piece::WKing as u8, 5);
    assert_eq!(Piece::BPawn as u8, 6);
    assert_eq!(Piece::BKing as u8, 11);
    for (i, pt) in PieceType::ALL.into_iter().enumerate() {
        assert_eq!(pt.index(), i);
    }
}

#[test]
fn piece_chars_are_fen_letters_and_round_trip() {
    let expected = [
        (Piece::WPawn, 'P'),
        (Piece::WKnight, 'N'),
        (Piece::WBishop, 'B'),
        (Piece::WRook, 'R'),
        (Piece::WQueen, 'Q'),
        (Piece::WKing, 'K'),
        (Piece::BPawn, 'p'),
        (Piece::BKnight, 'n'),
        (Piece::BBishop, 'b'),
        (Piece::BRook, 'r'),
        (Piece::BQueen, 'q'),
        (Piece::BKing, 'k'),
    ];
    for (p, c) in expected {
        assert_eq!(p.to_char(), c);
        assert_eq!(Piece::from_char(c), Some(p));
        assert_eq!(p.piece_type().to_char(), c.to_ascii_lowercase());
    }
    for bad in ['x', '1', ' ', '/', 'w'] {
        assert_eq!(Piece::from_char(bad), None);
    }
}

#[test]
fn promo_piece_maps_onto_the_four_promotable_types() {
    let expected = [
        (PromoPiece::Knight, PieceType::Knight, 'n'),
        (PromoPiece::Bishop, PieceType::Bishop, 'b'),
        (PromoPiece::Rook, PieceType::Rook, 'r'),
        (PromoPiece::Queen, PieceType::Queen, 'q'),
    ];
    for (i, (pp, pt, c)) in expected.into_iter().enumerate() {
        assert_eq!(pp.index(), i);
        assert_eq!(pp.piece_type(), pt);
        assert_eq!(pp.to_char(), c);
        assert_eq!(PromoPiece::from_char(c), Some(pp));
        assert_eq!(PromoPiece::ALL[i], pp);
    }
    for bad in ['k', 'p', 'Q', 'x'] {
        assert_eq!(PromoPiece::from_char(bad), None);
    }
}

// ---------------------------------------------------------------------------
// OptSquare
// ---------------------------------------------------------------------------

#[test]
fn opt_square_round_trips_and_none_is_distinct_from_every_square() {
    assert_eq!(OptSquare::NONE.get(), None);
    assert!(OptSquare::NONE.is_none());
    assert!(!OptSquare::NONE.is_some());
    assert_eq!(OptSquare::from_option(None), OptSquare::NONE);
    for sq in Square::all() {
        let some = OptSquare::some(sq);
        assert_eq!(some.get(), Some(sq));
        assert!(some.is_some());
        assert!(!some.is_none());
        assert_ne!(some, OptSquare::NONE);
        assert_eq!(OptSquare::from_option(Some(sq)), some);
        assert_eq!(format!("{some:?}"), sq.to_string());
    }
    assert_eq!(format!("{:?}", OptSquare::NONE), "-");
}

// ---------------------------------------------------------------------------
// Bitboard: set / clear / pop round-trips
// ---------------------------------------------------------------------------

#[test]
fn set_then_clear_is_the_identity_for_every_square() {
    for sq in Square::all() {
        let mut bb = Bitboard::EMPTY;
        assert!(!bb.contains(sq));
        bb.set(sq);
        assert!(bb.contains(sq), "{sq}");
        assert_eq!(bb, sq.bb());
        assert_eq!(bb.count(), 1);
        bb.set(sq);
        assert_eq!(bb.count(), 1, "{sq}: setting twice is idempotent");
        bb.clear(sq);
        assert_eq!(bb, Bitboard::EMPTY, "{sq}");
        bb.clear(sq);
        assert_eq!(bb, Bitboard::EMPTY, "{sq}: clearing twice is idempotent");

        assert_eq!(Bitboard::EMPTY.with(sq), sq.bb());
        assert_eq!(sq.bb().without(sq), Bitboard::EMPTY);
        assert_eq!(Bitboard::FULL.without(sq).with(sq), Bitboard::FULL);
        assert_eq!(Bitboard::FULL.without(sq).count(), 63);

        let mut t = Bitboard::EMPTY;
        t.toggle(sq);
        assert_eq!(t, sq.bb());
        t.toggle(sq);
        assert_eq!(t, Bitboard::EMPTY);
    }
}

/// `pop_lsb` yields the squares in ascending order, exactly once each, and
/// leaves the set empty. Checked over a spread of sets including the extremes.
#[test]
fn pop_lsb_drains_in_ascending_order() {
    let sets: Vec<Bitboard> = [
        Bitboard::EMPTY,
        Bitboard::FULL,
        Bitboard::FILE_A,
        Bitboard::RANK_8,
        Square::A1.bb() | Square::H8.bb(),
        Bitboard(0x8000_0000_0000_0001),
        Bitboard(0x5555_5555_5555_5555),
        Bitboard(0xAAAA_AAAA_AAAA_AAAA),
        Bitboard(0x0F0F_F0F0_1234_5678),
    ]
    .into_iter()
    .chain((0..64u8).map(|i| Bitboard(!0u64 << i)))
    .chain((0..64u8).map(|i| Bitboard(!0u64 >> i)))
    .collect();

    for set in sets {
        let mut bb = set;
        let mut popped = Vec::new();
        while let Some(sq) = bb.pop_lsb() {
            popped.push(sq);
            assert!(!bb.contains(sq), "{sq} was popped but is still present");
        }
        assert_eq!(bb, Bitboard::EMPTY);
        assert_eq!(popped.len(), set.count() as usize, "{set:?}");
        let expected: Vec<Square> = Square::all().filter(|sq| set.contains(*sq)).collect();
        assert_eq!(popped, expected, "{set:?}");
        // Iteration is the same sequence, and the sizes agree.
        assert_eq!(set.into_iter().collect::<Vec<_>>(), expected);
        assert_eq!(set.into_iter().len(), expected.len());
        // pop_lsb on the empty set is None, forever.
        assert_eq!(bb.pop_lsb(), None);
        assert_eq!(bb.pop_lsb(), None);
    }
}

#[test]
fn count_and_emptiness_agree_with_the_bit_count() {
    let samples = [
        (Bitboard::EMPTY, 0),
        (Bitboard::FULL, 64),
        (Bitboard::FILE_A, 8),
        (Bitboard::RANK_1, 8),
        (Bitboard(1), 1),
        (Bitboard(3), 2),
        (Bitboard(0x8000_0000_0000_0000), 1),
        (Bitboard(0xFF00_FF00_FF00_FF00), 32),
    ];
    for (bb, n) in samples {
        assert_eq!(bb.count(), n, "{bb:?}");
        assert_eq!(bb.is_empty(), n == 0);
        assert_eq!(bb.any(), n != 0);
        assert_eq!(bb.more_than_one(), n > 1);
        assert_eq!(bb.lsb().is_some(), n != 0);
    }
    assert_eq!(Bitboard::EMPTY.lsb(), None);
    assert_eq!(Bitboard::FULL.lsb(), Some(Square::A1));
    assert_eq!(Bitboard(0x8000_0000_0000_0000).lsb(), Some(Square::H8));
}

#[test]
fn operators_are_the_underlying_bit_operations() {
    let a = Bitboard(0x0F0F_0F0F_F0F0_F0F0);
    let b = Bitboard(0x00FF_00FF_00FF_00FF);
    assert_eq!((a & b).0, a.0 & b.0);
    assert_eq!((a | b).0, a.0 | b.0);
    assert_eq!((a ^ b).0, a.0 ^ b.0);
    assert_eq!((!a).0, !a.0);
    let mut c = a;
    c &= b;
    assert_eq!(c, a & b);
    c = a;
    c |= b;
    assert_eq!(c, a | b);
    c = a;
    c ^= b;
    assert_eq!(c, a ^ b);
    assert_eq!(a & Bitboard::FULL, a);
    assert_eq!(a | Bitboard::EMPTY, a);
    assert_eq!(a ^ a, Bitboard::EMPTY);
    assert_eq!(!Bitboard::EMPTY, Bitboard::FULL);
}

// ---------------------------------------------------------------------------
// Bitboard: shifts, against (file, rank) arithmetic
// ---------------------------------------------------------------------------

/// The oracle: move `(df, dr)` from every square of the set, dropping squares
/// that leave the board. Written in file/rank integers so it cannot share a
/// wrap bug with the shift being tested.
fn naive_shift(bb: Bitboard, df: i8, dr: i8) -> Bitboard {
    let mut out = 0u64;
    for i in 0..64u8 {
        if bb.0 & (1u64 << i) == 0 {
            continue;
        }
        let f = i8::try_from(i % 8).expect("fits") + df;
        let r = i8::try_from(i / 8).expect("fits") + dr;
        if (0..8).contains(&f) && (0..8).contains(&r) {
            out |= 1u64 << (r * 8 + f);
        }
    }
    Bitboard(out)
}

/// A named shift and the `(df, dr)` it claims to be.
type Shift = (&'static str, fn(Bitboard) -> Bitboard, i8, i8);

#[test]
fn shifts_never_wrap_across_the_board_edge() {
    let dirs: [Shift; 8] = [
        ("north", Bitboard::north, 0, 1),
        ("south", Bitboard::south, 0, -1),
        ("east", Bitboard::east, 1, 0),
        ("west", Bitboard::west, -1, 0),
        ("north_east", Bitboard::north_east, 1, 1),
        ("north_west", Bitboard::north_west, -1, 1),
        ("south_east", Bitboard::south_east, 1, -1),
        ("south_west", Bitboard::south_west, -1, -1),
    ];
    let mut sets: Vec<Bitboard> = Square::all().map(Square::bb).collect();
    sets.extend([
        Bitboard::EMPTY,
        Bitboard::FULL,
        Bitboard::FILE_A,
        Bitboard::FILE_H,
        Bitboard::RANK_1,
        Bitboard::RANK_8,
        Bitboard(0x8142_2418_1824_4281),
        Bitboard(0x0102_0408_1020_4080),
    ]);
    for (name, shift, df, dr) in dirs {
        for set in &sets {
            assert_eq!(shift(*set), naive_shift(*set, df, dr), "{name} of\n{set:?}");
        }
    }
    // The wrap cases by name, so a failure reads as chess rather than hex.
    assert_eq!(Square::H1.bb().east(), Bitboard::EMPTY);
    assert_eq!(Square::A1.bb().west(), Bitboard::EMPTY);
    assert_eq!(Square::H4.bb().north_east(), Bitboard::EMPTY);
    assert_eq!(Square::A4.bb().south_west(), Bitboard::EMPTY);
    assert_eq!(Square::A8.bb().north(), Bitboard::EMPTY);
    assert_eq!(Square::A1.bb().south(), Bitboard::EMPTY);
    assert_eq!(Square::A1.bb().north(), Square::A2.bb());
    assert_eq!(Square::A1.bb().east(), Square::B1.bb());
    assert_eq!(Square::E4.bb().north_east(), Square::F5.bb());
}

#[test]
fn forward_is_north_for_white_and_south_for_black() {
    for sq in Square::all() {
        assert_eq!(sq.bb().forward(Colour::White), sq.bb().north(), "{sq}");
        assert_eq!(sq.bb().forward(Colour::Black), sq.bb().south(), "{sq}");
    }
}

// ---------------------------------------------------------------------------
// Layout guards
// ---------------------------------------------------------------------------

/// The `const` guards in the crate already refuse to compile if these drift.
/// Restated here so the numbers are visible in a test report, and so that a
/// change to them shows up as a named failure rather than a build error in an
/// unrelated file.
#[test]
fn layouts_are_the_sizes_the_design_measured() {
    use core::mem::{align_of, size_of};
    assert_eq!(size_of::<Square>(), 1);
    assert_eq!(align_of::<Square>(), 1);
    assert_eq!(
        size_of::<Option<Square>>(),
        2,
        "no niche: the reason OptSquare exists"
    );
    assert_eq!(size_of::<OptSquare>(), 1);
    assert_eq!(size_of::<Piece>(), 1);
    assert_eq!(
        size_of::<Option<Piece>>(),
        1,
        "the niche the mailbox relies on"
    );
    assert_eq!(size_of::<Colour>(), 1);
    assert_eq!(size_of::<PieceType>(), 1);
    assert_eq!(size_of::<File>(), 1);
    assert_eq!(size_of::<Rank>(), 1);
    assert_eq!(size_of::<PromoPiece>(), 1);
    assert_eq!(size_of::<Bitboard>(), 8);
    assert_eq!(align_of::<Bitboard>(), 8);
}
