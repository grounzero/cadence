// SPDX-License-Identifier: GPL-3.0-or-later

//! `generate_noisy(b)` is `generate_legal(b)` filtered by `Move::is_noisy`, in `generate_legal`'s
//! order: a sequence, because a filtered list in that order leaves the bench count unchanged. The
//! walks favour the rare kinds and count them, so the branches are asserted reached.

mod support;

use cadence_core::position::Board;
use cadence_core::{FenStyle, Move, generate_legal, generate_noisy};
use support::generative as generate;

const WALKS: usize = 200;
const PLIES_PER_WALK: usize = 60;

/// So the coverage of the rare branches is a number and not a hope.
#[derive(Default, Debug)]
struct Seen {
    nodes: usize,
    /// The list must be empty there.
    quiet: usize,
    captures: usize,
    en_passant: usize,
    promotions: usize,
    promotion_captures: usize,
    king_captures: usize,
    /// The in-check subset is its own branch.
    in_check: usize,
    double_check: usize,
    noisy_evasions: usize,
    /// Never in the noisy list.
    castles: usize,
}

fn assert_noisy_subsequence(label: &str, board: &Board, seen: &mut Seen) {
    let legal = generate_legal(board);
    let noisy = generate_noisy(board);
    let expected: Vec<Move> = legal.iter().filter(|m| m.is_noisy()).collect();
    assert_eq!(
        noisy.as_slice(),
        expected.as_slice(),
        "{label}\n  {}\n  generate_noisy ({}) {:?}\n  legal, filtered ({}) {expected:?}",
        board.to_fen(FenStyle::Shredder),
        noisy.len(),
        noisy.as_slice(),
        expected.len(),
    );

    seen.nodes += 1;
    if expected.is_empty() {
        seen.quiet += 1;
    }
    let ksq = board.king_square(board.side_to_move());
    for m in legal.iter() {
        if m.is_en_passant() {
            seen.en_passant += 1;
        } else if m.is_promotion() && m.is_capture() {
            seen.promotion_captures += 1;
        } else if m.is_promotion() {
            seen.promotions += 1;
        } else if m.is_capture() {
            seen.captures += 1;
            if m.from_sq() == ksq {
                seen.king_captures += 1;
            }
        } else if m.is_castle() {
            seen.castles += 1;
        }
    }
    if board.in_check() {
        seen.in_check += 1;
        if board.checkers().more_than_one() {
            seen.double_check += 1;
        }
        if !expected.is_empty() {
            seen.noisy_evasions += 1;
        }
    }
}

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
    fens.extend(
        support::fen_notations()
            .into_iter()
            .flat_map(|f| [f.shredder, f.xfen]),
    );
    fens.push(support::move_capacity().fen);
    fens
}

#[test]
fn generate_noisy_is_the_noisy_subsequence_in_every_corpus_position() {
    let mut seen = Seen::default();
    for fen in corpus_fens() {
        let board = Board::from_fen(&fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert_noisy_subsequence(&fen, &board, &mut seen);
    }
    eprintln!("corpus: {seen:?}");
    // The corpus is built to reach the rare branches; say that it did.
    assert!(seen.captures > 0, "{seen:?}");
    assert!(seen.en_passant > 0, "{seen:?}");
    assert!(seen.promotions > 0, "{seen:?}");
    assert!(seen.promotion_captures > 0, "{seen:?}");
    assert!(seen.in_check > 0, "{seen:?}");
    assert!(seen.double_check > 0, "{seen:?}");
    assert!(seen.noisy_evasions > 0, "{seen:?}");
    assert!(seen.castles > 0, "{seen:?}");
    assert!(seen.quiet > 0, "{seen:?}");
}

/// Preferring the rare kinds when on offer, so en passant, promotion and castling are compared
/// often rather than by luck.
#[test]
fn generate_noisy_is_the_noisy_subsequence_along_walks() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(0x0151_0000_0000_0003);
    let mut seen = Seen::default();
    for walk in 0..WALKS {
        let fen = &seeds[walk % seeds.len()];
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        for ply in 0..PLIES_PER_WALK {
            assert_noisy_subsequence(
                &format!("walk {walk} ply {ply} from {fen}"),
                &board,
                &mut seen,
            );
            let legal = generate_legal(&board);
            if legal.is_empty() {
                break;
            }
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
    eprintln!("walks: {seen:?}");
    assert!(seen.nodes >= WALKS * 20, "walks ended early: {seen:?}");
    // Each floor names a branch this test exists to have compared.
    assert!(seen.quiet >= 200, "{seen:?}");
    assert!(seen.captures >= 5_000, "{seen:?}");
    assert!(seen.king_captures >= 50, "{seen:?}");
    assert!(seen.en_passant >= 20, "{seen:?}");
    assert!(seen.promotions >= 100, "{seen:?}");
    assert!(seen.promotion_captures >= 20, "{seen:?}");
    assert!(seen.in_check >= 200, "{seen:?}");
    // Not asserted: random play reaches double check rarely, none in 11,809 nodes at calibration,
    // and the corpus test asserts its double-check position.
    assert!(seen.noisy_evasions >= 50, "{seen:?}");
    assert!(seen.castles >= 100, "{seen:?}");
}
