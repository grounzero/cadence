// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `fen`: the parser checked against the placement field, not
//! against its own emitter.
//!
//! `from_fen(to_fen(x)) == x` is a self-consistency check. A parser that
//! consistently mis-assigns the castling rook files round-trips perfectly and
//! fails three steps later as "DFRC perft is wrong". So this gate reads the
//! placement field itself (its own eight-rank expansion, nothing from the
//! crate) and asserts what the parsed layout must agree with:
//!
//! - each `rook_from[i]` holds a rook of the right colour in the placement;
//! - `king_from[c]` is the king's square, strictly between the two rooks;
//! - `rights.has(c, s)` implies `rook_from[ci(c, s)]` is set, the invariant
//!   `Board::can_castle`'s square lookups depend on;
//! - both notations of the same position parse to the same rights, layout
//!   and key;
//! - a right for a rook that is not there is `FenError::Castling` at parse
//!   time, not a panic in move generation.
//!
//! The round trip stays (it catches emission bugs) and lives in
//! `property_fen_roundtrip`.

mod support;

use cadence_core::castling::{CastleSide, ci};
use cadence_core::fen::{FenError, FenStyle};
use cadence_core::position::Board;
use cadence_core::types::{Colour, File, OptSquare, Piece, PieceType, Rank, Square};
use support::generative as generate;

/// The placement field expanded into a mailbox by this test, independently
/// of the crate's parser.
fn placement(fen: &str) -> [Option<Piece>; 64] {
    let field = fen.split_whitespace().next().expect("placement field");
    let mut out = [None; 64];
    let ranks: Vec<&str> = field.split('/').collect();
    assert_eq!(ranks.len(), 8, "{fen}");
    for (i, rank) in ranks.iter().enumerate() {
        let r = 7 - i;
        let mut f = 0usize;
        for ch in rank.chars() {
            if let Some(d) = ch.to_digit(10) {
                f += usize::try_from(d).expect("fits");
            } else {
                out[r * 8 + f] =
                    Some(Piece::from_char(ch).unwrap_or_else(|| panic!("{ch} in {fen}")));
                f += 1;
            }
        }
        assert_eq!(f, 8, "{fen}: rank {rank}");
    }
    out
}

fn squares_of(mailbox: &[Option<Piece>; 64], piece: Piece) -> Vec<Square> {
    Square::all()
        .filter(|sq| mailbox[sq.index()] == Some(piece))
        .collect()
}

/// The X-FEN spelling of a start array's castling field.
fn as_xfen(shredder: &str) -> String {
    let mut fields: Vec<&str> = shredder.split_whitespace().collect();
    fields[2] = "KQkq";
    fields.join(" ")
}

/// Everything this gate asserts about one parsed position whose castling field
/// grants all four rights.
fn assert_layout_matches_placement(label: &str, fen: &str) {
    let board =
        Board::from_fen(fen).unwrap_or_else(|e| panic!("{label}: rejected ({e:?})\n  {fen}"));
    let mailbox = placement(fen);
    let layout = board.layout();
    let rights = board.castling_rights();

    for c in Colour::ALL {
        let king = Piece::new(c, PieceType::King);
        let rook = Piece::new(c, PieceType::Rook);
        let king_sq = squares_of(&mailbox, king);
        assert_eq!(king_sq.len(), 1, "{label}: one {c:?} king");
        let king_sq = king_sq[0];
        assert_eq!(
            layout.king_from[c.index()].get(),
            Some(king_sq),
            "{label}: king_from {c:?}"
        );
        assert_eq!(board.king_square(c), king_sq, "{label}: king_square {c:?}");

        let rooks = squares_of(&mailbox, rook);
        for s in CastleSide::ALL {
            let i = ci(c, s);
            assert!(rights.has(c, s), "{label}: {c:?} {s:?} right");
            let rf = layout.rook_from[i]
                .get()
                .unwrap_or_else(|| panic!("{label}: {c:?} {s:?} rook_from is NONE"));
            assert!(
                mailbox[rf.index()] == Some(rook),
                "{label}: rook_from[{i}] = {rf} does not hold a {c:?} rook in the placement"
            );
            assert!(rooks.contains(&rf));
            assert_eq!(
                rf.rank(),
                king_sq.rank(),
                "{label}: rook on the king's rank"
            );
            match s {
                CastleSide::King => assert!(rf.file() > king_sq.file(), "{label}: kingside rook"),
                CastleSide::Queen => assert!(rf.file() < king_sq.file(), "{label}: queenside rook"),
            }
        }
        // Strictly between.
        let k = layout.rook_from[ci(c, CastleSide::King)]
            .get()
            .expect("checked");
        let q = layout.rook_from[ci(c, CastleSide::Queen)]
            .get()
            .expect("checked");
        assert!(
            q.file() < king_sq.file() && king_sq.file() < k.file(),
            "{label}: {c:?} between"
        );
    }
}

// ---------------------------------------------------------------------------
// All 960 arrays, both notations
// ---------------------------------------------------------------------------

#[test]
fn every_start_array_parses_to_rooks_that_are_actually_there() {
    for (n, fen) in generate::all_960_start_fens().into_iter().enumerate() {
        assert_layout_matches_placement(&format!("array {n} (Shredder)"), &fen);
        assert_layout_matches_placement(&format!("array {n} (X-FEN)"), &as_xfen(&fen));
    }
}

/// `KQkq` and the Shredder file letters describe the same rights for every
/// start array, and the parsed positions are identical: same rights, same
/// layout, same key, same fingerprint.
#[test]
fn both_notations_parse_a_start_array_identically() {
    for (n, fen) in generate::all_960_start_fens().into_iter().enumerate() {
        let shredder = Board::from_fen(&fen).expect("shredder");
        let xfen = Board::from_fen(&as_xfen(&fen)).expect("xfen");
        assert_eq!(
            shredder.castling_rights(),
            xfen.castling_rights(),
            "array {n}"
        );
        assert_eq!(
            shredder.layout().rook_from,
            xfen.layout().rook_from,
            "array {n}"
        );
        assert_eq!(
            shredder.layout().king_from,
            xfen.layout().king_from,
            "array {n}"
        );
        assert_eq!(shredder.key(), xfen.key(), "array {n}: key");
        assert_eq!(
            generate::fingerprint(&shredder),
            generate::fingerprint(&xfen),
            "array {n}: fingerprint"
        );
        // And the emitters produce the strings the two notations require:
        // Shredder is the file letters, X-FEN is KQkq for a start array
        // (no rook stands outside a castling rook).
        assert_eq!(
            shredder.to_fen(FenStyle::Shredder),
            fen,
            "array {n}: Shredder emission"
        );
        assert_eq!(
            shredder.to_fen(FenStyle::XFen),
            as_xfen(&fen),
            "array {n}: X-FEN emission"
        );
    }
}

/// The corpus section 7 rows: the two spellings are the same position, and the
/// fallback row proves X-FEN names the file only where it must.
#[test]
fn corpus_notation_rows_parse_to_the_same_position() {
    for f in support::fen_notations() {
        let a = Board::from_fen(&f.shredder).expect("shredder");
        let b = Board::from_fen(&f.xfen).expect("xfen");
        assert_eq!(a.castling_rights(), b.castling_rights(), "{}", f.reason);
        assert_eq!(a.layout().rook_from, b.layout().rook_from, "{}", f.reason);
        assert_eq!(a.key(), b.key(), "{}", f.reason);
        assert_eq!(
            generate::fingerprint(&a),
            generate::fingerprint(&b),
            "{}",
            f.reason
        );
        assert_layout_matches_placement("notation row", &f.shredder);
        assert_eq!(a.to_fen(FenStyle::Shredder), f.shredder);
        assert_eq!(a.to_fen(FenStyle::XFen), f.xfen);
    }
}

// ---------------------------------------------------------------------------
// The rights ⇒ rook_from invariant, and rejection
// ---------------------------------------------------------------------------

/// `rights.has(c, s)` implies `rook_from[ci(c, s)].is_some()`, over every
/// position the corpus names.
#[test]
fn a_held_right_always_has_a_rook_square() {
    let mut fens: Vec<String> = support::standard_positions()
        .into_iter()
        .map(|p| p.fen)
        .collect();
    fens.extend(support::dfrc_arrays().into_iter().map(|a| a.fen));
    fens.extend(support::castling_cases().into_iter().map(|c| c.fen));
    fens.extend(support::edge_cases().into_iter().map(|c| c.fen));
    fens.extend(support::rights_captures().into_iter().map(|r| r.fen));
    fens.push(support::move_capacity().fen);
    for fen in fens {
        let board = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        let mailbox = placement(&fen);
        for c in Colour::ALL {
            for s in CastleSide::ALL {
                let i = ci(c, s);
                let held = board.castling_rights().has(c, s);
                let rf = board.layout().rook_from[i];
                assert_eq!(held, rf.is_some(), "{fen}: {c:?} {s:?} right vs rook_from");
                if let Some(rf) = rf.get() {
                    assert_eq!(
                        mailbox[rf.index()],
                        Some(Piece::new(c, PieceType::Rook)),
                        "{fen}: rook_from[{i}] = {rf}"
                    );
                    let kf = board.layout().king_from[c.index()]
                        .get()
                        .expect("king_from");
                    assert_eq!(kf, board.king_square(c));
                    assert_eq!(
                        kf.rank(),
                        Rank::One.relative(c),
                        "{fen}: king on its back rank"
                    );
                }
            }
            if !board.castling_rights().any(c) {
                assert_eq!(
                    board.layout().king_from[c.index()],
                    OptSquare::NONE,
                    "{fen}: {c:?}"
                );
            }
        }
    }
}

#[test]
fn a_right_whose_rook_or_king_is_missing_is_a_castling_error() {
    let bad = [
        // Right named, no rook anywhere on the rank.
        "4k3/8/8/8/8/8/8/4K3 w K - 0 1",
        "4k3/8/8/8/8/8/8/4K3 w Q - 0 1",
        "4k3/8/8/8/8/8/8/4K3 w k - 0 1",
        "4k3/8/8/8/8/8/8/4K3 b q - 0 1",
        // File letter naming an empty square.
        "4k3/8/8/8/8/8/8/R3K3 w H - 0 1",
        // File letter naming a rook of the wrong colour.
        "4k3/8/8/8/8/8/8/r3K3 w A - 0 1",
        // The king's own file.
        "4k3/8/8/8/8/8/8/R3K3 w E - 0 1",
        // K with a rook only on the queen's side.
        "4k3/8/8/8/8/8/8/R3K3 w K - 0 1",
        // A right for a king that is not on its back rank.
        "4k3/8/8/8/8/8/4K3/R7 w Q - 0 1",
        "8/4k3/8/8/8/8/8/R3K2R b k - 0 1",
        // A rook on the rank but not on the back rank of that colour.
        "4k3/8/8/8/8/8/R7/4K3 w Q - 0 1",
        // Lowercase file letter for White.
        "4k3/8/8/8/8/8/8/R3K3 w a - 0 1",
        // Not a castling character at all.
        "4k3/8/8/8/8/8/8/R3K3 w X - 0 1",
        "4k3/8/8/8/8/8/8/R3K3 w KQ- - 0 1",
    ];
    for fen in bad {
        assert_eq!(
            Board::from_fen(fen).err(),
            Some(FenError::Castling),
            "{fen}"
        );
    }
    // And the same rooks with the right letters parse.
    assert!(Board::from_fen("4k3/8/8/8/8/8/8/R3K3 w Q - 0 1").is_ok());
    assert!(Board::from_fen("4k3/8/8/8/8/8/8/R3K3 w A - 0 1").is_ok());
    assert!(Board::from_fen("4k3/8/8/8/8/8/8/R3K3 w - - 0 1").is_ok());
}

// ---------------------------------------------------------------------------
// The other fields
// ---------------------------------------------------------------------------

#[test]
fn malformed_fields_are_named_by_the_error() {
    let cases: [(&str, FenError); 12] = [
        ("", FenError::Fields),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR",
            FenError::Fields,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w",
            FenError::Fields,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1 extra",
            FenError::Fields,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP w KQkq - 0 1",
            FenError::Placement,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNRR w KQkq - 0 1",
            FenError::Placement,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNX w KQkq - 0 1",
            FenError::Placement,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR x KQkq - 0 1",
            FenError::SideToMove,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq e4 0 1",
            FenError::EnPassant,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq zz 0 1",
            FenError::EnPassant,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - x 1",
            FenError::Counter,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 0x",
            FenError::Counter,
        ),
    ];
    for (fen, want) in cases {
        assert_eq!(Board::from_fen(fen).err(), Some(want), "{fen}");
    }
    // Kings: none, two, or none of one colour.
    for fen in [
        "8/8/8/8/8/8/8/8 w - - 0 1",
        "4k3/8/8/8/8/8/8/8 w - - 0 1",
        "4k3/8/8/8/8/8/8/4K1K1 w - - 0 1",
        "4k2k/8/8/8/8/8/8/4K3 w - - 0 1",
    ] {
        assert_eq!(Board::from_fen(fen).err(), Some(FenError::Kings), "{fen}");
    }
    // A halfmove clock that does not fit its byte.
    assert_eq!(
        Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 300 1").err(),
        Some(FenError::Counter)
    );
}

#[test]
fn the_other_fields_parse_and_emit_faithfully() {
    let b = Board::from_fen("rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR w KQkq c6 0 2")
        .expect("parses");
    assert_eq!(b.side_to_move(), Colour::White);
    assert_eq!(b.ep_square(), Some(Square::C6));
    assert_eq!(b.halfmove_clock(), 0);
    assert_eq!(b.fullmove_number(), 2);
    assert_eq!(b.piece_at(Square::C5), Some(Piece::BPawn));
    assert_eq!(b.piece_at(Square::E4), Some(Piece::WPawn));
    assert_eq!(b.piece_at(Square::E2), None);
    assert_eq!(b.occupied().count(), 32);
    assert_eq!(b.pieces(Colour::White, PieceType::Pawn).count(), 8);
    assert_eq!(b.by_colour(Colour::Black).count(), 16);
    assert_eq!(b.by_type(PieceType::Knight).count(), 4);
    assert_eq!(b.king_square(Colour::White), Square::E1);
    assert_eq!(b.king_square(Colour::Black), Square::E8);
    assert_eq!(b.ply(), 0);
    assert!(b.game_history().is_empty());
    assert_eq!(
        b.to_fen(FenStyle::XFen),
        "rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR w KQkq c6 0 2"
    );
    assert_eq!(
        b.to_fen(FenStyle::Shredder),
        "rnbqkbnr/pp1ppppp/8/2p5/4P3/8/PPPP1PPP/RNBQKBNR w HAha c6 0 2"
    );

    let b = Board::from_fen("8/8/8/8/8/8/8/K6k b - - 99 150").expect("parses");
    assert_eq!(b.side_to_move(), Colour::Black);
    assert_eq!(b.ep_square(), None);
    assert_eq!(b.halfmove_clock(), 99);
    assert_eq!(b.fullmove_number(), 150);
    assert!(b.castling_rights().is_empty());
    assert_eq!(b.to_fen(FenStyle::XFen), "8/8/8/8/8/8/8/K6k b - - 99 150");

    // Four and five fields are accepted, counters defaulting to 0 and 1.
    let b = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - -").expect("four fields");
    assert_eq!((b.halfmove_clock(), b.fullmove_number()), (0, 1));
    let b = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 7").expect("five fields");
    assert_eq!((b.halfmove_clock(), b.fullmove_number()), (7, 1));
    assert_eq!(b.to_fen(FenStyle::XFen), "4k3/8/8/8/8/8/8/4K3 w - - 7 1");

    // The ep square must be on rank 3 or 6, and is otherwise kept as given.
    for (fen, want) in [
        ("4k3/8/8/8/4P3/8/8/4K3 b - e3 0 1", Some(Square::E3)),
        ("4k3/8/8/4p3/8/8/8/4K3 w - e6 0 1", Some(Square::E6)),
        ("4k3/8/8/8/8/8/8/4K3 w - - 0 1", None),
    ] {
        assert_eq!(Board::from_fen(fen).expect(fen).ep_square(), want, "{fen}");
    }
    let sq = Square::from_file_rank(File::E, Rank::Three);
    assert_eq!(sq, Square::E3);
}
