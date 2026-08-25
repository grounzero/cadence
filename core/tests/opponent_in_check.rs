// SPDX-License-Identifier: GPL-3.0-or-later

//! Positions where the side not to move is in check.
//!
//! No position reachable by legal play is like this, and `from_fen` accepts
//! one anyway: it validates that a position is representable, not that it is
//! reachable. GUIs, analysis tools and malformed input all produce them, so
//! the contract this file gates is that **`core` is total on any position
//! `from_fen` accepts**. The mechanism is that a king is never a target: no
//! generated move can take one, so the invariant `king_square` rests on
//! survives any sequence of `make_move`, and nothing downstream has to carry
//! an `Option` for a square that is always there.
//!
//! **Nothing else in the tree gates this, by construction, and that is why
//! the file exists.** The naive generator excludes the enemy king from its
//! target sets, so `legal_vs_naive` compared two generators that disagreed
//! on exactly this case and was never run anywhere they would; every walk
//! seed is a legal position; `engine/tests/bench.rs` and
//! `engine/tests/eval.rs` each assert that their own position list holds no
//! such FEN. Every one of those is a fence around an input. This is the
//! fence around the engine.
//!
//! Two families, because they break differently. In the first the side to
//! move can capture the enemy king, which is what the target sets decide. In
//! the second the kings are adjacent, and the enemy king also turns up in
//! the attacker set that decides check: it is a "checker" that no evasion
//! can answer, and with two real checkers beside it there are three, which a
//! position reachable by legal play cannot have.

mod support;

use cadence_core::position::Board;
use cadence_core::types::PieceType;
use cadence_core::{Move, generate_legal, generate_noisy, perft, perft_divide};
use support::generative::Rng;
use support::naive;

/// Placements drawn per family. The unbiased generator puts the side not to
/// move in check about a quarter of the time, so this yields ~500 of the
/// first family, and the coverage assertions below hold it to that.
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

/// Random placements in which the side not to move is in check, and the
/// count drawn to find them.
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

/// Every position this file runs on: the reported reproductions, then both
/// random families.
fn corpus() -> Vec<String> {
    let mut out: Vec<String> = REPORTED.iter().map(|f| (*f).to_string()).collect();
    out.extend(placements_with_the_opponent_in_check(0x1E9A_2000_0000_0011));
    out.extend(placements_with_touching_kings(0x1E9A_2000_0000_0021));
    out
}

/// The move set holds no move onto a square occupied by a king.
///
/// Stated over the mailbox rather than over `Move::is_capture`, because it
/// is the destination's occupant that decides whether `make_move` removes a
/// king, and a generator that spelled the capture as a quiet move would be
/// just as fatal.
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
    // The coverage assertion, not an assumption: the tests below are
    // vacuous if the generators stop producing the case, and a generator
    // that quietly stopped is exactly how a fence gets rebuilt.
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

    // Three or more pieces attacking the king of the side to move. A
    // position reachable by legal play cannot have it -- a discovered check
    // reveals at most one piece besides the mover -- and generation used to
    // assert so outright, which arbitrary input refutes.
    let many: usize = corpus()
        .iter()
        .filter(|fen| board(fen).checkers().count() >= 3)
        .count();
    assert!(
        many >= 10,
        "only {many} positions with three or more checkers"
    );
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
    // There is no external oracle for a position that cannot occur, so the
    // assertion is what perft can check about itself: it returns, and the
    // divide sums to the total. What is being gated is that the process is
    // still alive to be asked -- `cadence perft` on the first of these dies
    // with SIGABRT.
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
    // Not every one of them has a move, and the exception is instructive
    // rather than a failure: with the kings adjacent on a8 and b8 and Black
    // to move, both squares Black's king could go to are attacked by White's
    // and the third is White's king, which is not a target. Zero legal moves
    // is the right answer there. What would be wrong is all of them being
    // zero, which is what a mask applied too widely would look like.
    assert_eq!(with_moves, REPORTED.len() - 1, "wrong count with a move");

    // Deeper, over the random families, so the recursion runs on positions
    // it reaches rather than only on the ones it starts from.
    for fen in corpus().iter().take(200) {
        let mut b = board(fen);
        perft(&mut b, 3);
    }
}
