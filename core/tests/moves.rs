// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `mv`.
//!
//! Encode/decode over all 4,096 from/to pairs × every constructor, plus the
//! truth table for the flag predicates. The round trip alone would pass a
//! flag numbering in which castling reads as a capture, and "`is_capture` is
//! false for castling" is the most load-bearing property of the numbering:
//! SEE, MVV-LVA and qsearch all read that bit. So the predicates are checked
//! per constructor, not inferred from the round trip.
//!
//! `castle_side` is derived from the two files, never stored, and is checked
//! for every same-rank pair.
//!
//! `MoveList` is filled to its full capacity of 256, because a `u8` length
//! wraps to zero there and reports an empty list; the corpus's 218-move
//! position cannot reach that, which is why it is checked here.

use cadence_core::castling::CastleSide;
use cadence_core::mv::{MAX_MOVES, Move, MoveList, parse_uci, to_uci};
use cadence_core::types::{PromoPiece, Square};

/// What a constructor builds, from which every predicate value follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Quiet,
    DoublePush,
    Castle,
    Capture,
    EnPassant,
    Promo(PromoPiece),
    PromoCapture(PromoPiece),
}

impl Kind {
    fn capture(self) -> bool {
        matches!(
            self,
            Kind::Capture | Kind::EnPassant | Kind::PromoCapture(_)
        )
    }
    fn promotion(self) -> Option<PromoPiece> {
        match self {
            Kind::Promo(p) | Kind::PromoCapture(p) => Some(p),
            _ => None,
        }
    }
}

/// Every constructor, paired with what it claims to build.
#[derive(Clone, Copy, Debug)]
struct Shape {
    name: &'static str,
    make: fn(Square, Square) -> Move,
    kind: Kind,
}

// Function pointers cannot capture, so each promotion piece gets its own fn.
fn pn(f: Square, t: Square) -> Move {
    Move::new_promotion(f, t, PromoPiece::Knight)
}
fn pb(f: Square, t: Square) -> Move {
    Move::new_promotion(f, t, PromoPiece::Bishop)
}
fn pr(f: Square, t: Square) -> Move {
    Move::new_promotion(f, t, PromoPiece::Rook)
}
fn pq(f: Square, t: Square) -> Move {
    Move::new_promotion(f, t, PromoPiece::Queen)
}
fn pcn(f: Square, t: Square) -> Move {
    Move::new_promotion_capture(f, t, PromoPiece::Knight)
}
fn pcb(f: Square, t: Square) -> Move {
    Move::new_promotion_capture(f, t, PromoPiece::Bishop)
}
fn pcr(f: Square, t: Square) -> Move {
    Move::new_promotion_capture(f, t, PromoPiece::Rook)
}
fn pcq(f: Square, t: Square) -> Move {
    Move::new_promotion_capture(f, t, PromoPiece::Queen)
}

fn shapes() -> Vec<Shape> {
    let out = vec![
        Shape {
            name: "quiet",
            make: Move::new_quiet,
            kind: Kind::Quiet,
        },
        Shape {
            name: "double push",
            make: Move::new_double_push,
            kind: Kind::DoublePush,
        },
        Shape {
            name: "castle",
            make: Move::new_castle,
            kind: Kind::Castle,
        },
        Shape {
            name: "capture",
            make: Move::new_capture,
            kind: Kind::Capture,
        },
        Shape {
            name: "en passant",
            make: Move::new_en_passant,
            kind: Kind::EnPassant,
        },
        Shape {
            name: "promo n",
            make: pn,
            kind: Kind::Promo(PromoPiece::Knight),
        },
        Shape {
            name: "promo b",
            make: pb,
            kind: Kind::Promo(PromoPiece::Bishop),
        },
        Shape {
            name: "promo r",
            make: pr,
            kind: Kind::Promo(PromoPiece::Rook),
        },
        Shape {
            name: "promo q",
            make: pq,
            kind: Kind::Promo(PromoPiece::Queen),
        },
        Shape {
            name: "promo-capture n",
            make: pcn,
            kind: Kind::PromoCapture(PromoPiece::Knight),
        },
        Shape {
            name: "promo-capture b",
            make: pcb,
            kind: Kind::PromoCapture(PromoPiece::Bishop),
        },
        Shape {
            name: "promo-capture r",
            make: pcr,
            kind: Kind::PromoCapture(PromoPiece::Rook),
        },
        Shape {
            name: "promo-capture q",
            make: pcq,
            kind: Kind::PromoCapture(PromoPiece::Queen),
        },
    ];
    assert_eq!(
        out.len(),
        13,
        "thirteen of the sixteen flag values are assigned"
    );
    out
}

/// Every from/to pair. `from == to` is included: the encoding must round-trip
/// it even though no real move has it, because `from_bits` accepts any
/// pattern.
fn all_pairs() -> impl Iterator<Item = (Square, Square)> {
    Square::all().flat_map(|f| Square::all().map(move |t| (f, t)))
}

// ---------------------------------------------------------------------------
// Encode / decode
// ---------------------------------------------------------------------------

#[test]
fn every_constructor_round_trips_over_all_4096_pairs() {
    let mut seen = std::collections::HashSet::new();
    for shape in shapes() {
        for (from, to) in all_pairs() {
            let m = (shape.make)(from, to);
            assert_eq!(m.from_sq(), from, "{}: from", shape.name);
            assert_eq!(m.to_sq(), to, "{}: to", shape.name);
            assert_eq!(
                m.from_to(),
                from.index() | (to.index() << 6),
                "{}: butterfly index",
                shape.name
            );
            let bits = m.to_bits();
            assert_eq!(
                Move::from_bits(bits),
                m,
                "{}: from_bits(to_bits)",
                shape.name
            );
            assert_eq!(
                usize::from(bits & 63),
                from.index(),
                "{}: from in the low bits",
                shape.name
            );
            assert_eq!(
                usize::from((bits >> 6) & 63),
                to.index(),
                "{}: to above it",
                shape.name
            );
            assert!(
                seen.insert(bits),
                "{}: {from}{to} collides with another constructor's encoding",
                shape.name
            );
        }
    }
    // 13 constructors × 4,096 pairs, all distinct.
    assert_eq!(seen.len(), 13 * 4096);
}

/// `from_bits` is total over `u16`, and `to_bits` inverts it, the reserved
/// flag values included, because the transposition table stores whatever it
/// was given.
#[test]
fn from_bits_is_total_and_to_bits_inverts_it() {
    for bits in 0..=u16::MAX {
        let m = Move::from_bits(bits);
        assert_eq!(m.to_bits(), bits);
        assert_eq!(m.from_sq().index(), usize::from(bits & 63));
        assert_eq!(m.to_sq().index(), usize::from((bits >> 6) & 63));
    }
}

// ---------------------------------------------------------------------------
// The flag truth table
// ---------------------------------------------------------------------------

#[test]
fn flag_predicates_match_the_constructor_for_every_shape() {
    for shape in shapes() {
        // Two representative pairs are enough for the predicates: they read
        // only the nibble, and the round-trip test already covers all pairs.
        for (from, to) in [(Square::E2, Square::E4), (Square::H7, Square::G8)] {
            let m = (shape.make)(from, to);
            let ctx = format!("{} {from}{to} = {m:?}", shape.name);
            let k = shape.kind;
            assert_eq!(m.is_capture(), k.capture(), "{ctx}: is_capture");
            assert_eq!(
                m.is_promotion(),
                k.promotion().is_some(),
                "{ctx}: is_promotion"
            );
            assert_eq!(m.promotion_piece(), k.promotion(), "{ctx}: promotion_piece");
            assert_eq!(m.is_castle(), k == Kind::Castle, "{ctx}: is_castle");
            assert_eq!(
                m.is_en_passant(),
                k == Kind::EnPassant,
                "{ctx}: is_en_passant"
            );
            assert_eq!(
                m.is_double_push(),
                k == Kind::DoublePush,
                "{ctx}: is_double_push"
            );
            assert_eq!(
                m.is_noisy(),
                k.capture() || k.promotion().is_some(),
                "{ctx}: is_noisy == is_capture || is_promotion"
            );
            assert!(!m.is_null(), "{ctx}: a real move is never null");
        }
    }
}

/// The three named properties, stated on their own so a failure names them.
#[test]
fn castling_is_not_a_capture_and_en_passant_is() {
    let castle = Move::new_castle(Square::E1, Square::H1);
    assert!(castle.is_castle());
    assert!(
        !castle.is_capture(),
        "castling lands on a friendly rook; not a capture"
    );
    assert!(!castle.is_noisy(), "castling is quiet for qsearch");
    assert!(!castle.is_promotion());

    let ep = Move::new_en_passant(Square::E5, Square::D6);
    assert!(ep.is_en_passant());
    assert!(
        ep.is_capture(),
        "en passant captures a pawn that is not on `to`"
    );
    assert!(ep.is_noisy());

    let pc = Move::new_promotion_capture(Square::B7, Square::C8, PromoPiece::Queen);
    assert!(pc.is_capture() && pc.is_promotion() && pc.is_noisy());
    assert_eq!(pc.promotion_piece(), Some(PromoPiece::Queen));

    let p = Move::new_promotion(Square::A7, Square::A8, PromoPiece::Knight);
    assert!(!p.is_capture() && p.is_promotion() && p.is_noisy());
    assert_eq!(p.promotion_piece(), Some(PromoPiece::Knight));
}

#[test]
fn null_move_is_the_zero_pattern_and_only_it() {
    assert!(Move::NULL.is_null());
    assert_eq!(Move::NULL.to_bits(), 0);
    assert_eq!(Move::NULL.from_to(), 0);
    assert!(!Move::NULL.is_capture());
    assert!(!Move::NULL.is_promotion());
    assert!(!Move::NULL.is_castle());
    assert!(!Move::NULL.is_noisy());
    assert_eq!(Move::from_bits(0), Move::NULL);
    // Only the all-zero pattern is null: a1a1 with any other flag is not.
    for bits in 1..=u16::MAX {
        assert!(
            !Move::from_bits(bits).is_null(),
            "0x{bits:04x} must not be null"
        );
    }
    assert!(!Move::new_quiet(Square::A1, Square::A2).is_null());
    assert!(!Move::new_quiet(Square::B1, Square::A1).is_null());
}

// ---------------------------------------------------------------------------
// castle_side, derived from the files
// ---------------------------------------------------------------------------

#[test]
fn castle_side_is_kingside_iff_the_rook_file_is_higher() {
    let mut pairs = 0;
    for kf in Square::all() {
        for rf in Square::all() {
            if kf == rf || kf.rank() != rf.rank() {
                continue;
            }
            let m = Move::new_castle(kf, rf);
            let want = if rf.file() > kf.file() {
                CastleSide::King
            } else {
                CastleSide::Queen
            };
            assert_eq!(m.castle_side(), want, "castle {kf}{rf}");
            pairs += 1;
        }
    }
    // 8 ranks × 8 × 7 ordered pairs.
    assert_eq!(pairs, 8 * 56);
    assert_eq!(
        Move::new_castle(Square::E1, Square::H1).castle_side(),
        CastleSide::King
    );
    assert_eq!(
        Move::new_castle(Square::E1, Square::A1).castle_side(),
        CastleSide::Queen
    );
    // b1/c1: the rook is kingside even though it is on the queen's half.
    assert_eq!(
        Move::new_castle(Square::B1, Square::C1).castle_side(),
        CastleSide::King
    );
    assert_eq!(
        Move::new_castle(Square::G8, Square::H8).castle_side(),
        CastleSide::King
    );
    assert_eq!(
        Move::new_castle(Square::C8, Square::A8).castle_side(),
        CastleSide::Queen
    );
}

// ---------------------------------------------------------------------------
// Spelling
// ---------------------------------------------------------------------------

#[test]
fn king_takes_rook_spelling_is_from_to_and_a_promotion_suffix() {
    assert_eq!(
        Move::new_quiet(Square::E2, Square::E3).to_uci_chess960(),
        "e2e3"
    );
    assert_eq!(
        Move::new_double_push(Square::E2, Square::E4).to_uci_chess960(),
        "e2e4"
    );
    assert_eq!(
        Move::new_capture(Square::E4, Square::D5).to_uci_chess960(),
        "e4d5"
    );
    assert_eq!(
        Move::new_en_passant(Square::E5, Square::D6).to_uci_chess960(),
        "e5d6"
    );
    assert_eq!(
        Move::new_castle(Square::E1, Square::H1).to_uci_chess960(),
        "e1h1"
    );
    assert_eq!(
        Move::new_castle(Square::F8, Square::G8).to_uci_chess960(),
        "f8g8"
    );
    assert_eq!(
        Move::new_promotion(Square::E7, Square::E8, PromoPiece::Queen).to_uci_chess960(),
        "e7e8q"
    );
    assert_eq!(
        Move::new_promotion(Square::A7, Square::A8, PromoPiece::Knight).to_uci_chess960(),
        "a7a8n"
    );
    assert_eq!(
        Move::new_promotion_capture(Square::B7, Square::C8, PromoPiece::Rook).to_uci_chess960(),
        "b7c8r"
    );
    assert_eq!(
        Move::new_promotion_capture(Square::B2, Square::A1, PromoPiece::Bishop).to_uci_chess960(),
        "b2a1b"
    );
    assert_eq!(Move::NULL.to_uci_chess960(), "0000");
}

#[test]
fn debug_names_the_squares_and_the_flag() {
    assert_eq!(
        format!("{:?}", Move::new_castle(Square::E1, Square::H1)),
        "e1h1[Castle]"
    );
    assert_eq!(
        format!("{:?}", Move::new_quiet(Square::G1, Square::F3)),
        "g1f3[Quiet]"
    );
    assert_eq!(
        format!("{:?}", Move::new_double_push(Square::E2, Square::E4)),
        "e2e4[DoublePush]"
    );
    assert_eq!(
        format!("{:?}", Move::new_capture(Square::E4, Square::D5)),
        "e4d5[Capture]"
    );
    assert_eq!(
        format!("{:?}", Move::new_en_passant(Square::E5, Square::D6)),
        "e5d6[EnPassant]"
    );
    assert_eq!(
        format!(
            "{:?}",
            Move::new_promotion(Square::E7, Square::E8, PromoPiece::Queen)
        ),
        "e7e8q[PromoQ]"
    );
    assert_eq!(
        format!(
            "{:?}",
            Move::new_promotion_capture(Square::B7, Square::C8, PromoPiece::Knight)
        ),
        "b7c8n[PromoCapN]"
    );
    assert_eq!(format!("{:?}", Move::NULL), "0000[Null]");
    // A reserved nibble is named as such rather than misread as a real flag.
    assert_eq!(
        format!("{:?}", Move::from_bits(0x3000)),
        "a1a1[Reserved(0b0011)]"
    );
}

/// The GUI-facing spelling needs the legal move list. Three hand-built lists
/// cover the three branches: an ordinary castle spells as king-to-destination
/// unless the king does not move at all, or a quiet king move to that same
/// destination is also legal.
#[test]
fn standard_spelling_of_castling_depends_on_the_position() {
    // Standard array: e1h1 castle, no other king move to g1. Non-960 says
    // "e1g1", 960 says "e1h1"; both parse in both modes.
    let castle = Move::new_castle(Square::E1, Square::H1);
    let mut legal = MoveList::new();
    legal.push(castle);
    legal.push(Move::new_quiet(Square::E1, Square::F1));
    legal.push(Move::new_quiet(Square::E1, Square::D1));
    assert_eq!(to_uci(castle, &legal, false), "e1g1");
    assert_eq!(to_uci(castle, &legal, true), "e1h1");
    assert_eq!(parse_uci(&legal, "e1g1"), Some(castle));
    assert_eq!(parse_uci(&legal, "e1h1"), Some(castle));
    assert_eq!(
        parse_uci(&legal, "e1f1"),
        Some(Move::new_quiet(Square::E1, Square::F1))
    );

    // The ambiguity proof: Kf1, Rh1, g1 empty. f1g1 is a legal quiet move,
    // so the castle cannot be spelled f1g1 in either mode.
    let castle = Move::new_castle(Square::F1, Square::H1);
    let quiet = Move::new_quiet(Square::F1, Square::G1);
    let mut legal = MoveList::new();
    legal.push(quiet);
    legal.push(castle);
    assert_eq!(to_uci(castle, &legal, false), "f1h1");
    assert_eq!(to_uci(castle, &legal, true), "f1h1");
    assert_eq!(to_uci(quiet, &legal, false), "f1g1");
    assert_eq!(
        parse_uci(&legal, "f1g1"),
        Some(quiet),
        "the quiet move owns f1g1"
    );
    assert_eq!(parse_uci(&legal, "f1h1"), Some(castle));

    // Kg1, Rh1: the king does not move, so "g1g1" is not a UCI string and
    // the spelling falls back to king-takes-rook.
    let castle = Move::new_castle(Square::G1, Square::H1);
    let mut legal = MoveList::new();
    legal.push(castle);
    assert_eq!(to_uci(castle, &legal, false), "g1h1");
    assert_eq!(to_uci(castle, &legal, true), "g1h1");
    assert_eq!(parse_uci(&legal, "g1h1"), Some(castle));

    // Queenside, Black: Ke8, Ra8. Non-960 "e8c8", 960 "e8a8".
    let castle = Move::new_castle(Square::E8, Square::A8);
    let mut legal = MoveList::new();
    legal.push(castle);
    assert_eq!(to_uci(castle, &legal, false), "e8c8");
    assert_eq!(to_uci(castle, &legal, true), "e8a8");
    assert_eq!(parse_uci(&legal, "e8c8"), Some(castle));
    assert_eq!(parse_uci(&legal, "e8a8"), Some(castle));

    // Kb1, Rc1: the rook is kingside; king to g1, non-960 "b1g1".
    let castle = Move::new_castle(Square::B1, Square::C1);
    let mut legal = MoveList::new();
    legal.push(castle);
    assert_eq!(to_uci(castle, &legal, false), "b1g1");
    assert_eq!(parse_uci(&legal, "b1g1"), Some(castle));
    assert_eq!(parse_uci(&legal, "b1c1"), Some(castle));
}

#[test]
fn ordinary_moves_spell_the_same_in_both_modes_and_parsing_rejects_illegal_input() {
    let quiet = Move::new_quiet(Square::G1, Square::F3);
    let push = Move::new_double_push(Square::E2, Square::E4);
    let promo = Move::new_promotion(Square::E7, Square::E8, PromoPiece::Queen);
    let under = Move::new_promotion(Square::E7, Square::E8, PromoPiece::Knight);
    let ep = Move::new_en_passant(Square::D5, Square::E6);
    let mut legal = MoveList::new();
    for m in [quiet, push, promo, under, ep] {
        legal.push(m);
    }
    for m in [quiet, push, promo, under, ep] {
        for chess960 in [false, true] {
            let s = to_uci(m, &legal, chess960);
            assert_eq!(
                s,
                m.to_uci_chess960(),
                "{m:?}: not castling, so one spelling"
            );
            assert_eq!(parse_uci(&legal, &s), Some(m), "{m:?} via `{s}`");
        }
    }
    // Promotions are distinguished by their suffix, and a bare "e7e8" is not
    // any of them.
    assert_eq!(parse_uci(&legal, "e7e8q"), Some(promo));
    assert_eq!(parse_uci(&legal, "e7e8n"), Some(under));
    assert_eq!(parse_uci(&legal, "e7e8"), None);
    assert_eq!(parse_uci(&legal, "e7e8r"), None, "not in the list");
    // Not legal, malformed, or the null move.
    assert_eq!(parse_uci(&legal, "e2e3"), None);
    assert_eq!(parse_uci(&legal, "0000"), None);
    assert_eq!(parse_uci(&legal, ""), None);
    assert_eq!(parse_uci(&legal, "g1f3x"), None);
    assert_eq!(parse_uci(&legal, "G1F3"), None);
    assert_eq!(parse_uci(&legal, "z9a1"), None);
    assert_eq!(parse_uci(&MoveList::new(), "e2e4"), None);
}

// ---------------------------------------------------------------------------
// MoveList
// ---------------------------------------------------------------------------

#[test]
fn move_list_holds_its_full_capacity_without_wrapping() {
    let mut list = MoveList::new();
    assert!(list.is_empty());
    assert_eq!(list.len(), 0);
    assert_eq!(list.as_slice().len(), 0);

    let moves: Vec<Move> = (0..MAX_MOVES)
        .map(|i| Move::from_bits(u16::try_from(i).expect("fits") + 1))
        .collect();
    for (i, m) in moves.iter().enumerate() {
        list.push(*m);
        assert_eq!(list.len(), i + 1, "len after {} pushes", i + 1);
        assert!(!list.is_empty());
    }
    // 256, not 0.
    assert_eq!(list.len(), MAX_MOVES);
    assert_eq!(list.as_slice().len(), MAX_MOVES);
    assert_eq!(list.as_slice(), moves.as_slice());
    assert_eq!(list.iter().collect::<Vec<_>>(), moves);
    for m in &moves {
        assert!(list.contains(*m));
    }
    assert!(!list.contains(Move::NULL));

    let copy = list.clone();
    assert_eq!(copy.as_slice(), list.as_slice());
    assert_eq!(MoveList::default().len(), 0);
}
