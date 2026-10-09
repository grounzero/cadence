// SPDX-License-Identifier: GPL-3.0-or-later

//! `has_legal_move(b)` is `!generate_legal(b).is_empty()` wherever it is asked. The walks count
//! the positions whose only moves come late in the generator, which the early exit must not skip.

mod support;

use cadence_core::position::Board;
use cadence_core::{FenStyle, PieceType, generate_legal, has_legal_move};
use support::generative as generate;

const DEPTH: usize = 2;
const WALKS: usize = 200;
const PLIES_PER_WALK: usize = 60;

#[derive(Default, Debug)]
struct Seen {
    nodes: usize,
    no_move: usize,
    /// Every move is a pawn's, the generator's last block.
    pawn_only: usize,
    /// No king move, so the answer comes from a later block.
    no_king_move: usize,
}

fn assert_agrees(label: &str, board: &Board, seen: &mut Seen) {
    let legal = generate_legal(board);
    assert_eq!(
        has_legal_move(board),
        !legal.is_empty(),
        "{label}\n  {}\n  legal ({}) {:?}",
        board.to_fen(FenStyle::Shredder),
        legal.len(),
        legal.as_slice(),
    );
    seen.nodes += 1;
    let us = board.side_to_move();
    let pawns = board.pieces(us, PieceType::Pawn);
    let ksq = board.king_square(us);
    if legal.is_empty() {
        seen.no_move += 1;
    } else {
        if legal.iter().all(|m| pawns.contains(m.from_sq())) {
            seen.pawn_only += 1;
        }
        if legal.iter().all(|m| m.from_sq() != ksq || m.is_castle()) {
            seen.no_king_move += 1;
        }
    }
}

fn walk_children(label: &str, board: &mut Board, depth: usize, seen: &mut Seen) {
    assert_agrees(label, board, seen);
    if depth == 0 {
        return;
    }
    for m in generate_legal(board).iter() {
        board.make_move(m);
        walk_children(label, board, depth - 1, seen);
        board.unmake_move(m);
    }
}

#[test]
fn has_legal_move_agrees_with_the_generator_on_the_corpus_and_its_children() {
    let mut fens: Vec<String> = generate::walk_seeds();
    fens.extend(support::castling_cases().into_iter().map(|c| c.fen));
    fens.extend(support::edge_cases().into_iter().map(|c| c.fen));
    fens.extend(support::rights_captures().into_iter().map(|r| r.fen));
    fens.extend(support::immediate_castles().into_iter().map(|r| r.fen));
    fens.push(support::move_capacity().fen);
    let mut seen = Seen::default();
    for fen in &fens {
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        walk_children(fen, &mut board, DEPTH, &mut seen);
    }
    eprintln!("corpus: {seen:?}");
    assert!(seen.no_move > 0, "{seen:?}");
    assert!(seen.pawn_only > 0, "{seen:?}");
    assert!(seen.no_king_move > 0, "{seen:?}");
}

#[test]
fn has_legal_move_agrees_on_the_terminal_and_late_block_fixtures() {
    let cases = [
        ("7k/8/6Q1/8/8/8/8/7K b - - 0 1", false, "stalemated"),
        ("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1", false, "mated"),
        ("k7/P7/1K6/8/7p/8/8/8 b - - 0 1", true, "only a pawn push"),
        ("k7/P7/1K6/8/8/8/7p/8 b - - 0 1", true, "only promotions"),
    ];
    let mut seen = Seen::default();
    for (fen, expected, label) in cases {
        let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        assert_eq!(has_legal_move(&board), expected, "{label}: {fen}");
        assert_agrees(label, &board, &mut seen);
    }
    assert_eq!(seen.pawn_only, 2, "{seen:?}");
}

#[test]
fn has_legal_move_agrees_along_walks() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(0x0151_0000_0000_0004);
    let mut seen = Seen::default();
    for walk in 0..WALKS {
        let fen = &seeds[walk % seeds.len()];
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        for ply in 0..PLIES_PER_WALK {
            assert_agrees(
                &format!("walk {walk} ply {ply} from {fen}"),
                &board,
                &mut seen,
            );
            let legal = generate_legal(&board);
            if legal.is_empty() {
                break;
            }
            board.make_move(legal.as_slice()[rng.below(legal.len())]);
        }
    }
    eprintln!("walks: {seen:?}");
    assert!(seen.nodes >= WALKS * 20, "walks ended early: {seen:?}");
    assert!(seen.no_king_move >= 100, "{seen:?}");
}
