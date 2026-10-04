// SPDX-License-Identifier: GPL-3.0-or-later

//! No node count sees emission or parsing, whose failure is an illegal move in a tournament game.
//! Non-960 castling spells `e1g1` unless a quiet king move to `g1` is also legal; parsing accepts
//! both spellings in both modes.

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

/// Four can castle immediately, covering the spelling only Chess960 has.
#[test]
fn uci_round_trips_over_all_960_start_arrays() {
    for (n, fen) in generate::all_960_start_fens().into_iter().enumerate() {
        round_trip(&format!("array {n}"), &fen);
    }
}

/// Promotions, en passant and captures only appear once the game has moved on.
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
