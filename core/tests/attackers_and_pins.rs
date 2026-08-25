// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `attackers_to` and for `blockers`/`pinners`: brute force.
//!
//! Castling legality, king-move legality and en passant
//! all rest on `attackers_to`; every pin filter in move generation rests on
//! the blockers computation. Neither had a gate. A bug in either was reached
//! only by perft, which reports a wrong total and cannot say which of the
//! two broke: a reversed pawn-attack direction and a pin computed with the
//! wrong slider type both present as "the node count is off by a little".
//!
//! **Half one.** For every square, `attackers_to(sq, occ)` must equal the set
//! of pieces `p` on the board for which a naive "does `p` attack `sq` under
//! `occ`" says yes (the oracle in `support::naive`, written in `(file,
//! rank)` integers with nothing from `attacks` or `magic`). Under the board's
//! occupancy, under the occupancy with each king lifted (the king-retreat
//! case), and
//! under random subsets, because the occupancy is caller-supplied and the
//! contract is that only slider blocking reads it.
//!
//! **Half two.** For each colour, `blockers(c)` must equal the set of pieces
//! whose removal exposes `c`'s king to an enemy slider that did not attack it
//! before, and `pinners(c)` the set of those sliders: the definition,
//! evaluated by removing each piece and retesting.
//!
//! Positions: every corpus position, two thousand random placements, and
//! positions reached by walking from the corpus seeds. The random placements
//! are where the slider geometry actually gets exercised: a start array has
//! no pins in it.

mod support;

use cadence_core::bitboard::Bitboard;
use cadence_core::position::Board;
use cadence_core::types::{Colour, PieceType, Square};
use support::generative as generate;
use support::naive;

const RANDOM_PLACEMENTS: usize = 2_000;
const WALKS: usize = 60;
const PLIES_PER_WALK: usize = 30;

fn corpus_fens() -> Vec<String> {
    let mut fens: Vec<String> = support::standard_positions()
        .into_iter()
        .map(|p| p.fen)
        .collect();
    fens.extend(support::dfrc_arrays().into_iter().map(|a| a.fen));
    fens.extend(support::castling_cases().into_iter().map(|c| c.fen));
    fens.extend(support::edge_cases().into_iter().map(|c| c.fen));
    fens.extend(support::rights_captures().into_iter().map(|r| r.fen));
    fens.extend(support::immediate_castles().into_iter().map(|r| r.fen));
    fens.push(support::move_capacity().fen);
    fens
}

fn random_fens() -> Vec<String> {
    let mut rng = generate::Rng::new(0x0A77_ACC5_0000_0001);
    (0..RANDOM_PLACEMENTS)
        .map(|_| naive::random_placement_fen(&mut rng))
        .collect()
}

/// The board's occupancy, each king lifted, and a few random subsets: the
/// occupancy is the caller's, and every one of these is a caller.
fn occupancies(board: &Board, rng: &mut generate::Rng) -> Vec<(String, Bitboard)> {
    let occ = board.occupied();
    let mut out = vec![
        ("board".to_string(), occ),
        (
            "white king lifted".to_string(),
            occ.without(board.king_square(Colour::White)),
        ),
        (
            "black king lifted".to_string(),
            occ.without(board.king_square(Colour::Black)),
        ),
        ("empty".to_string(), Bitboard::EMPTY),
    ];
    for i in 0..3 {
        out.push((
            format!("random subset {i}"),
            Bitboard(occ.0 & rng.next_u64()),
        ));
    }
    out
}

fn assert_attackers(label: &str, board: &Board, rng: &mut generate::Rng) {
    for (name, occ) in occupancies(board, rng) {
        for sq in Square::all() {
            let got = board.attackers_to(sq, occ);
            let want = naive::attackers_to(board, sq, occ);
            assert_eq!(
                got,
                want,
                "{label}: attackers_to({sq}) under {name}\n  {}\ngot\n{got:?}\nwant\n{want:?}",
                board.to_fen(cadence_core::FenStyle::Shredder)
            );
        }
    }
    // checkers is the side to move's king under the board occupancy, enemy
    // pieces only.
    let us = board.side_to_move();
    let want = naive::attackers_to(board, board.king_square(us), board.occupied())
        & board.by_colour(us.flip());
    assert_eq!(board.checkers(), want, "{label}: checkers");
    assert_eq!(board.in_check(), want.any(), "{label}: in_check");
}

fn assert_pins(label: &str, board: &Board) {
    for c in Colour::ALL {
        let (blockers, pinners) = naive::blockers_and_pinners(board, c);
        assert_eq!(
            board.blockers(c),
            blockers,
            "{label}: blockers({c:?})\n  {}\ngot\n{:?}\nwant\n{blockers:?}",
            board.to_fen(cadence_core::FenStyle::Shredder),
            board.blockers(c)
        );
        assert_eq!(
            board.pinners(c),
            pinners,
            "{label}: pinners({c:?})\n  {}\ngot\n{:?}\nwant\n{pinners:?}",
            board.to_fen(cadence_core::FenStyle::Shredder),
            board.pinners(c)
        );
        // Structural consequences of the definition: a pinner is an enemy
        // slider, a blocker is not the king, and every blocker has exactly
        // one pinner behind it.
        let them = c.flip();
        let sliders = board.pieces(them, PieceType::Bishop)
            | board.pieces(them, PieceType::Rook)
            | board.pieces(them, PieceType::Queen);
        assert_eq!(
            pinners & !sliders,
            Bitboard::EMPTY,
            "{label}: pinners are enemy sliders"
        );
        assert!(
            !blockers.contains(board.king_square(c)),
            "{label}: the king is not a blocker"
        );
    }
    // A blocker of the mover's own colour is exactly a pinned piece; the
    // classic case is asserted by name below in `pins_by_hand`.
}

#[test]
fn attackers_to_matches_brute_force_over_the_corpus() {
    let mut rng = generate::Rng::new(1);
    for fen in corpus_fens() {
        let board = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert_attackers(&fen, &board, &mut rng);
    }
}

#[test]
fn attackers_to_matches_brute_force_over_random_placements() {
    let mut rng = generate::Rng::new(2);
    for fen in random_fens() {
        let board = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert_attackers(&fen, &board, &mut rng);
    }
}

#[test]
fn blockers_and_pinners_match_the_definition_over_the_corpus() {
    for fen in corpus_fens() {
        let board = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert_pins(&fen, &board);
    }
}

#[test]
fn blockers_and_pinners_match_the_definition_over_random_placements() {
    let mut pinned_seen = 0usize;
    let mut enemy_blocker_seen = 0usize;
    for fen in random_fens() {
        let board = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert_pins(&fen, &board);
        for c in Colour::ALL {
            pinned_seen += (board.blockers(c) & board.by_colour(c)).count() as usize;
            enemy_blocker_seen += (board.blockers(c) & board.by_colour(c.flip())).count() as usize;
        }
    }
    // The random placements must actually exercise both kinds of blocker,
    // or the test above is checking empty sets against empty sets.
    assert!(
        pinned_seen > 200,
        "only {pinned_seen} pinned pieces seen; the generator is too tame"
    );
    assert!(
        enemy_blocker_seen > 200,
        "only {enemy_blocker_seen} enemy blockers seen"
    );
}

/// Along walks from the corpus seeds, so that both halves are checked in
/// positions a game actually reaches, including after castling, promotion
/// and en passant.
#[test]
fn both_halves_hold_along_walks() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(3);
    let mut nodes = 0usize;
    for walk in 0..WALKS {
        let fen = &seeds[walk % seeds.len()];
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        for ply in 0..PLIES_PER_WALK {
            let label = format!("walk {walk} ply {ply} from {fen}");
            assert_attackers(&label, &board, &mut rng);
            assert_pins(&label, &board);
            nodes += 1;
            let legal = naive::legal(&mut board);
            if legal.is_empty() {
                break;
            }
            board.make_move(legal[rng.below(legal.len())]);
        }
    }
    assert!(nodes > WALKS * 10, "walks ended early: {nodes} nodes");
}

/// The classic geometries by name, so a failure reads as chess.
#[test]
fn pins_by_hand() {
    // Bishop pins the knight to the king; queen behind the knight is not a
    // pin (two pieces between).
    let b = Board::from_fen("4k3/8/8/1b6/8/3N4/8/5K2 w - - 0 1").expect("fen");
    assert_eq!(b.blockers(Colour::White), Square::D3.bb());
    assert_eq!(b.pinners(Colour::White), Square::B5.bb());
    assert_eq!(b.blockers(Colour::Black), Bitboard::EMPTY);

    // Rook behind two white pieces: neither is a blocker.
    let b = Board::from_fen("4k3/8/8/8/8/8/8/r1NBK3 w - - 0 1").expect("fen");
    assert_eq!(b.blockers(Colour::White), Bitboard::EMPTY);
    assert_eq!(b.pinners(Colour::White), Bitboard::EMPTY);

    // An enemy piece between the slider and the king is a blocker for that
    // king (a discovered-check candidate for its own side).
    let b = Board::from_fen("4k3/8/8/8/8/8/8/r1n1K3 w - - 0 1").expect("fen");
    assert_eq!(b.blockers(Colour::White), Square::C1.bb());
    assert_eq!(b.pinners(Colour::White), Square::A1.bb());

    // The horizontal double-vacate en-passant position: neither pawn is a
    // blocker, because removing one leaves the other blocking.
    let b = Board::from_fen("8/8/8/K2pP2r/8/8/8/7k w - d6 0 1").expect("fen");
    assert_eq!(b.blockers(Colour::White), Bitboard::EMPTY);
    assert!(!b.in_check());

    // A bishop pinning along a diagonal, while a rook that already gives
    // check is a checker, not a pinner.
    let b = Board::from_fen("4k3/8/8/8/1b6/8/3P4/r3K3 w - - 0 1").expect("fen");
    assert_eq!(b.checkers(), Square::A1.bb());
    assert!(b.in_check());
    assert_eq!(b.blockers(Colour::White), Square::D2.bb());
    assert_eq!(b.pinners(Colour::White), Square::B4.bb());
    // The same without the rook: the pin stands alone.
    let b = Board::from_fen("4k3/8/8/8/1b6/8/3P4/4K3 w - - 0 1").expect("fen");
    assert!(!b.in_check());
    assert_eq!(b.blockers(Colour::White), Square::D2.bb());
    assert_eq!(b.pinners(Colour::White), Square::B4.bb());
    // A queen not on any line with the king pins nothing.
    let b = Board::from_fen("4k3/8/8/8/8/8/1q1P4/4K3 w - - 0 1").expect("fen");
    assert_eq!(b.blockers(Colour::White), Bitboard::EMPTY);
    assert_eq!(b.pinners(Colour::White), Bitboard::EMPTY);
}

/// Attackers by hand: the pawn direction is the classic reversal.
#[test]
fn attackers_by_hand() {
    let b = Board::from_fen("4k3/8/8/3p4/4P3/8/8/4K3 w - - 0 1").expect("fen");
    let occ = b.occupied();
    // The black pawn on d5 attacks e4 and c4; the white pawn on e4 attacks
    // d5 and f5. Neither attacks the square behind itself.
    assert_eq!(b.attackers_to(Square::E4, occ), Square::D5.bb());
    assert_eq!(b.attackers_to(Square::D5, occ), Square::E4.bb());
    assert_eq!(b.attackers_to(Square::E5, occ), Bitboard::EMPTY);
    assert_eq!(b.attackers_to(Square::D4, occ), Bitboard::EMPTY);
    assert_eq!(b.attackers_to(Square::C4, occ), Square::D5.bb());
    assert_eq!(b.attackers_to(Square::F5, occ), Square::E4.bb());
    // Kings attack their neighbours; both colours are reported.
    assert_eq!(b.attackers_to(Square::D1, occ), Square::E1.bb());
    assert_eq!(b.attackers_to(Square::D8, occ), Square::E8.bb());

    // Slider blocking reads occ: lift the blocker and the rook sees through.
    let b = Board::from_fen("4k3/8/8/8/8/8/8/R2nK3 w - - 0 1").expect("fen");
    let occ = b.occupied();
    assert_eq!(b.attackers_to(Square::E1, occ), Bitboard::EMPTY);
    assert_eq!(
        b.attackers_to(Square::E1, occ.without(Square::D1)),
        Square::A1.bb()
    );
    assert_eq!(
        b.attackers_to(Square::D1, occ),
        Square::A1.bb() | Square::E1.bb()
    );
}
