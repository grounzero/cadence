// SPDX-License-Identifier: GPL-3.0-or-later

//! `unmake_move` restores everything, and the incremental key is real.
//!
//! Two properties in one walk, and only one of them has teeth.
//!
//! "`unmake` restores the key" is nearly tautological under copy-make: the key
//! is part of the per-ply snapshot being restored, so it comes back whether or
//! not it was ever correct. The half that bites is the second assertion:
//! the incrementally maintained key equalling a from-scratch recomputation at
//! **every node**, which is only a real invariant because all board mutation
//! goes through one set of helpers that update the bitboards, the mailbox and
//! the key together. If the key can only change where the board changes, then
//! a disagreement means the board changed somewhere it should not have.
//!
//! The mailbox and the twelve piece bitboards are fingerprinted separately for
//! the same reason: the failure being hunted is the two disagreeing, and a
//! fingerprint derived from one of them cannot see it.
//!
//! Nothing in perft finds either of these. Perft counts leaves; a board that
//! is corrupt and then correctly restored counts the same leaves.

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
            // The half with teeth, asserted at every node rather than only at
            // the ends of the walk.
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
