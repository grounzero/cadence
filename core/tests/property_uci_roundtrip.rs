// SPDX-License-Identifier: GPL-3.0-or-later

//! UCI round-trip: `parse_uci(emit(m)) == Some(m)`.
//!
//! Over all 960 start arrays and thousands of random positions, every legal
//! move, with `UCI_Chess960` **both on and off**.
//!
//! This is the property no node count can see. Emission and parsing do not
//! affect the search tree at all: they affect what a GUI is told, and the
//! failure mode is an illegal move in a tournament game, or an engine that
//! silently ignores the move the GUI asked it to play. The whole corpus could
//! be green with emission completely broken.
//!
//! Both modes matter separately, and the non-960 mode is where the danger is:
//! castling has to be spelled `e1g1` there, unless a quiet king move to `g1`
//! is *also* legal, in which case it must fall back to king-takes-rook because
//! `e1g1` would be ambiguous. Parsing must accept both spellings in both
//! modes regardless: the option governs output only, and GUIs get this wrong
//! often enough that liberality is free insurance.

mod support;

use cadence_core::{generate_legal, parse_uci, to_uci};
use support::generative as generate;

const RANDOM_POSITIONS: usize = 3_000;
const PLIES_PER_WALK: usize = 24;

fn round_trip(label: &str, fen: &str) {
    let board = cadence_core::Board::from_fen(fen)
        .unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})\n  {fen}"));
    let legal = generate_legal(&board);

    for m in legal.as_slice() {
        for chess960 in [true, false] {
            let s = to_uci(*m, &legal, chess960);
            assert!(
                (4..=5).contains(&s.len()),
                "{label}: `{s}` is not a UCI move string\n  {fen}"
            );
            let back = parse_uci(&legal, &s);
            assert_eq!(
                back,
                Some(*m),
                "{label}: emitted `{s}` with UCI_Chess960={chess960}, parsed back {back:?}\n  {fen}"
            );
        }

        // The option governs output only. Whichever spelling this move has in
        // the other mode must still parse, in this one.
        let spellings = [to_uci(*m, &legal, true), to_uci(*m, &legal, false)];
        for s in &spellings {
            assert_eq!(
                parse_uci(&legal, s),
                Some(*m),
                "{label}: `{s}` must parse regardless of the option's setting\n  {fen}"
            );
        }
    }
}

/// All 960 start arrays. Castling rights are live in every one of them, and
/// four of them can castle immediately, so this covers the spelling that only
/// exists in Chess960.
#[test]
fn uci_round_trips_over_all_960_start_arrays() {
    for (n, fen) in generate::all_960_start_fens().into_iter().enumerate() {
        round_trip(&format!("array {n}"), &fen);
    }
}

/// Thousands of random positions, reached by walking from the corpus seeds.
///
/// Start arrays are all pawns and back rank; the interesting spellings
/// (promotions, en passant, captures) only appear once the game has moved on.
#[test]
fn uci_round_trips_over_random_positions() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(0x1234_5678_9ABC_DEF0);
    let mut visited = 0usize;
    let mut walk = 0usize;

    while visited < RANDOM_POSITIONS {
        let fen = &seeds[walk % seeds.len()];
        walk += 1;
        let mut board = cadence_core::Board::from_fen(fen)
            .unwrap_or_else(|e| panic!("walk {walk}: FEN rejected ({e:?})\n  {fen}"));

        for _ in 0..PLIES_PER_WALK {
            round_trip(
                &format!("walk {walk}"),
                &board.to_fen(cadence_core::FenStyle::Shredder),
            );
            visited += 1;
            if visited >= RANDOM_POSITIONS {
                break;
            }
            let legal = generate::legal(&board);
            if legal.is_empty() {
                break;
            }
            board.make_move(legal[rng.below(legal.len())]);
        }
    }

    assert_eq!(visited, RANDOM_POSITIONS);
}
