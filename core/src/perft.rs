// SPDX-License-Identifier: GPL-3.0-or-later

//! Perft: the movegen correctness gate. Pure recursion over `generate_legal`.

use crate::movegen::generate_legal;
use crate::position::Board;
use alloc::string::String;
use alloc::vec::Vec;

/// Count leaf nodes of the legal move tree to `depth`. `perft(_, 0)` is 1 by definition.
pub fn perft(board: &mut Board, depth: u32) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = generate_legal(board);
    if depth == 1 {
        return moves.len() as u64;
    }
    let mut nodes = 0;
    for m in moves.iter() {
        board.make_move(m);
        nodes += perft(board, depth - 1);
        board.unmake_move(m);
    }
    nodes
}

/// Perft split by root move: `(king-takes-rook UCI, nodes below it)`, in generation order.
/// Empty at depth 0.
#[must_use]
pub fn perft_divide(board: &mut Board, depth: u32) -> Vec<(String, u64)> {
    if depth == 0 {
        return Vec::new();
    }
    let moves = generate_legal(board);
    let mut out = Vec::with_capacity(moves.len());
    for m in moves.iter() {
        board.make_move(m);
        let nodes = perft(board, depth - 1);
        board.unmake_move(m);
        out.push((m.to_uci_chess960(), nodes));
    }
    out
}
