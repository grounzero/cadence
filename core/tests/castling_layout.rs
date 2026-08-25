// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `castling`: rights and layout, with no FEN parser and no move
//! generation involved.
//!
//! The layout is built directly from the Scharnagl back ranks (the same
//! decoder the corpus's DFRC arrays were checked against) for all 960
//! arrays × both colours, and every field is compared with a naive statement
//! of the rules: destinations fixed by side, `king_path` the closed segment,
//! `must_be_empty` both segments minus the two origins, `update_mask` clearing
//! exactly the rights a move from or to that square kills.
//!
//! The degenerate-castle census is the external number: over the 1,920
//! castles, **552 move exactly one piece, 1,368 move both, 0 move neither**
//! (corpus section 5). It is computable from the layout alone, and it is what makes
//! "clear both origins before setting either destination" a rule about 28.7%
//! of DFRC castles rather than a corner case.

mod support;

use cadence_core::bitboard::Bitboard;
use cadence_core::castling::{CastleSide, CastlingLayout, CastlingRights, ci};
use cadence_core::types::{Colour, File, OptSquare, Rank, Square};
use support::generative as generate;

// ---------------------------------------------------------------------------
// ci and CastlingRights
// ---------------------------------------------------------------------------

#[test]
fn slot_order_is_the_fen_token_order() {
    assert_eq!(ci(Colour::White, CastleSide::King), 0);
    assert_eq!(ci(Colour::White, CastleSide::Queen), 1);
    assert_eq!(ci(Colour::Black, CastleSide::King), 2);
    assert_eq!(ci(Colour::Black, CastleSide::Queen), 3);
    for c in Colour::ALL {
        for s in CastleSide::ALL {
            assert_eq!(CastlingRights::bit(c, s), 1 << ci(c, s));
        }
        assert_eq!(
            CastlingRights::both(c),
            CastlingRights::bit(c, CastleSide::King) | CastlingRights::bit(c, CastleSide::Queen)
        );
    }
    assert_eq!(CastlingRights::both(Colour::White), 0b0011);
    assert_eq!(CastlingRights::both(Colour::Black), 0b1100);
}

/// Every one of the sixteen values, every predicate.
#[test]
fn rights_predicates_over_all_sixteen_values() {
    for bits in 0..16u8 {
        let r = CastlingRights::from_bits(bits);
        assert_eq!(r.bits(), bits);
        assert_eq!(r.zobrist_index(), usize::from(bits));
        assert_eq!(r.is_empty(), bits == 0);
        for c in Colour::ALL {
            for s in CastleSide::ALL {
                assert_eq!(
                    r.has(c, s),
                    bits & (1 << ci(c, s)) != 0,
                    "{bits:04b} {c:?} {s:?}"
                );
            }
            assert_eq!(
                r.any(c),
                bits & CastlingRights::both(c) != 0,
                "{bits:04b} {c:?}"
            );
        }
        for mask in 0..16u8 {
            assert_eq!(r.masked(mask).bits(), bits & mask);
        }
        // Masking can only remove.
        assert_eq!(r.masked(0xFF), r);
        assert_eq!(r.masked(0), CastlingRights::NONE);
    }
    assert_eq!(
        CastlingRights::from_bits(0xF0),
        CastlingRights::NONE,
        "high bits ignored"
    );
    assert_eq!(CastlingRights::from_bits(0xFF), CastlingRights::ALL);
    assert_eq!(CastlingRights::default(), CastlingRights::NONE);
    assert!(CastlingRights::ALL.has(Colour::Black, CastleSide::Queen));
    assert!(!CastlingRights::NONE.any(Colour::White));
}

// ---------------------------------------------------------------------------
// The layout, over all 960 arrays × both colours
// ---------------------------------------------------------------------------

/// The naive closed segment along a rank, both ends included.
fn segment(a: Square, b: Square) -> Bitboard {
    assert_eq!(a.rank(), b.rank());
    let (lo, hi) = if a.file() <= b.file() { (a, b) } else { (b, a) };
    let mut out = Bitboard::EMPTY;
    for f in lo.file().index()..=hi.file().index() {
        out.set(Square::from_file_rank(
            File::new(u8::try_from(f).expect("fits")),
            a.rank(),
        ));
    }
    out
}

/// One colour's castling geometry from a Scharnagl back rank.
struct Array {
    king: Square,
    /// Kingside rook, queenside rook.
    rooks: [Square; 2],
}

fn array(n: u32, c: Colour) -> Array {
    let rank = Rank::One.relative(c);
    let back = generate::scharnagl(n);
    let king_file = back.find('k').expect("a king");
    let rook_files: Vec<usize> = back.match_indices('r').map(|(i, _)| i).collect();
    assert_eq!(rook_files.len(), 2);
    assert!(rook_files[0] < king_file && king_file < rook_files[1]);
    let sq = |f: usize| Square::from_file_rank(File::new(u8::try_from(f).expect("fits")), rank);
    Array {
        king: sq(king_file),
        rooks: [sq(rook_files[1]), sq(rook_files[0])],
    }
}

/// Both colours of array `n`, as the layout constructor takes them.
fn layout_inputs(n: u32) -> ([OptSquare; 2], [OptSquare; 4]) {
    let w = array(n, Colour::White);
    let b = array(n, Colour::Black);
    (
        [OptSquare::some(w.king), OptSquare::some(b.king)],
        [
            OptSquare::some(w.rooks[0]),
            OptSquare::some(w.rooks[1]),
            OptSquare::some(b.rooks[0]),
            OptSquare::some(b.rooks[1]),
        ],
    )
}

#[test]
fn destinations_are_fixed_by_side_and_paths_are_the_closed_segments() {
    for n in 0..960 {
        let (kings, rooks) = layout_inputs(n);
        let layout = CastlingLayout::new(kings, rooks);
        assert_eq!(layout.king_from, kings, "array {n}");
        assert_eq!(layout.rook_from, rooks, "array {n}");

        for c in Colour::ALL {
            let rank = Rank::One.relative(c);
            let kf = kings[c.index()].get().expect("king");
            for s in CastleSide::ALL {
                let i = ci(c, s);
                let rf = rooks[i].get().expect("rook");
                let (kt_file, rt_file) = match s {
                    CastleSide::King => (File::G, File::F),
                    CastleSide::Queen => (File::C, File::D),
                };
                let kt = Square::from_file_rank(kt_file, rank);
                let rt = Square::from_file_rank(rt_file, rank);
                let ctx = format!("array {n} {c:?} {s:?}: K{kf}->{kt} R{rf}->{rt}");

                assert_eq!(layout.king_to[i].get(), Some(kt), "{ctx}: king_to");
                assert_eq!(layout.rook_to[i].get(), Some(rt), "{ctx}: rook_to");

                let king_path = segment(kf, kt);
                assert_eq!(layout.king_path[i], king_path, "{ctx}: king_path");
                assert!(
                    layout.king_path[i].contains(kf),
                    "{ctx}: king_path holds kf"
                );
                assert!(
                    layout.king_path[i].contains(kt),
                    "{ctx}: king_path holds kt"
                );

                let must = (segment(kf, kt) | segment(rf, rt)) & !(kf.bb() | rf.bb());
                assert_eq!(layout.must_be_empty[i], must, "{ctx}: must_be_empty");
                assert!(!layout.must_be_empty[i].contains(kf), "{ctx}: excludes kf");
                assert!(!layout.must_be_empty[i].contains(rf), "{ctx}: excludes rf");
                // The rook side of the king is where the king travels; the
                // squares behind the king are never required empty.
                assert!(
                    (layout.must_be_empty[i] & !Bitboard::rank(rank)).is_empty(),
                    "{ctx}: must_be_empty stays on the back rank"
                );
            }
        }
    }
}

#[test]
fn update_mask_clears_exactly_the_rights_a_move_touching_the_square_kills() {
    for n in 0..960 {
        let (kings, rooks) = layout_inputs(n);
        let layout = CastlingLayout::new(kings, rooks);
        for sq in Square::all() {
            let mut expected = 0b1111u8;
            for c in Colour::ALL {
                if kings[c.index()].get() == Some(sq) {
                    expected &= !CastlingRights::both(c);
                }
                for s in CastleSide::ALL {
                    if rooks[ci(c, s)].get() == Some(sq) {
                        expected &= !CastlingRights::bit(c, s);
                    }
                }
            }
            assert_eq!(
                layout.update_mask[sq.index()] & 0b1111,
                expected,
                "array {n}: update_mask[{sq}]"
            );
        }
        // Applied the way make_move applies it: a rook leaving its square and
        // capturing the other rook kills both rights in one AND.
        let all = CastlingRights::ALL;
        let wk = rooks[ci(Colour::White, CastleSide::King)]
            .get()
            .expect("rook");
        let bk = rooks[ci(Colour::Black, CastleSide::King)]
            .get()
            .expect("rook");
        let after = all.masked(layout.update_mask[wk.index()] & layout.update_mask[bk.index()]);
        assert!(
            !after.has(Colour::White, CastleSide::King),
            "array {n}: mover's right"
        );
        assert!(
            !after.has(Colour::Black, CastleSide::King),
            "array {n}: victim's right"
        );
        assert!(
            after.has(Colour::White, CastleSide::Queen),
            "array {n}: untouched"
        );
        assert!(
            after.has(Colour::Black, CastleSide::Queen),
            "array {n}: untouched"
        );
        // A king move kills both of its rights and nothing of the opponent's.
        let wkf = kings[Colour::White.index()].get().expect("king");
        let after = all.masked(layout.update_mask[wkf.index()]);
        assert_eq!(
            after.bits(),
            CastlingRights::both(Colour::Black),
            "array {n}: king move"
        );
    }
}

/// The census: 552 castles move exactly one piece, 1,368 move both, none
/// move neither. Corpus section 5, reproduced from the layout alone.
///
/// Counted per colour: each colour's 960 arrays × 2 sides are the 1,920
/// castles, and the two colours are mirrors, so both must give the same
/// numbers: a rank-8 mirroring bug in the layout shows up here as one
/// colour's census disagreeing with the other's.
#[test]
fn degenerate_castle_census_is_552_one_piece_1368_two_piece_0_zero_piece() {
    for c in Colour::ALL {
        let mut one = 0;
        let mut two = 0;
        let mut zero = 0;
        let mut king_still = 0;
        let mut rook_still = 0;
        for n in 0..960 {
            let (kings, rooks) = layout_inputs(n);
            let layout = CastlingLayout::new(kings, rooks);
            for s in CastleSide::ALL {
                let i = ci(c, s);
                let kf = layout.king_from[c.index()].get().expect("king");
                let rf = layout.rook_from[i].get().expect("rook");
                let kt = layout.king_to[i].get().expect("king_to");
                let rt = layout.rook_to[i].get().expect("rook_to");
                let king_moves = kf != kt;
                let rook_moves = rf != rt;
                if !king_moves {
                    king_still += 1;
                }
                if !rook_moves {
                    rook_still += 1;
                }
                match (king_moves, rook_moves) {
                    (true, true) => two += 1,
                    (false, false) => zero += 1,
                    _ => one += 1,
                }
            }
        }
        assert_eq!(one + two + zero, 1920, "{c:?}");
        assert_eq!(one, 552, "{c:?}: castles where exactly one piece moves");
        assert_eq!(two, 1368, "{c:?}: castles where both pieces move");
        assert_eq!(zero, 0, "{c:?}: castles where neither piece moves");
        // The one-piece castles split into king-stays and rook-stays; both
        // shapes exist, and neither is rare.
        assert_eq!(king_still + rook_still, 552, "{c:?}");
        assert!(king_still > 0 && rook_still > 0, "{c:?}");
    }
}

#[test]
fn absent_rights_have_no_geometry_and_reject_any_occupancy() {
    let none = CastlingLayout::none();
    for i in 0..4 {
        assert!(none.rook_from[i].is_none());
        assert!(none.king_to[i].is_none());
        assert!(none.rook_to[i].is_none());
        assert_eq!(none.king_path[i], Bitboard::EMPTY);
        assert_eq!(none.must_be_empty[i], Bitboard::FULL);
    }
    assert!(none.king_from[0].is_none() && none.king_from[1].is_none());
    for sq in Square::all() {
        assert_eq!(none.update_mask[sq.index()] & 0b1111, 0b1111, "{sq}");
    }

    // One right only: the other three slots are absent, and the layout for
    // the one present slot is what the full layout has.
    let partial = CastlingLayout::new(
        [OptSquare::some(Square::E1), OptSquare::NONE],
        [
            OptSquare::some(Square::H1),
            OptSquare::NONE,
            OptSquare::NONE,
            OptSquare::NONE,
        ],
    );
    assert_eq!(partial.king_to[0].get(), Some(Square::G1));
    assert_eq!(partial.rook_to[0].get(), Some(Square::F1));
    assert_eq!(partial.king_path[0], segment(Square::E1, Square::G1));
    assert_eq!(partial.must_be_empty[0], Square::F1.bb() | Square::G1.bb());
    for i in 1..4 {
        assert!(partial.rook_from[i].is_none());
        assert_eq!(partial.must_be_empty[i], Bitboard::FULL);
        assert_eq!(partial.king_path[i], Bitboard::EMPTY);
    }
    assert_eq!(partial.update_mask[Square::E1.index()] & 0b1111, 0b1100);
    assert_eq!(partial.update_mask[Square::H1.index()] & 0b1111, 0b1110);
    assert_eq!(partial.update_mask[Square::A1.index()] & 0b1111, 0b1111);
}
