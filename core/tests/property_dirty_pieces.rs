// SPDX-License-Identifier: GPL-3.0-or-later

//! `DirtyPieces` verified in feature space.
//!
//! This test closes a gap that otherwise surfaces only when a network fails
//! to train. `make_move` returns a
//! `DirtyPieces`; perft ignores it; the stores that build it are
//! dead-store-eliminable once `make_move` inlines. So it can be **entirely
//! unwritten**, or written wrongly, and every node count in the corpus still
//! verifies. Nothing else in this repository reads it until an accumulator
//! exists, and nothing can until there is a network for it to feed.
//!
//! Verified in feature space rather than by mailbox diff, because the mailbox
//! is what `make_move` already maintains and comparing a thing to itself
//! proves nothing. The 768-dimensional occupancy vector is what the network
//! actually consumes, and the delta has to be correct *in that space*.
//!
//! Three assertions, and the second is the one that catches DFRC castling:
//!
//! 1. Applying the delta (**all `from` subtractions, then all `to`
//!    additions**) to the pre-move vector yields the post-move vector.
//! 2. No intermediate value leaves `{0, 1}`. A naive replay that applies each
//!    entry completely, subtract-then-add per piece, puts a piece onto a
//!    square a later entry is about to vacate: against a mailbox that
//!    silently clobbers, against this vector it produces a `2` or a `-1`.
//!    28.7% of DFRC castles move only one of the two pieces, so this is not a
//!    corner case.
//! 3. The inverse delta restores the pre-move vector.
//!
//! Both perspectives, because the flip is half of what `feature_index` does.

mod support;

use cadence_core::types::{Colour, Square};
use cadence_core::{NUM_INPUTS, feature_index};
use support::generative as generate;

const WALKS: usize = 400;
const PLIES_PER_WALK: usize = 40;

/// The 768-wide occupancy vector of a position, from one perspective.
fn occupancy(board: &cadence_core::Board, persp: Colour) -> Vec<i32> {
    let mut v = vec![0i32; NUM_INPUTS];
    for i in 0..64u8 {
        let sq = Square::new(i);
        if let Some(piece) = board.piece_at(sq) {
            v[feature_index(persp, piece, sq)] += 1;
        }
    }
    v
}

#[test]
fn dirty_pieces_reproduce_the_feature_space_delta() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(0x0D1B_7ED0_C0FF_EE01);

    for walk in 0..WALKS {
        let fen = &seeds[walk % seeds.len()];
        let mut board = cadence_core::Board::from_fen(fen)
            .unwrap_or_else(|e| panic!("walk {walk}: FEN rejected ({e:?})\n  {fen}"));

        for ply in 0..PLIES_PER_WALK {
            let legal = generate::legal(&board);
            if legal.is_empty() {
                break;
            }
            let m = legal[rng.below(legal.len())];
            let uci = m.to_uci_chess960();

            for persp in [Colour::White, Colour::Black] {
                let before = occupancy(&board, persp);

                let dirty = board.make_move(m);
                let after = occupancy(&board, persp);
                board.unmake_move(m);

                let entries = dirty.as_slice();
                assert!(
                    entries.len() <= cadence_core::MAX_DIRTY,
                    "walk {walk} ply {ply} {uci}: {} entries exceeds MAX_DIRTY",
                    entries.len()
                );
                assert!(
                    entries.len() <= cadence_core::MAX_DIRTY_REACHABLE,
                    "walk {walk} ply {ply} {uci}: {} entries; the reachable maximum is {}, so \
                     either this is a bug or the proof in the design is wrong",
                    entries.len(),
                    cadence_core::MAX_DIRTY_REACHABLE
                );

                let ctx = format!("walk {walk} ply {ply} {uci} {persp:?}\n  from {fen}");
                let mut v = before.clone();

                // First pass: every subtraction.
                for e in entries {
                    if let Some(from) = e.from.get() {
                        let i = feature_index(persp, e.piece, from);
                        v[i] -= 1;
                        assert!(
                            (0..=1).contains(&v[i]),
                            "{ctx}: feature {i} left {{0,1}} at {} during subtraction",
                            v[i]
                        );
                    }
                }
                // Second pass: every addition. Never interleaved with the
                // first: that is the whole ordering contract.
                for e in entries {
                    if let Some(to) = e.to.get() {
                        let i = feature_index(persp, e.piece, to);
                        v[i] += 1;
                        assert!(
                            (0..=1).contains(&v[i]),
                            "{ctx}: feature {i} left {{0,1}} at {} during addition",
                            v[i]
                        );
                    }
                }
                assert_eq!(v, after, "{ctx}: delta does not reach the post-move vector");

                // And back again.
                for e in entries {
                    if let Some(to) = e.to.get() {
                        v[feature_index(persp, e.piece, to)] -= 1;
                    }
                }
                for e in entries {
                    if let Some(from) = e.from.get() {
                        v[feature_index(persp, e.piece, from)] += 1;
                    }
                }
                assert_eq!(
                    v, before,
                    "{ctx}: inverse delta does not restore the pre-move vector"
                );
            }

            board.make_move(m);
        }
    }
}

/// A null move must produce an empty delta, and it is the **only** move that
/// may: castling where neither piece moves is unreachable, proved
/// exhaustively over all 960 arrays.
#[test]
fn only_a_null_move_produces_an_empty_delta() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(0x5EED_0000_0000_0001);

    for walk in 0..WALKS {
        let fen = &seeds[walk % seeds.len()];
        let mut board = cadence_core::Board::from_fen(fen)
            .unwrap_or_else(|e| panic!("walk {walk}: FEN rejected ({e:?})\n  {fen}"));

        for _ in 0..PLIES_PER_WALK {
            let legal = generate::legal(&board);
            if legal.is_empty() {
                break;
            }
            let m = legal[rng.below(legal.len())];
            let dirty = board.make_move(m);
            assert!(
                !dirty.as_slice().is_empty(),
                "{} produced an empty delta; only a null move may\n  {fen}",
                m.to_uci_chess960()
            );
        }
    }
}
