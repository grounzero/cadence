// SPDX-License-Identifier: GPL-3.0-or-later

//! Unmake restoring the key is nearly tautological under copy-make; the key matching a
//! recomputation at every node bites, because all mutation goes through helpers that update board
//! and key together. Perft sees neither: a board corrupted and then correctly restored counts the
//! same leaves.

mod support;

use support::generative as generate;

/// Verification target: one hundred thousand moves across all walks.
const TOTAL_MOVES: usize = 100_000;
const MAX_PLIES_PER_WALK: usize = 200;

#[test]
fn unmake_restores_the_position_and_the_key_is_recomputable() {
    let seeds = generate::walk_seeds();
    assert!(!seeds.is_empty(), "no walk seeds");

    let mut rng = generate::Rng::new(0x0BAD_C0DE_D15E_A5E5);
    let mut moves_played = 0usize;
    let mut walk = 0usize;

    while moves_played < TOTAL_MOVES {
        let fen = &seeds[walk % seeds.len()];
        walk += 1;

        let mut board = cadence_core::Board::from_fen(fen)
            .unwrap_or_else(|e| panic!("walk {walk}: FEN rejected ({e:?})\n  {fen}"));
        let mut line: Vec<String> = Vec::new();

        for ply in 0..MAX_PLIES_PER_WALK {
            // Asserted at every node, not only at the walk's ends.
            assert_eq!(
                board.key(),
                board.recompute_key(),
                "walk {walk} ply {ply}: incremental key disagrees with recomputation\n  \
                 from {fen}\n  after {line:?}"
            );

            let legal = generate::legal(&board);
            if legal.is_empty() {
                break;
            }
            let m = legal[rng.below(legal.len())];
            let uci = m.to_uci_chess960();

            let before = generate::fingerprint(&board);
            board.make_move(m);
            board.unmake_move(m);
            let after = generate::fingerprint(&board);

            assert_eq!(
                after, before,
                "walk {walk} ply {ply}: unmake did not restore after {uci}\n  \
                 from {fen}\n  after {line:?}"
            );

            board.make_move(m);
            line.push(uci);
            moves_played += 1;
            if moves_played >= TOTAL_MOVES {
                break;
            }
        }
    }

    assert_eq!(moves_played, TOTAL_MOVES);
}
