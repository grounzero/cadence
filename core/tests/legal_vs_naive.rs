// SPDX-License-Identifier: GPL-3.0-or-later

//! The two share only the gated attack tables and `attackers_to`; the naive one plays each move to
//! test king safety, so a disagreement names the move and lies in the fast generator's legality
//! reasoning. Duplicates are caught separately: a doubled move has the right set and the wrong
//! count.

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
    // The same flag bits, not just the spellings.
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
    // Unreachable positions `from_fen` accepts, where the two once differed. The second family has
    // the kings adjacent, where the enemy king attacks ours as well as standing on a square we can
    // move to.
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
