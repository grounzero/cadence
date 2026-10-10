// SPDX-License-Identifier: GPL-3.0-or-later

//! `core` must be total on any position `from_fen` accepts, which a king never being a target
//! guarantees. Every other gate fences an unreachable input out; this is the fence around the
//! engine.

mod support;

use cadence_core::position::Board;
use cadence_core::types::PieceType;
use cadence_core::{Move, generate_legal, generate_noisy, perft, perft_divide};
use support::generative::Rng;
use support::naive;

/// The unbiased generator puts the opponent in check about a quarter of the time, so ~500 of the
/// first family.
const PLACEMENTS: usize = 2000;

/// The two original crash reproductions and the touching-kings cases that
/// reach the same abort through the checkers.
const REPORTED: [&str; 4] = [
    "k7/8/8/8/8/8/8/R6K w - - 0 1",
    "4k3/8/8/8/8/8/8/4R2K w - - 0 1",
    "kK6/8/8/8/8/8/8/8 w - - 0 1",
    "kK6/8/8/8/8/8/8/8 b - - 0 1",
];

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

fn placements_with_the_opponent_in_check(seed: u64) -> Vec<String> {
    let mut rng = Rng::new(seed);
    (0..PLACEMENTS)
        .map(|_| naive::random_placement_fen(&mut rng))
        .filter(|fen| board(fen).opponent_in_check())
        .collect()
}

fn placements_with_touching_kings(seed: u64) -> Vec<String> {
    let mut rng = Rng::new(seed);
    (0..PLACEMENTS)
        .map(|_| naive::random_touching_kings_fen(&mut rng))
        .collect()
}

fn corpus() -> Vec<String> {
    let mut out: Vec<String> = REPORTED.iter().map(|f| (*f).to_string()).collect();
    out.extend(placements_with_the_opponent_in_check(0x1E9A_2000_0000_0011));
    out.extend(placements_with_touching_kings(0x1E9A_2000_0000_0021));
    out
}

/// Over the mailbox, not `is_capture`: the destination's occupant decides whether `make_move`
/// removes a king.
fn assert_no_king_is_a_target(label: &str, b: &Board) {
    let kings = b.by_type(PieceType::King);
    let offending: Vec<Move> = generate_legal(b)
        .iter()
        .chain(generate_noisy(b).iter())
        .filter(|m| kings.contains(m.to_sq()))
        .collect();
    assert!(
        offending.is_empty(),
        "{label}: generation offers a king capture {:?}\n  {}",
        offending
            .iter()
            .map(|m| m.to_uci_chess960())
            .collect::<Vec<_>>(),
        b.to_fen(cadence_core::FenStyle::Shredder)
    );
}

#[test]
fn the_placements_reach_both_families() {
    // The tests below are vacuous if the generators stop producing the case.
    let in_check = placements_with_the_opponent_in_check(0x1E9A_2000_0000_0011);
    assert!(
        in_check.len() >= 300,
        "only {} of {PLACEMENTS} placements have the side not to move in check",
        in_check.len()
    );

    let touching = placements_with_touching_kings(0x1E9A_2000_0000_0021);
    let mut both_ways = 0;
    for fen in &touching {
        let b = board(fen);
        assert!(b.opponent_in_check(), "{fen}: kings do not touch");
        if b.in_check() {
            both_ways += 1;
        }
    }
    assert_eq!(
        both_ways,
        touching.len(),
        "adjacent kings check each other; the side to move is in check too"
    );

    // Three or more checkers, impossible by legal play, which generation once asserted outright.
    let many: usize = corpus()
        .iter()
        .filter(|fen| board(fen).checkers().count() >= 3)
        .count();
    assert!(
        many >= 10,
        "only {many} positions with three or more checkers"
    );
}

/// The other half of the predicate: every placement above reads true, and nothing else checked
/// that it ever reads false.
#[test]
fn the_side_not_to_move_is_never_in_check_after_a_legal_move() {
    for fen in [
        cadence_core::START_FEN,
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    ] {
        let mut b = board(fen);
        assert!(!b.opponent_in_check(), "{fen}: the opponent reads in check");
        for m in generate_legal(&b).iter() {
            b.make_move(m);
            assert!(
                !b.opponent_in_check(),
                "{fen} after {}: the mover reads in check",
                m.to_uci_chess960()
            );
            b.unmake_move(m);
        }
    }
}

#[test]
fn no_generated_move_takes_a_king() {
    for fen in corpus() {
        assert_no_king_is_a_target(&fen, &board(&fen));
    }
}

#[test]
fn make_and_unmake_survive_the_whole_move_list() {
    // The direct reproduction: `make_move` recomputes the check info, which
    // asks for the king of the side that has just moved into the position.
    for fen in corpus() {
        let mut b = board(&fen);
        let before = b.to_fen(cadence_core::FenStyle::Shredder);
        let key = b.key();
        for m in generate_legal(&b).iter() {
            b.make_move(m);
            assert_no_king_is_a_target(&format!("{fen} after {}", m.to_uci_chess960()), &b);
            b.unmake_move(m);
            assert_eq!(
                b.to_fen(cadence_core::FenStyle::Shredder),
                before,
                "{fen}: {} did not round trip",
                m.to_uci_chess960()
            );
            assert_eq!(
                b.key(),
                key,
                "{fen}: {} left the key changed",
                m.to_uci_chess960()
            );
        }
    }
}

#[test]
fn perft_runs_where_the_opponent_is_in_check() {
    // No external oracle exists, so perft must return and its divide sum to the total; `cadence
    // perft` once died with SIGABRT on the first of these.
    let mut with_moves = 0;
    for fen in REPORTED {
        let mut b = board(fen);
        assert_eq!(
            perft(&mut b, 0),
            1,
            "{fen}: depth 0 is one node by definition"
        );
        for depth in 1..=3 {
            let total = perft(&mut b, depth);
            let rows = perft_divide(&mut b, depth);
            let summed: u64 = rows.iter().map(|(_, n)| n).sum();
            assert_eq!(total, summed, "{fen} at depth {depth}: divide disagrees");
        }
        if perft(&mut b, 1) > 0 {
            with_moves += 1;
        }
    }
    // Kings adjacent on a8 and b8 with Black to move leave Black no legal move, correctly. All of
    // them having none would be a mask applied too widely.
    assert_eq!(with_moves, REPORTED.len() - 1, "wrong count with a move");

    // Deeper, over the random families, so the recursion runs on positions
    // it reaches rather than only on the ones it starts from.
    for fen in corpus().iter().take(200) {
        let mut b = board(fen);
        perft(&mut b, 3);
    }
}
