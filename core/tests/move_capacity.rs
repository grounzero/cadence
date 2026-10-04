// SPDX-License-Identifier: GPL-3.0-or-later

//! Nothing else nears the 218-move bound. A `u8` length wraps to zero at 256, which reads as no
//! legal moves, like a stalemate rather than a bug.

mod support;

#[test]
fn perft_at_maximum_move_count() {
    let c = support::move_capacity();
    support::assert_perft("capacity", &c.fen, &c.nodes);
}

/// All 218, by name. The count alone would be satisfied by any 218 moves.
#[test]
fn move_list_at_maximum_capacity() {
    let c = support::move_capacity();
    let expected = support::expected_moves(&c.fen);
    let label = "capacity moves";

    let mut want = expected.moves.clone();
    want.sort();
    assert_eq!(want.len(), 218, "the corpus list should hold 218 moves");

    support::assert_move_list(label, &want, &support::legal_uci(label, &c.fen));
}
