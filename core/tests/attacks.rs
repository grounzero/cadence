// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `magic` and `attacks`.
//!
//! **Exhaustive, not sampled.** For every square, every occupancy subset of
//! the slider's relevant mask is looked up and compared with a naive
//! ray-walk. That is 64 × 4,096 rook lookups and 64 × 512 bishop lookups,
//! and it is the whole domain of a magic table: a magic that collides on one
//! subset is wrong for one specific occupancy pattern, which random sampling
//! finds late or never and an exhaustive walk finds now.
//!
//! The oracle computes its own relevant mask. If it read the crate's mask and
//! the crate's mask were missing a square, the subsets enumerated would never
//! set that square and the collision it causes would go untested. Squares
//! outside the mask are additionally filled at random on top of each subset,
//! because a lookup must ignore them and the table cannot know that unless
//! the mask is right.
//!
//! The oracle is `(file, rank)` stepping in plain integers, sharing nothing
//! with the shifts in `bitboard` or the tables in `attacks`.
//!
//! `BETWEEN` and `RAY` are checked over all 4,096 pairs against the naive
//! open segment and the naive full line, plus symmetry.

use cadence_core::attacks;
use cadence_core::bitboard::Bitboard;
use cadence_core::types::{Colour, Square};

// ---------------------------------------------------------------------------
// The oracle
// ---------------------------------------------------------------------------

const ROOK_DIRS: [(i8, i8); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const BISHOP_DIRS: [(i8, i8); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];
const KNIGHT_DELTAS: [(i8, i8); 8] = [
    (1, 2),
    (2, 1),
    (2, -1),
    (1, -2),
    (-1, -2),
    (-2, -1),
    (-2, 1),
    (-1, 2),
];
const KING_DELTAS: [(i8, i8); 8] = [
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -1),
    (1, -1),
];

fn coords(sq: Square) -> (i8, i8) {
    let i = i8::try_from(sq.index()).expect("fits");
    (i % 8, i / 8)
}

fn at(f: i8, r: i8) -> Option<u64> {
    ((0..8).contains(&f) && (0..8).contains(&r))
        .then(|| 1u64 << (u8::try_from(r * 8 + f).expect("in range")))
}

/// Walk each direction until the edge, including the first occupied square.
fn walk(sq: Square, occ: u64, dirs: &[(i8, i8)]) -> u64 {
    let (f0, r0) = coords(sq);
    let mut out = 0u64;
    for &(df, dr) in dirs {
        let (mut f, mut r) = (f0 + df, r0 + dr);
        while let Some(bit) = at(f, r) {
            out |= bit;
            if occ & bit != 0 {
                break;
            }
            f += df;
            r += dr;
        }
    }
    out
}

fn leaps(sq: Square, deltas: &[(i8, i8)]) -> u64 {
    let (f0, r0) = coords(sq);
    deltas
        .iter()
        .filter_map(|&(df, dr)| at(f0 + df, r0 + dr))
        .fold(0, |acc, bit| acc | bit)
}

/// The relevant-occupancy mask: the empty-board rays with the edge squares
/// removed, because a blocker on the last square of a ray changes nothing.
fn relevant_mask(sq: Square, dirs: &[(i8, i8)]) -> u64 {
    let (f0, r0) = coords(sq);
    let mut out = 0u64;
    for &(df, dr) in dirs {
        let (mut f, mut r) = (f0 + df, r0 + dr);
        // Stop one short of the edge in this direction.
        while let Some(bit) = at(f + df, r + dr) {
            let _ = bit;
            out |= at(f, r).expect("on board");
            f += df;
            r += dr;
        }
    }
    out
}

/// Every subset of `mask`, by the carry-rippler.
fn subsets(mask: u64) -> Vec<u64> {
    let mut out = Vec::with_capacity(1 << mask.count_ones());
    let mut s = 0u64;
    loop {
        out.push(s);
        s = s.wrapping_sub(mask) & mask;
        if s == 0 {
            break;
        }
    }
    out
}

/// A cheap deterministic generator for the noise outside the mask.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// ---------------------------------------------------------------------------
// Sliders: exhaustive over every subset of every mask
// ---------------------------------------------------------------------------

fn check_slider(
    name: &str,
    dirs: &[(i8, i8)],
    lookup: fn(Square, Bitboard) -> Bitboard,
    expected_mask_bits: (u32, u32),
) {
    let mut rng = 0x5EED_u64;
    let mut total = 0usize;
    for sq in Square::all() {
        let mask = relevant_mask(sq, dirs);
        let bits = mask.count_ones();
        assert!(
            (expected_mask_bits.0..=expected_mask_bits.1).contains(&bits),
            "{name} {sq}: oracle mask has {bits} bits, outside {expected_mask_bits:?}"
        );
        for occ in subsets(mask) {
            let want = walk(sq, occ, dirs);
            let got = lookup(sq, Bitboard(occ));
            assert_eq!(
                got.0,
                want,
                "{name} on {sq} under occupancy\n{:?}\ngot\n{got:?}\nwant\n{:?}",
                Bitboard(occ),
                Bitboard(want)
            );
            // Squares outside the mask must not matter. Fill them at random
            // and the answer must not move; a mask missing a square would
            // fail here for the occupancy that sets it.
            let noise = splitmix(&mut rng) & !mask;
            let noisy = occ | noise;
            let want_noisy = walk(sq, noisy, dirs);
            assert_eq!(
                want, want_noisy,
                "{name} {sq}: the oracle's own mask is wrong; noise changed the answer"
            );
            let got_noisy = lookup(sq, Bitboard(noisy));
            assert_eq!(
                got_noisy.0,
                want,
                "{name} on {sq}: occupancy outside the relevant mask changed the lookup\n{:?}",
                Bitboard(noisy)
            );
            total += 1;
        }
    }
    // The size of the domain, so a truncated enumeration is visible.
    let expected_total: usize = Square::all()
        .map(|sq| 1usize << relevant_mask(sq, dirs).count_ones())
        .sum();
    assert_eq!(
        total, expected_total,
        "{name}: not every subset was checked"
    );
}

#[test]
fn rook_attacks_equal_the_ray_walk_for_every_occupancy_subset() {
    // Corner rooks have 12 relevant bits, centre rooks 10.
    check_slider("rook", &ROOK_DIRS, attacks::rook_attacks, (10, 12));
}

#[test]
fn bishop_attacks_equal_the_ray_walk_for_every_occupancy_subset() {
    // Edge bishops have 5 relevant bits, the four centre squares 9.
    check_slider("bishop", &BISHOP_DIRS, attacks::bishop_attacks, (5, 9));
}

#[test]
fn queen_attacks_are_the_union_of_rook_and_bishop() {
    let mut rng = 0x00C0_FFEE_u64;
    for sq in Square::all() {
        for _ in 0..64 {
            let occ = Bitboard(splitmix(&mut rng) & splitmix(&mut rng));
            let want = walk(sq, occ.0, &ROOK_DIRS) | walk(sq, occ.0, &BISHOP_DIRS);
            assert_eq!(attacks::queen_attacks(sq, occ).0, want, "queen on {sq}");
            assert_eq!(
                attacks::queen_attacks(sq, occ),
                attacks::rook_attacks(sq, occ) | attacks::bishop_attacks(sq, occ)
            );
        }
    }
}

/// The empty-board totals are a fixed property of the geometry: 896 rook
/// squares (14 per square) and 560 bishop squares. A wrong table that is
/// internally consistent still has to match these.
#[test]
fn empty_board_slider_totals_match_the_geometry() {
    let rook: u32 = Square::all()
        .map(|sq| attacks::rook_attacks(sq, Bitboard::EMPTY).count())
        .sum();
    let bishop: u32 = Square::all()
        .map(|sq| attacks::bishop_attacks(sq, Bitboard::EMPTY).count())
        .sum();
    assert_eq!(rook, 64 * 14);
    assert_eq!(bishop, 560);
    assert_eq!(
        attacks::rook_attacks(Square::A1, Bitboard::EMPTY).count(),
        14
    );
    assert_eq!(
        attacks::bishop_attacks(Square::A1, Bitboard::EMPTY).count(),
        7
    );
    assert_eq!(
        attacks::bishop_attacks(Square::D4, Bitboard::EMPTY).count(),
        13
    );
}

// ---------------------------------------------------------------------------
// Leapers
// ---------------------------------------------------------------------------

#[test]
fn knight_attacks_equal_the_eight_deltas_for_every_square() {
    let mut total = 0u32;
    for sq in Square::all() {
        let want = leaps(sq, &KNIGHT_DELTAS);
        assert_eq!(attacks::knight_attacks(sq).0, want, "knight on {sq}");
        total += want.count_ones();
    }
    // 336 knight moves on an empty board, a well-known constant.
    assert_eq!(total, 336);
    assert_eq!(attacks::knight_attacks(Square::A1).count(), 2);
    assert_eq!(attacks::knight_attacks(Square::E4).count(), 8);
}

#[test]
fn king_attacks_equal_the_eight_neighbours_for_every_square() {
    let mut total = 0u32;
    for sq in Square::all() {
        let want = leaps(sq, &KING_DELTAS);
        assert_eq!(attacks::king_attacks(sq).0, want, "king on {sq}");
        total += want.count_ones();
    }
    // 420 king moves on an empty board.
    assert_eq!(total, 420);
    assert_eq!(attacks::king_attacks(Square::A1).count(), 3);
    assert_eq!(attacks::king_attacks(Square::E4).count(), 8);
}

#[test]
fn pawn_attacks_are_the_two_forward_diagonals_and_never_wrap() {
    for sq in Square::all() {
        let white = leaps(sq, &[(1, 1), (-1, 1)]);
        let black = leaps(sq, &[(1, -1), (-1, -1)]);
        assert_eq!(
            attacks::pawn_attacks(Colour::White, sq).0,
            white,
            "white pawn on {sq}"
        );
        assert_eq!(
            attacks::pawn_attacks(Colour::Black, sq).0,
            black,
            "black pawn on {sq}"
        );
        assert_eq!(attacks::pawn_attacks_bb(Colour::White, sq.bb()).0, white);
        assert_eq!(attacks::pawn_attacks_bb(Colour::Black, sq.bb()).0, black);
    }
    // The set form is the union of the singles.
    let mut rng = 0xA77A_u64;
    for _ in 0..256 {
        let pawns = Bitboard(splitmix(&mut rng) & splitmix(&mut rng));
        for c in Colour::ALL {
            let want = pawns.into_iter().fold(Bitboard::EMPTY, |acc, sq| {
                acc | attacks::pawn_attacks(c, sq)
            });
            assert_eq!(
                attacks::pawn_attacks_bb(c, pawns),
                want,
                "{c:?} pawns\n{pawns:?}"
            );
        }
    }
    // Named wrap cases.
    assert_eq!(
        attacks::pawn_attacks(Colour::White, Square::A2),
        Square::B3.bb()
    );
    assert_eq!(
        attacks::pawn_attacks(Colour::White, Square::H2),
        Square::G3.bb()
    );
    assert_eq!(
        attacks::pawn_attacks(Colour::Black, Square::A7),
        Square::B6.bb()
    );
    assert_eq!(
        attacks::pawn_attacks(Colour::Black, Square::H7),
        Square::G6.bb()
    );
    assert_eq!(
        attacks::pawn_attacks(Colour::White, Square::E8),
        Bitboard::EMPTY
    );
    assert_eq!(
        attacks::pawn_attacks(Colour::Black, Square::E1),
        Bitboard::EMPTY
    );
    assert_eq!(
        attacks::pawn_attacks(Colour::White, Square::E4),
        Square::D5.bb() | Square::F5.bb()
    );
}

// ---------------------------------------------------------------------------
// BETWEEN and RAY, all 4,096 pairs
// ---------------------------------------------------------------------------

/// The direction from `a` to `b` if they share a rank, file or diagonal.
fn direction(a: Square, b: Square) -> Option<(i8, i8)> {
    let (fa, ra) = coords(a);
    let (fb, rb) = coords(b);
    let (df, dr) = (fb - fa, rb - ra);
    if df == 0 && dr == 0 {
        return None;
    }
    if df == 0 || dr == 0 || df.abs() == dr.abs() {
        Some((df.signum(), dr.signum()))
    } else {
        None
    }
}

/// Squares strictly between `a` and `b` along their shared line.
fn naive_between(a: Square, b: Square) -> u64 {
    let Some((df, dr)) = direction(a, b) else {
        return 0;
    };
    let (fb, rb) = coords(b);
    let (mut f, mut r) = coords(a);
    let mut out = 0u64;
    loop {
        f += df;
        r += dr;
        if (f, r) == (fb, rb) {
            return out;
        }
        out |= at(f, r).expect("still on the segment");
    }
}

/// The whole line through `a` and `b`, edge to edge.
fn naive_ray(a: Square, b: Square) -> u64 {
    let Some((df, dr)) = direction(a, b) else {
        return 0;
    };
    let (f0, r0) = coords(a);
    let mut out = 0u64;
    for sign in [1i8, -1] {
        let (mut f, mut r) = (f0, r0);
        while let Some(bit) = at(f, r) {
            out |= bit;
            f += sign * df;
            r += sign * dr;
        }
    }
    out
}

#[test]
fn between_is_the_open_segment_for_aligned_pairs_and_empty_otherwise() {
    let mut aligned_pairs = 0u32;
    for a in Square::all() {
        for b in Square::all() {
            let want = naive_between(a, b);
            let got = attacks::between(a, b);
            assert_eq!(got.0, want, "between({a}, {b})\ngot\n{got:?}");
            assert_eq!(
                attacks::between(b, a),
                got,
                "between({a}, {b}) is not symmetric"
            );
            assert!(
                !got.contains(a) && !got.contains(b),
                "between({a}, {b}) is open"
            );
            if direction(a, b).is_some() {
                aligned_pairs += 1;
            } else {
                assert_eq!(got, Bitboard::EMPTY, "between({a}, {b}): not aligned");
            }
        }
    }
    // 64 × (14 rook + bishop) directed pairs: 896 + 560.
    assert_eq!(aligned_pairs, 896 + 560);
    assert_eq!(attacks::between(Square::A1, Square::A1), Bitboard::EMPTY);
    assert_eq!(attacks::between(Square::A1, Square::A2), Bitboard::EMPTY);
    assert_eq!(attacks::between(Square::A1, Square::A3), Square::A2.bb());
    assert_eq!(
        attacks::between(Square::A1, Square::H8),
        Square::B2.bb()
            | Square::C3.bb()
            | Square::D4.bb()
            | Square::E5.bb()
            | Square::F6.bb()
            | Square::G7.bb()
    );
    assert_eq!(attacks::between(Square::A1, Square::B3), Bitboard::EMPTY);
}

#[test]
fn ray_is_the_full_line_for_aligned_pairs_and_empty_otherwise() {
    for a in Square::all() {
        for b in Square::all() {
            let want = naive_ray(a, b);
            let got = attacks::ray(a, b);
            assert_eq!(got.0, want, "ray({a}, {b})\ngot\n{got:?}");
            assert_eq!(attacks::ray(b, a), got, "ray({a}, {b}) is not symmetric");
            if direction(a, b).is_some() {
                assert!(
                    got.contains(a) && got.contains(b),
                    "ray({a}, {b}) holds both ends"
                );
                assert!(
                    (got & !attacks::between(a, b)).contains(a),
                    "ray contains between"
                );
                assert_eq!(
                    got & attacks::between(a, b),
                    attacks::between(a, b),
                    "between({a}, {b}) ⊆ ray({a}, {b})"
                );
                // The line is the same for any two of its squares.
                for c in got {
                    if c != a {
                        assert_eq!(attacks::ray(a, c), got, "ray({a}, {c}) ≠ ray({a}, {b})");
                    }
                    assert!(attacks::aligned(a, b, c), "aligned({a}, {b}, {c})");
                }
            } else {
                assert_eq!(got, Bitboard::EMPTY, "ray({a}, {b}): not aligned");
                assert!(!attacks::aligned(a, b, a));
            }
        }
    }
    assert_eq!(attacks::ray(Square::E4, Square::E4), Bitboard::EMPTY);
    assert_eq!(attacks::ray(Square::A1, Square::H8).count(), 8);
    assert_eq!(
        attacks::ray(Square::A1, Square::B2),
        attacks::ray(Square::G7, Square::H8)
    );
    assert_eq!(attacks::ray(Square::E4, Square::E5), Bitboard::FILE_E);
    assert_eq!(attacks::ray(Square::A4, Square::C4), Bitboard::RANK_4);
    assert_eq!(attacks::ray(Square::A1, Square::B3), Bitboard::EMPTY);
    assert!(attacks::aligned(Square::E1, Square::E8, Square::E4));
    assert!(!attacks::aligned(Square::E1, Square::E8, Square::D4));
}
