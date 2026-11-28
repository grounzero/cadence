// SPDX-License-Identifier: GPL-3.0-or-later

//! `is_legal(b, m)` is `generate_legal(b).contains(m)`: for every encoding at the fixtures, where
//! the flags decide, and along walks for every move legal anywhere on them, as a table's move is.

mod support;

use std::collections::BTreeSet;

use cadence_core::position::Board;
use cadence_core::{Move, generate_legal, is_legal};
use support::generative as generate;

const WALKS: usize = 40;
const PLIES_PER_WALK: usize = 40;

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The count of `moves` the generator yields here.
fn assert_agrees(label: &str, board: &Board, moves: impl IntoIterator<Item = Move>) -> usize {
    let legal = generate_legal(board);
    let mut accepted = 0;
    for m in moves {
        let expected = legal.contains(m);
        assert_eq!(is_legal(board, m), expected, "{label}: {m:?} in {board:?}");
        accepted += usize::from(expected);
    }
    accepted
}

#[test]
fn every_encoding_agrees_with_the_generator_at_the_fixtures() {
    let mut fens: Vec<String> = support::castling_cases()
        .into_iter()
        .map(|c| c.fen)
        .collect();
    fens.extend(support::edge_cases().into_iter().map(|c| c.fen));
    fens.extend(support::rights_captures().into_iter().map(|r| r.fen));
    fens.extend(support::immediate_castles().into_iter().map(|r| r.fen));
    fens.push(support::move_capacity().fen);
    let (mut castles, mut en_passant, mut evasions) = (0, 0, 0);
    for fen in &fens {
        let b = board(fen);
        assert_agrees(fen, &b, (0..=u16::MAX).map(Move::from_bits));
        let legal = generate_legal(&b);
        castles += legal.iter().filter(|m| m.is_castle()).count();
        en_passant += legal.iter().filter(|m| m.is_en_passant()).count();
        evasions += usize::from(b.in_check());
    }
    assert!(
        castles > 0 && en_passant > 0 && evasions > 0,
        "the fixtures reach {castles} castles, {en_passant} en passant moves, {evasions} checks"
    );
}

#[test]
fn moves_legal_elsewhere_agree_with_the_generator_along_walks() {
    let seeds = generate::walk_seeds();
    let mut rng = generate::Rng::new(0x0151_0000_0000_0036);
    let mut positions = Vec::new();
    let mut pool = BTreeSet::new();
    for walk in 0..WALKS {
        let fen = &seeds[walk % seeds.len()];
        let mut b = board(fen);
        for ply in 0..PLIES_PER_WALK {
            let legal = generate_legal(&b);
            pool.extend(legal.iter().map(Move::to_bits));
            positions.push((format!("walk {walk} ply {ply} from {fen}"), b.duplicate()));
            if legal.is_empty() {
                break;
            }
            b.make_move(legal.as_slice()[rng.below(legal.len())]);
        }
    }
    let pool: Vec<Move> = pool.into_iter().map(Move::from_bits).collect();
    let mut evasions = 0;
    for (label, b) in &positions {
        let accepted = assert_agrees(label, b, pool.iter().copied());
        assert_eq!(
            accepted,
            generate_legal(b).len(),
            "{label}: the pool lacks a legal move"
        );
        evasions += usize::from(b.in_check());
    }
    assert!(
        positions.len() >= WALKS * 20 && evasions > 0,
        "{} positions, {evasions} in check",
        positions.len()
    );
}
