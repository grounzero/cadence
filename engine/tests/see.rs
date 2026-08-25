// SPDX-License-Identifier: GPL-3.0-or-later

//! The static exchange evaluation, held to a second implementation that
//! plays the exchange out on the board.
//!
//! **Why this one gets an oracle.** A wrong exchange evaluation is
//! invisible. It does not crash, it does not change a perft count, and it
//! does not fail a search gate: it quietly calls a winning capture losing,
//! the search skips the capture, the engine plays a little worse, and every
//! other test in this repository keeps passing. The only way to see it is
//! to compute the same number a second way and compare. So the oracle here
//! is not a list of expected values; it is a second implementation that
//! shares nothing with the first but the value table and the definition.
//! `see::see` keeps an attacker set and an occupancy and reveals x-rays by
//! re-querying the sliders. The oracle makes the moves. It asks the legal
//! generator what can recapture, plays the cheapest answer, asks again, and
//! takes the move back, so pins, discovered checks and the king's safety
//! are whatever `core` says they are and not whatever this file thinks
//! they are. The two have to agree to the integer, on every legal move of
//! every position in a corpus built to reach the cases that matter: x-rays
//! through the capturer and through an en-passant victim, recapturers that
//! are pinned at the root, recapturers that become pinned during the
//! exchange, checks uncovered by a capture, promotions on the first move
//! and on a recapture, and kings that may or may not take.
//!
//! **What the constructed positions add.** The corpus test says the two
//! agree; it does not say either is right. The positions below each pin
//! one mechanism to a number worked out by hand, in a pair: the position
//! with the mechanism and the same position without it, so that the test
//! fails if the mechanism is not modelled and fails differently if it is
//! modelled wrongly. Every one is checked for what it claims before the
//! function is asked, and every one is asked in both colours.
//!
//! **The tie-break is part of the definition.** "Least valuable" is by
//! value, then by the order of `PieceType`, then by square, and the oracle
//! picks the same way. Two pieces of one value can uncover different
//! x-rays, so an oracle that picked differently would disagree for a
//! reason that is neither implementation's fault. The king is worth zero
//! and so goes first, and the corpus is what settled that: the function's
//! first draft tried it last, as the usual convention does, and the oracle
//! found a position where the king's capture wins a queen that a queen's
//! capture gives straight back, because the king's move uncovers nothing
//! and the queen's uncovers a bishop. Whether the least
//! valuable piece is the *best* piece to recapture with is a separate
//! question, and the corpus test measures it rather than asserting it: the
//! oracle can also play every recapture and take the best, and the count
//! of exchanges where that differs from the cheapest-first answer is
//! printed, not gated.

mod support;

use cadence_core::fen::FenStyle;
use cadence_core::position::Board;
use cadence_core::types::{File, Rank};
use cadence_core::{
    Bitboard, Colour, Move, Piece, PieceType, START_FEN, Square, generate_legal, parse_uci,
};
use cadence_engine::see::{VALUES, see, value};
use support::{Rng, mirror_fen};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

fn fen(b: &Board) -> String {
    b.to_fen(FenStyle::Shredder)
}

fn mv(b: &Board, uci: &str) -> Move {
    let legal = generate_legal(b);
    parse_uci(&legal, uci).unwrap_or_else(|| panic!("{uci} is not legal in {}", fen(b)))
}

fn sq(s: &str) -> Square {
    Square::from_algebraic(s).unwrap_or_else(|| panic!("not a square: {s}"))
}

/// A UCI move under the colour mirror: the ranks flip, the files stay.
fn mirror_uci(uci: &str) -> String {
    uci.chars()
        .map(|c| match c {
            '1'..='8' => char::from(b'9' - (c as u8 - b'0')),
            other => other,
        })
        .collect()
}

fn piece_type_at(b: &Board, s: Square) -> PieceType {
    b.piece_at(s).map_or_else(
        || panic!("no piece on {s} in {}", fen(b)),
        Piece::piece_type,
    )
}

// ---------------------------------------------------------------------------
// The oracle
// ---------------------------------------------------------------------------

/// What the corpus reached, counted by the oracle from what the generator
/// handed it and not from anything the function under test reports.
#[derive(Debug, Default)]
struct Census {
    positions: usize,
    in_check_at_root: usize,
    moves: usize,
    captures: usize,
    castles: usize,
    en_passant: usize,
    /// The first move promotes.
    promotes_first: usize,
    /// A recapture promotes.
    promotes_later: usize,
    /// A recapturer that did not attack the square before the first move:
    /// an x-ray, revealed by a piece that has since left.
    x_rays: usize,
    /// A piece that attacks the square and may not capture on it while its
    /// side is not in check from anywhere else: a pin.
    pinned: usize,
    /// The same, with its side in check from a piece not on the square: a
    /// discovered check barred it.
    barred_by_check: usize,
    /// A king that attacks the square and may not go there.
    king_refused: usize,
    king_recaptures: usize,
    /// A side that could recapture and chose not to.
    declined: usize,
    /// The longest exchange, in captures including the first.
    longest: usize,
}

/// Material `m` takes off the board, plus what a promotion puts on it.
fn gained(b: &Board, m: Move) -> i32 {
    let victim = if m.is_en_passant() {
        value(PieceType::Pawn)
    } else if m.is_capture() {
        value(piece_type_at(b, m.to_sq()))
    } else {
        0
    };
    let bonus = m
        .promotion_piece()
        .map_or(0, |p| value(p.piece_type()) - value(PieceType::Pawn));
    victim + bonus
}

/// The side to move's best result on `to`: zero if it stops, or what the
/// cheapest legal recapture gains less what the other side then gets. With
/// `best`, every legal recapture is tried rather than the cheapest.
fn play_out(
    b: &mut Board,
    to: Square,
    root_attackers: Bitboard,
    best: bool,
    depth: usize,
    census: &mut Census,
) -> i32 {
    let stm = b.side_to_move();
    let legal = generate_legal(b);
    let candidates: Vec<Move> = legal
        .iter()
        .filter(|m| m.to_sq() == to && m.is_capture() && !m.is_en_passant())
        .collect();

    // Who attacks the square and cannot take on it, and why.
    let pseudo = b.attackers_to(to, b.occupied()) & b.by_colour(stm);
    let checked_from_elsewhere = (b.checkers() & !to.bb()).any();
    for s in pseudo {
        if candidates.iter().any(|m| m.from_sq() == s) {
            continue;
        }
        if piece_type_at(b, s) == PieceType::King {
            census.king_refused += 1;
        } else if checked_from_elsewhere {
            census.barred_by_check += 1;
        } else {
            census.pinned += 1;
        }
    }
    if candidates.is_empty() {
        census.longest = census.longest.max(depth);
        return 0;
    }

    // Least valuable first: value, then piece-type order, then square. A
    // pawn that promotes is four moves with one key, and the best of the
    // four is taken.
    let key = |m: &Move| {
        let pt = piece_type_at(b, m.from_sq());
        (value(pt), pt.index(), m.from_sq().index())
    };
    let cheapest = candidates.iter().map(key).min().expect("a candidate");
    let tried: Vec<Move> = candidates
        .into_iter()
        .filter(|m| best || key(m) == cheapest)
        .collect();

    let mut result = i32::MIN;
    for m in tried {
        if !best {
            if !root_attackers.contains(m.from_sq()) {
                census.x_rays += 1;
            }
            if piece_type_at(b, m.from_sq()) == PieceType::King {
                census.king_recaptures += 1;
            }
            if m.is_promotion() {
                census.promotes_later += 1;
            }
        }
        let g = gained(b, m);
        b.make_move(m);
        let reply = play_out(b, to, root_attackers, best, depth + 1, census);
        b.unmake_move(m);
        result = result.max(g - reply);
    }
    if !best && result < 0 {
        census.declined += 1;
    }
    result.max(0)
}

/// The exchange value of `m`, played out. `b` comes back as it went in.
fn oracle(b: &mut Board, m: Move, best: bool, census: &mut Census) -> i32 {
    if !best {
        census.moves += 1;
        if m.is_capture() {
            census.captures += 1;
        }
        if m.is_en_passant() {
            census.en_passant += 1;
        }
        if m.is_promotion() {
            census.promotes_first += 1;
        }
    }
    if m.is_castle() {
        if !best {
            census.castles += 1;
        }
        return 0;
    }
    let to = m.to_sq();
    let root_attackers = b.attackers_to(to, b.occupied());
    let g = gained(b, m);
    b.make_move(m);
    let reply = play_out(b, to, root_attackers, best, 1, census);
    b.unmake_move(m);
    g - reply
}

/// The function and the oracle on one move, both required to give
/// `expected`. The failure message names which of the two disagreed with
/// the hand-worked number, because the two failures mean different things.
fn check(b: &Board, uci: &str, expected: i32) {
    let m = mv(b, uci);
    let mut census = Census::default();
    let played = oracle(&mut b.duplicate(), m, false, &mut census);
    assert_eq!(
        played,
        expected,
        "{} {uci}: the play-out says {played}, the hand says {expected}; the position does not claim what it thinks",
        fen(b)
    );
    let got = see(b, m);
    assert_eq!(
        got,
        expected,
        "{} {uci}: see says {got}, the play-out and the hand say {expected}",
        fen(b)
    );
}

/// `check`, in both colours.
fn check_both(fen_w: &str, uci: &str, expected: i32) {
    check(&board(fen_w), uci, expected);
    check(&board(&mirror_fen(fen_w)), &mirror_uci(uci), expected);
}

// ---------------------------------------------------------------------------
// The values
// ---------------------------------------------------------------------------

#[test]
fn the_values_order_the_pieces_and_make_a_minor_for_a_minor_level() {
    use PieceType::{Bishop, King, Knight, Pawn, Queen, Rook};
    assert!(value(Pawn) < value(Knight));
    assert_eq!(value(Knight), value(Bishop), "a minor for a minor is level");
    assert!(value(Bishop) < value(Rook));
    assert!(value(Rook) < value(Queen));
    assert_eq!(
        value(King),
        0,
        "the first to recapture, never read into a result"
    );
    for pt in PieceType::ALL {
        assert_eq!(value(pt), VALUES[pt.index()]);
    }
    // Two minors are worth more than a rook and less than a rook and a
    // pawn, a rook and a pawn less than two minors plus nothing: the usual
    // relations, so that an exchange's sign matches what a player would say.
    assert!(2 * value(Knight) > value(Rook));
    assert_eq!(value(Rook) + value(Pawn), 2 * value(Knight));
    assert!(value(Queen) > value(Rook) + value(Bishop));
}

// ---------------------------------------------------------------------------
// Constructed positions, one mechanism each, with and without
// ---------------------------------------------------------------------------

/// A rook takes a pawn defended by a rook, with a second rook behind the
/// first on the file. The x-ray is what turns a loss into a gain.
#[test]
fn an_x_ray_behind_the_capturer_is_counted() {
    let with = "3rk3/8/8/3p4/8/3R4/8/3RK3 w - - 0 1";
    let without = "3rk3/8/8/3p4/8/3R4/8/4K3 w - - 0 1";
    let b = board(with);
    let (d1, d3, d5) = (sq("d1"), sq("d3"), sq("d5"));
    assert!(
        !b.attackers_to(d5, b.occupied()).contains(d1),
        "the rook on d1 must not attack d5 while d3 is occupied"
    );
    assert!(
        b.attackers_to(d5, b.occupied().without(d3)).contains(d1),
        "and must once d3 is lifted"
    );
    // Rxd5 100, Rxd5 500, Rxd5 500: the second white rook wins the pawn.
    check_both(with, "d3d5", 100);
    // Without it, Rxd5 Rxd5 and the pawn cost a rook.
    check_both(without, "d3d5", -400);
}

/// A knight takes a pawn whose only defender, a knight, is pinned to its
/// king by a bishop. Put a pawn on the pin line and the defender is free.
#[test]
fn a_pinned_defender_does_not_recapture_and_a_blocker_on_the_pin_line_frees_it() {
    let pinned = "4k3/8/2n5/8/B2p4/5N2/8/6K1 w - - 0 1";
    let freed = "4k3/3p4/2n5/8/B2p4/5N2/8/6K1 w - - 0 1";
    let c6 = sq("c6");
    assert!(
        board(pinned).blockers(Colour::Black).contains(c6),
        "the knight on c6 must be pinned"
    );
    assert!(
        !board(freed).blockers(Colour::Black).contains(c6),
        "and not once d7 is occupied"
    );
    check_both(pinned, "f3d4", 100);
    check_both(freed, "f3d4", -200);
}

/// A pinned bishop may still capture along its pin. The move has to be
/// quiet: a piece pinned at the root is the only piece on its line, so the
/// square it captures on, being on that line, was empty. The queen steps
/// onto the diagonal, the bishop takes it toward its pinner, and the
/// pinner takes the bishop. A rule that refused every pinned piece would
/// call the step free; it loses a queen for a bishop.
#[test]
fn a_pinned_piece_captures_along_its_pin_line() {
    let fen_w = "7k/8/8/4b3/8/8/1B6/K2Q4 w - - 0 1";
    let b = board(fen_w);
    let (d4, e5, h8) = (sq("d4"), sq("e5"), sq("h8"));
    assert!(
        b.blockers(Colour::Black).contains(e5),
        "the bishop on e5 must be pinned"
    );
    assert!(
        cadence_core::attacks::aligned(h8, e5, d4),
        "and d4 must lie on its pin line"
    );
    assert!(b.piece_at(d4).is_none(), "and d4 must be empty");
    // Qd4 0, Bxd4 900, Bxd4 300.
    check_both(fen_w, "d1d4", -600);
}

/// A rook takes a rook defended by a knight, and a pawn stands ready to
/// recapture on the eighth rank. Without the promotion the knight's
/// recapture is worth taking; with it the knight stays put.
#[test]
fn a_promotion_on_a_recapture_is_worth_the_promoted_piece() {
    let fen_w = "k6r/5nP1/8/8/8/8/8/K6R w - - 0 1";
    let b = board(fen_w);
    let h8 = sq("h8");
    assert_eq!(h8.rank(), Rank::Eight);
    assert!(
        b.attackers_to(h8, b.occupied()).contains(sq("g7")),
        "the pawn on g7 must attack h8"
    );
    // Rxh8 500; Nxh8 500 would be answered by gxh8=Q 300 + 800, so the
    // knight declines. Without the promotion, 500 - 300 = 200 is worth
    // taking and the exchange would be 300.
    check_both(fen_w, "h1h8", 500);
    // The first move promoting: gxh8=Q 500 + 800, Nxh8 900, Rxh8 300.
    check_both(fen_w, "g7h8q", 700);
}

/// An en-passant capture opens the file behind the captured pawn, not only
/// the one behind the capturer: the rook on d1 sees d6 through d5.
#[test]
fn en_passant_opens_the_file_behind_the_captured_pawn() {
    let with = "3r3k/8/8/3pP3/8/8/8/3R3K w - d6 0 1";
    let without = "3r3k/8/8/3pP3/8/8/8/7K w - d6 0 1";
    let b = board(with);
    let (d1, d5, d6) = (sq("d1"), sq("d5"), sq("d6"));
    assert_eq!(b.ep_square(), Some(d6));
    assert!(
        !b.attackers_to(d6, b.occupied()).contains(d1),
        "the rook on d1 must not attack d6 while the black pawn stands on d5"
    );
    assert!(
        b.attackers_to(d6, b.occupied().without(d5)).contains(d1),
        "and must once the captured pawn is lifted"
    );
    let m = mv(&b, "e5d6");
    assert!(m.is_en_passant());
    // exd6 100, Rxd6 100, Rxd6 500.
    check_both(with, "e5d6", 100);
    // Without the rook behind, exd6 Rxd6: a pawn each.
    check_both(without, "e5d6", 0);
}

/// A pawn takes a knight next to the king, and the king may not recapture
/// because the pawn's departure has opened a bishop's diagonal onto the
/// square. The square was not attacked when the king looked at it; it is
/// once the capture has been made.
#[test]
fn a_king_does_not_recapture_onto_a_square_the_capturer_uncovered() {
    let with = "4k3/3n4/2P5/8/B7/8/8/7K w - - 0 1";
    let without = "4k3/3n4/2P5/8/8/8/8/7K w - - 0 1";
    let b = board(with);
    let (a4, c6, d7) = (sq("a4"), sq("c6"), sq("d7"));
    assert!(
        !b.attackers_to(d7, b.occupied()).contains(a4),
        "the bishop must not attack d7 while the pawn stands on c6"
    );
    assert!(
        b.attackers_to(d7, b.occupied().without(c6)).contains(a4),
        "and must once the pawn has left"
    );
    check_both(with, "c6d7", 300);
    check_both(without, "c6d7", 200);
}

/// A knight takes a pawn and, by leaving the e-file, uncovers a rook's
/// check on the king. The pawn that defends the square may not recapture:
/// its capture would not answer the check.
#[test]
fn a_discovered_check_bars_every_recapture_but_the_kings() {
    let with = "4k3/2p5/3p4/8/4N3/8/8/4R2K w - - 0 1";
    let without = "4k3/2p5/3p4/8/4N3/8/8/7K w - - 0 1";
    let mut b = board(with);
    let m = mv(&b, "e4d6");
    b.make_move(m);
    assert!(
        b.checkers().contains(sq("e1")),
        "the rook on e1 must be giving check once the knight has left e4"
    );
    assert!(
        generate_legal(&b).iter().all(|r| r.to_sq() != sq("d6")),
        "and no recapture on d6 may be legal"
    );
    b.unmake_move(m);
    check_both(with, "e4d6", 100);
    check_both(without, "e4d6", -200);
}

/// Two black pieces stand on the e-file between a white rook and the black
/// king, so neither is pinned. The knight recaptures first, and its leaving
/// pins the bishop behind it, so the bishop's recapture is illegal. A pin
/// mask taken from the root position does not see it and would let the
/// bishop take the pawn.
#[test]
fn a_pin_that_arises_during_the_exchange_is_seen() {
    let fen_w = "4k3/4b3/4n3/2p5/1P6/1N6/8/4R2K w - - 0 1";
    let mut b = board(fen_w);
    let e7 = sq("e7");
    assert!(
        !b.blockers(Colour::Black).contains(e7),
        "the bishop on e7 must not be pinned at the root"
    );
    let first = mv(&b, "b3c5");
    b.make_move(first);
    let second = mv(&b, "e6c5");
    b.make_move(second);
    assert!(
        b.blockers(Colour::Black).contains(e7),
        "and must be once the knight has left e6"
    );
    b.unmake_move(second);
    b.unmake_move(first);
    // Nxc5 100, Nxc5 300, bxc5 300, and the bishop may not take back.
    // Were it allowed: Bxc5 100 makes the knight's recapture worth 100
    // and the whole exchange 0.
    check_both(fen_w, "b3c5", 100);
}

/// A rook takes a knight defended by a queen, and a pawn guards the square.
/// The queen could recapture and does not, because the pawn would take it.
/// Without the pawn, it does.
#[test]
fn the_side_to_move_may_decline() {
    let with = "3q3k/8/8/3n4/4P3/8/8/3R3K w - - 0 1";
    let without = "3q3k/8/8/3n4/8/8/8/3R3K w - - 0 1";
    check_both(with, "d1d5", 300);
    check_both(without, "d1d5", -200);
    // And the pawn taking first: exd5 300, Qxd5 100, Rxd5 900, so the
    // queen declines and the pawn simply wins the knight.
    check_both(with, "e4d5", 300);
}

/// A quiet move onto an attacked square is an exchange with nothing taken
/// first: the rook steps into the queen's file and is lost for nothing,
/// unless the pawn defends it, when the queen would be lost for the rook
/// and declines.
#[test]
fn a_quiet_move_is_an_exchange_that_takes_nothing_first() {
    let defended = "3q3k/8/8/8/4P3/8/8/3R3K w - - 0 1";
    let hanging = "3q3k/8/8/8/8/8/8/3R3K w - - 0 1";
    check_both(defended, "d1d5", 0);
    check_both(hanging, "d1d5", -500);
    // A king step is never answered: nothing may take a king, and the move
    // is legal, so the square is safe.
    check_both(hanging, "h1h2", 0);
}

/// Castling is not an exchange.
#[test]
fn a_castle_evaluates_to_zero() {
    let b = board("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1");
    for m in generate_legal(&b).iter().filter(|m| m.is_castle()) {
        assert_eq!(see(&b, m), 0, "{}", m.to_uci_chess960());
    }
}

// ---------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------

/// The pieces a random placement draws from: pawn-heavy, like a game.
const MENU: [PieceType; 7] = [
    PieceType::Pawn,
    PieceType::Pawn,
    PieceType::Pawn,
    PieceType::Knight,
    PieceType::Bishop,
    PieceType::Rook,
    PieceType::Queen,
];

/// A random placement: two kings not touching, up to thirty other pieces
/// with no pawn on a back rank, either side to move, and an en-passant
/// square whenever the side not to move has a pawn that could just have
/// double-pushed beside an enemy pawn. Rejected, with `None`, when the
/// side not to move is in check: the function is specified over positions
/// a game can reach, and legal play cannot reach one in that state.
fn random_placement(rng: &mut Rng) -> Option<Board> {
    let mut cells: [Option<Piece>; 64] = [None; 64];
    let wk = Square::new(u8::try_from(rng.below(64)).expect("fits"));
    let bk = loop {
        let s = Square::new(u8::try_from(rng.below(64)).expect("fits"));
        let df = s.file().index().abs_diff(wk.file().index());
        let dr = s.rank().index().abs_diff(wk.rank().index());
        if s != wk && (df > 1 || dr > 1) {
            break s;
        }
    };
    cells[wk.index()] = Some(Piece::new(Colour::White, PieceType::King));
    cells[bk.index()] = Some(Piece::new(Colour::Black, PieceType::King));
    let n = rng.below(31);
    let mut placed = 0;
    let mut tries = 0;
    while placed < n && tries < 400 {
        tries += 1;
        let s = Square::new(u8::try_from(rng.below(64)).expect("fits"));
        if cells[s.index()].is_some() {
            continue;
        }
        let pt = MENU[rng.below(MENU.len())];
        if pt == PieceType::Pawn && (s.rank() == Rank::One || s.rank() == Rank::Eight) {
            continue;
        }
        let c = if rng.below(2) == 0 {
            Colour::White
        } else {
            Colour::Black
        };
        cells[s.index()] = Some(Piece::new(c, pt));
        placed += 1;
    }
    let stm = if rng.below(2) == 0 {
        Colour::White
    } else {
        Colour::Black
    };

    // An en-passant square, when a pawn of the side not to move stands on
    // the rank a double push reaches, the two squares behind it are empty,
    // and an enemy pawn stands beside it to take it.
    let them = stm.flip();
    let mut ep = None;
    for f in 0..8u8 {
        let at =
            |file: u8, rank: Rank| cells[Square::from_file_rank(File::new(file), rank).index()];
        let pawn_rank = Rank::Four.relative(them);
        let ep_rank = Rank::Three.relative(them);
        let from_rank = Rank::Two.relative(them);
        if at(f, pawn_rank) != Some(Piece::new(them, PieceType::Pawn))
            || at(f, ep_rank).is_some()
            || at(f, from_rank).is_some()
        {
            continue;
        }
        let beside = |file: u8| at(file, pawn_rank) == Some(Piece::new(stm, PieceType::Pawn));
        if (f > 0 && beside(f - 1)) || (f < 7 && beside(f + 1)) {
            ep = Some(Square::from_file_rank(File::new(f), ep_rank));
            break;
        }
    }

    let mut placement = String::new();
    for r in (0..8).rev() {
        let mut empty = 0;
        for f in 0..8 {
            let s = Square::from_file_rank(File::new(f), Rank::new(r));
            match cells[s.index()] {
                Some(p) => {
                    if empty > 0 {
                        placement.push_str(&empty.to_string());
                        empty = 0;
                    }
                    placement.push(p.to_char());
                }
                None => empty += 1,
            }
        }
        if empty > 0 {
            placement.push_str(&empty.to_string());
        }
        if r > 0 {
            placement.push('/');
        }
    }
    let fen = format!(
        "{placement} {} - {} 0 1",
        if stm == Colour::White { "w" } else { "b" },
        ep.map_or("-".to_string(), |s| s.to_string())
    );
    let b = board(&fen);
    if b.opponent_in_check() { None } else { Some(b) }
}

/// Every position the agreement runs over: the corpus; random walks from
/// the start position, the DFRC arrays and the endgame seeds; and random
/// placements, six thousand of them. The walks prefer a double push that lands beside an enemy
/// pawn when one is available, half the time, so that en-passant captures
/// are reached in numbers rather than by luck.
fn positions() -> Vec<Board> {
    let mut out: Vec<Board> = support::corpus_fens()
        .iter()
        .map(|f| board(f))
        .filter(|b| !b.opponent_in_check())
        .collect();
    let mut seeds: Vec<String> = vec![START_FEN.to_string()];
    seeds.extend(support::dfrc_arrays().into_iter().map(|(_, _, f)| f));
    seeds.extend(support::ENDGAME_FENS.iter().map(|s| (*s).to_string()));
    let mut rng = Rng::new(0x5EE0_5EE0_5EE0_0001);
    for (i, f) in seeds.iter().enumerate() {
        let seed = board(f);
        assert!(
            !seed.opponent_in_check(),
            "seed {f}: the side not to move is in check"
        );
        let walks = if i > 20 { 12 } else { 4 };
        for _ in 0..walks {
            let mut b = board(f);
            for _ in 0..80 {
                let legal = generate_legal(&b);
                if legal.is_empty() {
                    break;
                }
                let us = b.side_to_move();
                let beside_enemy_pawn: Vec<Move> = legal
                    .iter()
                    .filter(|m| {
                        m.is_double_push() && {
                            let to = m.to_sq();
                            let enemy_pawns = b.pieces(us.flip(), PieceType::Pawn);
                            let rank = to.rank().bb();
                            ((to.bb().east() | to.bb().west()) & rank & enemy_pawns).any()
                        }
                    })
                    .collect();
                let m = if !beside_enemy_pawn.is_empty() && rng.below(2) == 0 {
                    beside_enemy_pawn[rng.below(beside_enemy_pawn.len())]
                } else {
                    legal.as_slice()[rng.below(legal.len())]
                };
                b.play(m);
                out.push(b.duplicate());
            }
        }
    }
    let mut placements = 0;
    while placements < 6000 {
        if let Some(b) = random_placement(&mut rng) {
            out.push(b);
            placements += 1;
        }
    }
    out
}

#[test]
fn the_function_agrees_with_the_play_out_over_the_corpus_and_random_positions() {
    let mut census = Census::default();
    let mut best_compared = 0usize;
    let mut best_skipped = 0usize;
    let mut best_differs = 0usize;
    let mut best_higher = 0usize;
    for b in positions() {
        census.positions += 1;
        if b.in_check() {
            census.in_check_at_root += 1;
        }
        let mut work = b.duplicate();
        let mut none = Census::default();
        for m in generate_legal(&b).iter() {
            let expected = oracle(&mut work, m, false, &mut census);
            let got = see(&b, m);
            assert_eq!(
                got,
                expected,
                "{} {}: see says {got}, the play-out says {expected}",
                fen(&b),
                m.to_uci_chess960()
            );
            // Cheapest-first against both sides choosing freely, measured
            // and not gated, on the captures the exhaustive play-out can
            // afford.
            if m.is_capture() {
                if b.attackers_to(m.to_sq(), b.occupied()).count() > 8 {
                    best_skipped += 1;
                    continue;
                }
                best_compared += 1;
                let optimum = oracle(&mut work, m, true, &mut none);
                if optimum != expected {
                    best_differs += 1;
                    if optimum > expected {
                        best_higher += 1;
                    }
                }
            }
        }
    }
    eprintln!("{census:#?}");
    eprintln!(
        "cheapest-first against free choice by both sides: {best_compared} captures compared \
         ({best_skipped} skipped for having more than eight attackers), {best_differs} differ, \
         {best_higher} of those higher under free choice"
    );

    // Coverage. Each floor is about half of what the corpus reached when
    // it was written (24,080 positions, 411,512 moves, 41,163 captures),
    // so a corpus that stops reaching a case fails here rather than
    // passing vacuously.
    assert!(census.positions >= 12_000, "{census:#?}");
    assert!(census.in_check_at_root >= 1_500, "{census:#?}");
    assert!(census.captures >= 20_000, "{census:#?}");
    assert!(census.castles >= 300, "{census:#?}");
    assert!(census.en_passant >= 90, "{census:#?}");
    assert!(census.promotes_first >= 2_000, "{census:#?}");
    assert!(census.promotes_later >= 2_000, "{census:#?}");
    assert!(census.x_rays >= 8_000, "{census:#?}");
    assert!(census.pinned >= 1_400, "{census:#?}");
    assert!(census.barred_by_check >= 600, "{census:#?}");
    assert!(census.king_refused >= 3_500, "{census:#?}");
    assert!(census.king_recaptures >= 10_000, "{census:#?}");
    assert!(census.declined >= 13_000, "{census:#?}");
    assert!(census.longest >= 6, "{census:#?}");
}
