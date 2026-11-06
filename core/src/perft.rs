// SPDX-License-Identifier: GPL-3.0-or-later

use crate::movegen::generate_legal;
use crate::position::Board;
use alloc::string::String;
use alloc::vec::Vec;

/// # Panics
///
/// If `depth` past the board's ply is more than `MAX_PLY`, before any move is made: the board holds
/// no more states than that.
pub fn perft(board: &mut Board, depth: u32) -> u64 {
    within_reach(board, depth);
    count(board, depth)
}

fn count(board: &mut Board, depth: u32) -> u64 {
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
        nodes += count(board, depth - 1);
        board.unmake_move(m);
    }
    nodes
}

/// King-takes-rook spellings, in generation order.
///
/// # Panics
///
/// If `depth` past the board's ply is more than `MAX_PLY`, before any move is made: the board holds
/// no more states than that.
#[must_use]
pub fn perft_divide(board: &mut Board, depth: u32) -> Vec<(String, u64)> {
    within_reach(board, depth);
    if depth == 0 {
        return Vec::new();
    }
    let moves = generate_legal(board);
    let mut out = Vec::with_capacity(moves.len());
    for m in moves.iter() {
        board.make_move(m);
        let nodes = count(board, depth - 1);
        board.unmake_move(m);
        out.push((m.to_uci_chess960(), nodes));
    }
    out
}

fn within_reach(board: &Board, depth: u32) {
    let deepest = usize::try_from(depth)
        .ok()
        .and_then(|d| d.checked_add(board.ply()));
    assert!(
        deepest.is_some_and(|d| d <= crate::MAX_PLY),
        "perft depth {depth} at ply {} is deeper than the board holds",
        board.ply()
    );
}
