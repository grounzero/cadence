// SPDX-License-Identifier: GPL-3.0-or-later

//! `generate_legal` against an independent legal generator, as move sets.
//!
//! The corpus says how many moves there are and, for a handful of positions,
//! which. This says which, everywhere: in every corpus position and at every
//! node of walks from the corpus seeds, the generator's move set must equal
//! the set produced by `support::naive::legal`, the obvious pseudo-legal
//! generator plus a make/unmake king-safety filter, written as a move source
//! for the position gates and kept as a second opinion on this one.
//!
//! The two share the attack tables and `attackers_to`, both gated on their
//! own, and nothing else: the naive generator knows no pins, no check
//! dispatch, no evasion masks and no en-passant occupancy rule. It answers
//! "is the king attacked afterwards" by actually playing the move. So when
//! the two disagree the diff names the move, and the disagreement is in the
//! fast generator's legality reasoning, not in what a piece attacks.
//!
//! Duplicates are caught separately: a generator that emits a move twice has
//! the right set and the wrong count, and perft would see a wrong total with
//! nothing to localise it.
//!
//! **The one place the two used to disagree was a place this file never
//! went.** The naive generator excludes the enemy king from its target sets,
//! so where the side not to move is in check it declines the king capture
//! and the fast generator offered it; the corpus and every walk seed is a
//! legal position, so the comparison never ran anywhere that shows. Both
//! generators now decline it, and the last test here is the one that runs on
//! positions where they would once have differed. `tests/opponent_in_check.rs`
//! holds the rest of the family.

mod support;

use cadence_core::position::Board;
use cadence_core::{Move, generate_legal};
use support::generative as generate;
use support::naive;

const WALKS: usize = 150;
const PLACEMENTS: usize = 2000;
const PLIES_PER_WALK: usize = 60;

fn uci_set(moves: &[Move]) -> Vec<String> {
    let mut v: Vec<String> = moves.iter().map(|m| m.to_uci_chess960()).collect();
    v.sort();
    v
}

/// The generator's moves equal the naive generator's, as a set and in count.
fn assert_same_moves(label: &str, board: &mut Board) {
    let fast = generate_legal(board).as_slice().to_vec();
    let slow = naive::legal(board);
    let fast_uci = uci_set(&fast);
    let slow_uci = uci_set(&slow);

    let fast_set: std::collections::BTreeSet<&String> = fast_uci.iter().collect();
    let slow_set: std::collections::BTreeSet<&String> = slow_uci.iter().collect();
    let missing: Vec<&&String> = slow_set.difference(&fast_set).collect();
    let spurious: Vec<&&String> = fast_set.difference(&slow_set).collect();
    assert!(
        missing.is_empty() && spurious.is_empty(),
        "{label}\n  {}\n  generate_legal is missing {missing:?}\n  generate_legal has spurious {spurious:?}\n  fast ({}) {fast_uci:?}\n  slow ({}) {slow_uci:?}",
        board.to_fen(cadence_core::FenStyle::Shredder),
        fast_uci.len(),
        slow_uci.len()
    );
    assert_eq!(
        fast_uci.len(),
        fast_set.len(),
        "{label}: generate_legal emitted a move twice\n  {}\n  {fast_uci:?}",
        board.to_fen(cadence_core::FenStyle::Shredder)
    );
    // The moves themselves, not just their spellings: the same flag bits.
    let mut fast_bits: Vec<u16> = fast.iter().map(|m| m.to_bits()).collect();
    let mut slow_bits: Vec<u16> = slow.iter().map(|m| m.to_bits()).collect();
    fast_bits.sort_unstable();
    slow_bits.sort_unstable();
    assert_eq!(
        fast_bits,
        slow_bits,
        "{label}: same spellings, different encodings (a flag differs)\n  {}\n  fast {fast:?}\n  slow {slow:?}",
        board.to_fen(cadence_core::FenStyle::Shredder)
    );
}

#[test]
fn generate_legal_matches_the_naive_generator_in_every_corpus_position() {
    let mut fens: Vec<String> = support::standard_positions()
        .into_iter()
        .map(|p| p.fen)
        .collect();
    fens.extend(support::dfrc_arrays().into_iter().map(|a| a.fen));
    fens.extend(support::castling_cases().into_iter().map(|c| c.fen));
    fens.extend(support::edge_cases().into_iter().map(|c| c.fen));
    fens.extend(support::rights_captures().into_iter().map(|r| r.fen));
    fens.extend(support::immediate_castles().into_iter().map(|r| r.fen));
    fens.extend(
        support::fen_notations()
            .into_iter()
            .flat_map(|f| [f.shredder, f.xfen]),
    );
    fens.push(support::move_capacity().fen);
    for fen in fens {
        let mut board = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert_same_moves(&fen, &mut board);
    }
}

#[test]
fn generate_legal_matches_the_naive_generator_along_walks() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(0x1E9A_1000_0000_0007);
    let mut nodes = 0usize;
    let mut in_check = 0usize;
    for walk in 0..WALKS {
        let fen = &seeds[walk % seeds.len()];
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        for ply in 0..PLIES_PER_WALK {
            assert_same_moves(&format!("walk {walk} ply {ply} from {fen}"), &mut board);
            nodes += 1;
            if board.in_check() {
                in_check += 1;
            }
            let legal = generate_legal(&board);
            if legal.is_empty() {
                break;
            }
            // Prefer the rare kinds, as the position walk does, so castling,
            // en passant and promotion are compared often rather than by luck.
            let rare: Vec<Move> = legal
                .iter()
                .filter(|m| m.is_castle() || m.is_en_passant() || m.is_promotion())
                .collect();
            let m = if !rare.is_empty() && rng.below(2) == 0 {
                rare[rng.below(rare.len())]
            } else {
                legal.as_slice()[rng.below(legal.len())]
            };
            board.make_move(m);
        }
    }
    eprintln!("{nodes} nodes compared, {in_check} of them in check");
    assert!(nodes >= WALKS * 20, "walks ended early: {nodes} nodes");
    assert!(
        in_check > 100,
        "too few in-check nodes ({in_check}) to have compared evasions"
    );
}

#[test]
fn generate_legal_matches_the_naive_generator_with_the_opponent_in_check() {
    // Positions no legal play can reach, which `from_fen` accepts: the case
    // both generators have an opinion about and neither was ever asked. The
    // first family is a random placement filtered for it; the second has the
    // kings adjacent, where the enemy king is an attacker of our king as
    // well as a piece on a square we can move to.
    let mut rng = generate::Rng::new(0x1E9A_3000_0000_0003);
    let mut compared = 0usize;
    let mut touching = 0usize;
    for i in 0..PLACEMENTS {
        let fen = if i % 2 == 0 {
            naive::random_placement_fen(&mut rng)
        } else {
            touching += 1;
            naive::random_touching_kings_fen(&mut rng)
        };
        let mut board = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        if !board.opponent_in_check() {
            continue;
        }
        assert_same_moves(&fen, &mut board);
        compared += 1;
    }
    eprintln!("{compared} positions compared with the side not to move in check");
    assert!(
        compared >= PLACEMENTS / 2,
        "only {compared} of {PLACEMENTS} placements reached the case"
    );
    assert!(touching > 0, "the touching-kings family was never drawn");
}
