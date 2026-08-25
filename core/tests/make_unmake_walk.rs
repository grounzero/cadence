// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `position`: make/unmake and the incremental key, on a walk
//! that needs no move generator.
//!
//! The moves come from `support::naive::legal` (the obvious pseudo-legal
//! generator plus a make/unmake king-safety filter), so this runs before
//! `generate_legal` exists. `property_make_unmake` walks the same properties
//! over the real generator once there is one; this is the version that can
//! fail first, when the only thing that has been written is the position.
//!
//! At every node of every walk:
//!
//! - **`key() == recompute_key()`**, and `pawn_key()` likewise. Under
//!   copy-make "unmake restores the key" is nearly tautological, so this is
//!   the assertion with teeth: the key can only change where the board
//!   changes, so a disagreement is the board moving where it should not.
//! - **The key is a function of the position, not the path**: reparsing the
//!   position's own FEN gives the same key. That is what the ep-key rule
//!   is for, and it is checked directly as well: the ep key is mixed in only
//!   when an enemy pawn can actually take.
//! - **make then unmake restores the fingerprint** (mailbox, twelve piece
//!   bitboards, occupancy, key, both FENs) for every legal move, not just
//!   the one the walk plays.
//! - **`DirtyPieces` replays the mailbox**: every subtraction finds its piece,
//!   every addition finds an empty square (the mailbox form of "no value
//!   leaves {0, 1}"), all subtractions before any addition, and the result
//!   is the post-move mailbox. At most three entries; never zero.
//! - **`gives_check(m)`** agrees with making the move and reading `checkers`.
//! - **`can_castle`** agrees with the naive predicate written from the
//!   corpus's rules, with both pieces lifted.
//! - **Null move** flips the side, clears ep, keys correctly, and unmakes.
//! - The clocks and the castling rights change by the rules.

mod support;

use cadence_core::castling::{CastleSide, ci};
use cadence_core::position::Board;
use cadence_core::types::{Colour, Piece, PieceType, Square};
use cadence_core::{FenStyle, Move};
use support::generative as generate;
use support::naive;

const WALKS: usize = 120;
const PLIES_PER_WALK: usize = 60;

fn mailbox(board: &Board) -> [Option<Piece>; 64] {
    let mut out = [None; 64];
    for sq in Square::all() {
        out[sq.index()] = board.piece_at(sq);
    }
    out
}

/// Every per-node assertion, given the legal moves at this node.
#[expect(
    clippy::too_many_lines,
    reason = "one node, every property, in reading order"
)]
fn assert_node(label: &str, board: &mut Board, legal: &[Move]) {
    let fen = board.to_fen(FenStyle::Shredder);

    // The key, and the key as a function of the position.
    assert_eq!(
        board.key(),
        board.recompute_key(),
        "{label}: key vs recomputation\n  {fen}"
    );
    assert_eq!(
        board.pawn_key(),
        board.recompute_pawn_key(),
        "{label}: pawn key vs recomputation\n  {fen}"
    );
    let reparsed = Board::from_fen(&fen)
        .unwrap_or_else(|e| panic!("{label}: own FEN rejected ({e:?})\n  {fen}"));
    assert_eq!(
        reparsed.key(),
        board.key(),
        "{label}: key depends on the path\n  {fen}"
    );
    assert_eq!(
        reparsed.pawn_key(),
        board.pawn_key(),
        "{label}: pawn key depends on the path"
    );
    assert_eq!(
        reparsed.castling_rights(),
        board.castling_rights(),
        "{label}: rights\n  {fen}"
    );
    assert_eq!(
        reparsed.checkers(),
        board.checkers(),
        "{label}: checkers vs reparse\n  {fen}"
    );
    for c in Colour::ALL {
        assert_eq!(
            reparsed.blockers(c),
            board.blockers(c),
            "{label}: blockers vs reparse"
        );
        assert_eq!(
            reparsed.pinners(c),
            board.pinners(c),
            "{label}: pinners vs reparse"
        );
    }

    // The ep rule, stated directly at this node.
    let us = board.side_to_move();
    let ep_capturable = board.ep_square().is_some_and(|ep| {
        (naive::attackers_to(board, ep, board.occupied()) & board.pieces(us, PieceType::Pawn)).any()
    });
    let mut fields: Vec<&str> = fen.split_whitespace().collect();
    fields[3] = "-";
    let without_ep = Board::from_fen(&fields.join(" ")).expect("ep dropped");
    if ep_capturable {
        assert_ne!(
            without_ep.key(),
            board.key(),
            "{label}: an available ep capture must hash\n  {fen}"
        );
    } else {
        assert_eq!(
            without_ep.key(),
            board.key(),
            "{label}: an unavailable ep square must not hash\n  {fen}"
        );
    }

    // Castling, both sides, against the naive predicate.
    for s in CastleSide::ALL {
        assert_eq!(
            board.can_castle(us, s),
            naive::can_castle(board, us, s),
            "{label}: can_castle({us:?}, {s:?})\n  {fen}"
        );
    }

    // Every legal move: unmake restores, dirty replays, gives_check agrees,
    // the clocks and rights follow the rules.
    let before = generate::fingerprint(board);
    let before_mailbox = mailbox(board);
    let before_half = board.halfmove_clock();
    let before_full = board.fullmove_number();
    let before_rights = board.castling_rights();
    let before_ply = board.ply();
    let layout = *board.layout();
    for &m in legal {
        let ctx = format!("{label} {m:?}\n  {fen}");
        let gives_check = board.gives_check(m);
        let mover = board.piece_at(m.from_sq()).expect("a piece moves");
        let victim = if m.is_en_passant() {
            Some(Piece::new(us.flip(), PieceType::Pawn))
        } else if m.is_castle() {
            None
        } else {
            board.piece_at(m.to_sq())
        };

        let dirty = board.make_move(m);

        assert_eq!(board.side_to_move(), us.flip(), "{ctx}: side to move");
        assert_eq!(board.ply(), before_ply + 1, "{ctx}: ply");
        assert_eq!(board.in_check(), gives_check, "{ctx}: gives_check");
        assert_eq!(board.key(), board.recompute_key(), "{ctx}: key after make");
        assert_eq!(board.state().captured, victim, "{ctx}: captured");

        // Clocks.
        let irreversible = mover.piece_type() == PieceType::Pawn || m.is_capture();
        assert_eq!(
            board.halfmove_clock(),
            if irreversible { 0 } else { before_half + 1 },
            "{ctx}: halfmove clock"
        );
        assert_eq!(
            board.fullmove_number(),
            if us == Colour::Black {
                before_full + 1
            } else {
                before_full
            },
            "{ctx}: fullmove number"
        );

        // Rights: a right survives iff it was held and the move touched
        // neither the king's nor that rook's origin square.
        for c in Colour::ALL {
            for s in CastleSide::ALL {
                let kf = layout.king_from[c.index()].get();
                let rf = layout.rook_from[ci(c, s)].get();
                let touched = |sq: Option<Square>| sq == Some(m.from_sq()) || sq == Some(m.to_sq());
                let want = before_rights.has(c, s) && !touched(kf) && !touched(rf);
                assert_eq!(
                    board.castling_rights().has(c, s),
                    want,
                    "{ctx}: right {c:?} {s:?}"
                );
            }
        }

        // The ep square is set after a double push and only then.
        if m.is_double_push() {
            let ep = Square::new(
                u8::try_from(usize::midpoint(m.from_sq().index(), m.to_sq().index()))
                    .expect("fits"),
            );
            assert_eq!(board.ep_square(), Some(ep), "{ctx}: ep square");
        } else {
            assert_eq!(board.ep_square(), None, "{ctx}: ep square cleared");
        }

        // DirtyPieces replays the mailbox: all subs, then all adds.
        let after_mailbox = mailbox(board);
        let entries = dirty.as_slice();
        assert!(!entries.is_empty(), "{ctx}: empty delta for a real move");
        assert!(
            entries.len() <= cadence_core::MAX_DIRTY_REACHABLE,
            "{ctx}: {} entries",
            entries.len()
        );
        assert_eq!(entries.len(), dirty.len());
        let mut v = before_mailbox;
        for e in entries {
            if let Some(from) = e.from.get() {
                assert_eq!(
                    v[from.index()],
                    Some(e.piece),
                    "{ctx}: subtracting {:?} from {from}",
                    e.piece
                );
                v[from.index()] = None;
            }
        }
        for e in entries {
            if let Some(to) = e.to.get() {
                assert_eq!(
                    v[to.index()],
                    None,
                    "{ctx}: adding {:?} onto occupied {to}",
                    e.piece
                );
                v[to.index()] = Some(e.piece);
            }
        }
        assert!(
            v == after_mailbox,
            "{ctx}: delta does not reach the post-move mailbox"
        );
        // Shape per move type.
        let expected_len = if m.is_castle() {
            let i = ci(us, m.castle_side());
            usize::from(layout.king_from[us.index()] != layout.king_to[i])
                + usize::from(layout.rook_from[i] != layout.rook_to[i])
        } else if m.is_promotion() {
            2 + usize::from(m.is_capture())
        } else {
            1 + usize::from(m.is_capture())
        };
        assert_eq!(entries.len(), expected_len, "{ctx}: delta length");

        board.unmake_move(m);
        assert_eq!(
            generate::fingerprint(board),
            before,
            "{ctx}: unmake did not restore"
        );
        assert_eq!(board.ply(), before_ply, "{ctx}: ply after unmake");
        assert_eq!(
            board.halfmove_clock(),
            before_half,
            "{ctx}: halfmove after unmake"
        );
        assert_eq!(
            board.fullmove_number(),
            before_full,
            "{ctx}: fullmove after unmake"
        );
    }

    // The null move.
    let dirty = board.make_null_move();
    assert!(dirty.is_empty(), "{label}: null move delta");
    assert_eq!(
        board.side_to_move(),
        us.flip(),
        "{label}: null flips the side"
    );
    assert_eq!(board.ep_square(), None, "{label}: null clears ep");
    assert_eq!(board.key(), board.recompute_key(), "{label}: null key");
    assert_eq!(board.ply(), before_ply + 1);
    board.unmake_null_move();
    assert_eq!(
        generate::fingerprint(board),
        before,
        "{label}: unmake null did not restore"
    );
    assert_eq!(board.ply(), before_ply);
}

/// The walk's move choice: uniform, except that when a castle, an en passant
/// or a promotion is available it is taken half the time. Those are the
/// branches of `make_move` a uniform walk from a start array reaches rarely
/// or never, and the point of the walk is to reach them.
fn pick(legal: &[Move], rng: &mut generate::Rng) -> Move {
    let rare: Vec<Move> = legal
        .iter()
        .copied()
        .filter(|m| m.is_castle() || m.is_en_passant() || m.is_promotion())
        .collect();
    if !rare.is_empty() && rng.below(2) == 0 {
        rare[rng.below(rare.len())]
    } else {
        legal[rng.below(legal.len())]
    }
}

#[test]
fn make_unmake_and_the_key_hold_at_every_node_of_the_walks() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(0x5EED_5EED_0000_0005);
    let mut nodes = 0usize;
    let mut kinds = [0usize; 5]; // castle, ep, promotion, capture, double push

    for walk in 0..WALKS {
        let fen = &seeds[walk % seeds.len()];
        let mut board =
            Board::from_fen(fen).unwrap_or_else(|e| panic!("walk {walk}: {e:?}\n  {fen}"));
        assert_eq!(board.ply(), 0);
        for ply in 0..PLIES_PER_WALK {
            let legal = naive::legal(&mut board);
            assert_node(&format!("walk {walk} ply {ply}"), &mut board, &legal);
            nodes += 1;
            if legal.is_empty() {
                break;
            }
            let m = pick(&legal, &mut rng);
            if m.is_castle() {
                kinds[0] += 1;
            }
            if m.is_en_passant() {
                kinds[1] += 1;
            }
            if m.is_promotion() {
                kinds[2] += 1;
            }
            if m.is_capture() {
                kinds[3] += 1;
            }
            if m.is_double_push() {
                kinds[4] += 1;
            }
            board.make_move(m);
        }
    }
    eprintln!(
        "{nodes} nodes; castles {} ep {} promotions {} captures {} double pushes {}",
        kinds[0], kinds[1], kinds[2], kinds[3], kinds[4]
    );
    assert!(nodes >= WALKS * 20, "walks ended early: {nodes} nodes");
    // The walks must have played every kind of move, or a make_move branch
    // went untested.
    assert!(kinds[0] > 0, "no castling played");
    assert!(kinds[1] > 0, "no en passant played");
    assert!(kinds[2] > 0, "no promotion played");
    assert!(kinds[3] > 0, "no capture played");
    assert!(kinds[4] > 0, "no double push played");
}

/// The ep rule by name: after e2e4 the ep square is set either way, but the
/// key differs from the no-ep position only when a black pawn can take.
#[test]
fn en_passant_key_is_mixed_in_only_when_a_capture_is_available() {
    let e2e4 = Move::new_double_push(Square::E2, Square::E4);

    let mut quiet = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1").expect("fen");
    quiet.make_move(e2e4);
    assert_eq!(
        quiet.ep_square(),
        Some(Square::E3),
        "the ep square is set after every double push"
    );
    let no_ep = Board::from_fen("4k3/8/8/8/4P3/8/8/4K3 b - - 0 1").expect("fen");
    let with_ep = Board::from_fen("4k3/8/8/8/4P3/8/8/4K3 b - e3 0 1").expect("fen");
    assert_eq!(
        quiet.key(),
        no_ep.key(),
        "no black pawn can take: the ep key is not mixed in"
    );
    assert_eq!(
        with_ep.key(),
        no_ep.key(),
        "nor when the FEN names an ep square nobody can use"
    );
    assert_eq!(quiet.key(), quiet.recompute_key());

    let mut capturable = Board::from_fen("4k3/8/8/8/3p4/8/4P3/4K3 w - - 0 1").expect("fen");
    capturable.make_move(e2e4);
    let no_ep = Board::from_fen("4k3/8/8/8/3pP3/8/8/4K3 b - - 0 1").expect("fen");
    let with_ep = Board::from_fen("4k3/8/8/8/3pP3/8/8/4K3 b - e3 0 1").expect("fen");
    assert_ne!(
        capturable.key(),
        no_ep.key(),
        "d4xe3 is available: the ep key must be mixed in"
    );
    assert_eq!(capturable.key(), with_ep.key());
    assert_eq!(capturable.key(), capturable.recompute_key());

    // Unmake restores the pre-push key in both cases.
    quiet.unmake_move(e2e4);
    capturable.unmake_move(e2e4);
    assert_eq!(
        quiet.key(),
        Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1")
            .expect("fen")
            .key()
    );
    assert_eq!(
        capturable.key(),
        Board::from_fen("4k3/8/8/8/3p4/8/4P3/4K3 w - - 0 1")
            .expect("fen")
            .key()
    );
    // The two mirrored for Black.
    let d7d5 = Move::new_double_push(Square::D7, Square::D5);
    let mut b = Board::from_fen("4k3/3p4/8/4P3/8/8/8/4K3 b - - 0 1").expect("fen");
    b.make_move(d7d5);
    assert_eq!(b.ep_square(), Some(Square::D6));
    assert_ne!(
        b.key(),
        Board::from_fen("4k3/8/8/3pP3/8/8/8/4K3 w - - 0 2")
            .expect("fen")
            .key()
    );
    assert_eq!(
        b.key(),
        Board::from_fen("4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 2")
            .expect("fen")
            .key()
    );
}

/// The degenerate castles by hand: king stays, rook stays, swap, rook onto
/// the king's origin. Each must leave the right pieces on the right squares
/// and unmake cleanly: the ordering rule "clear both origins before setting
/// either destination" is what these exercise.
#[test]
fn degenerate_castles_make_and_unmake_by_hand() {
    let cases = [
        // fen, castle move, king_to, rook_to
        (
            "4k3/8/8/8/8/8/8/6KR w H - 0 1",
            "g1h1",
            Square::G1,
            Square::F1,
        ),
        (
            "4k3/8/8/8/8/8/8/4KR2 w F - 0 1",
            "e1f1",
            Square::G1,
            Square::F1,
        ),
        (
            "4k3/8/8/8/8/8/8/5KR1 w G - 0 1",
            "f1g1",
            Square::G1,
            Square::F1,
        ),
        (
            "4k3/8/8/8/8/8/8/R1K5 w A - 0 1",
            "c1a1",
            Square::C1,
            Square::D1,
        ),
        (
            "4k3/8/8/8/8/8/8/R2K4 w A - 0 1",
            "d1a1",
            Square::C1,
            Square::D1,
        ),
        (
            "6k1/8/8/8/8/8/8/1KR5 w C - 0 1",
            "b1c1",
            Square::G1,
            Square::F1,
        ),
        (
            "6kr/8/8/8/8/8/8/4K3 b h - 0 1",
            "g8h8",
            Square::G8,
            Square::F8,
        ),
        (
            "5kr1/8/8/8/8/8/8/4K3 b g - 0 1",
            "f8g8",
            Square::G8,
            Square::F8,
        ),
        (
            "r2k4/8/8/8/8/8/8/4K3 b a - 0 1",
            "d8a8",
            Square::C8,
            Square::D8,
        ),
    ];
    for (fen, uci, kt, rt) in cases {
        let mut board = Board::from_fen(fen).expect(fen);
        let us = board.side_to_move();
        let kf = Square::from_algebraic(&uci[..2]).expect("kf");
        let rf = Square::from_algebraic(&uci[2..]).expect("rf");
        let m = Move::new_castle(kf, rf);
        assert!(
            board.can_castle(us, m.castle_side()),
            "{fen}: {uci} should be available"
        );
        let before = generate::fingerprint(&board);
        let dirty = board.make_move(m);
        assert_eq!(
            board.piece_at(kt),
            Some(Piece::new(us, PieceType::King)),
            "{fen}: king on {kt}"
        );
        assert_eq!(
            board.piece_at(rt),
            Some(Piece::new(us, PieceType::Rook)),
            "{fen}: rook on {rt}"
        );
        for sq in [kf, rf] {
            if sq != kt && sq != rt {
                assert_eq!(board.piece_at(sq), None, "{fen}: {sq} vacated");
            }
        }
        assert_eq!(board.pieces(us, PieceType::King).count(), 1, "{fen}");
        assert_eq!(board.pieces(us, PieceType::Rook).count(), 1, "{fen}");
        assert_eq!(board.occupied().count(), 3, "{fen}");
        assert!(!board.castling_rights().any(us), "{fen}: rights gone");
        assert_eq!(board.key(), board.recompute_key(), "{fen}: key");
        let expected_len = usize::from(kf != kt) + usize::from(rf != rt);
        assert_eq!(dirty.len(), expected_len, "{fen}: {:?}", dirty.as_slice());
        board.unmake_move(m);
        assert_eq!(generate::fingerprint(&board), before, "{fen}: unmake");
    }
}
