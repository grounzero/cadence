// SPDX-License-Identifier: GPL-3.0-or-later

//! Corpus move-list capacity.
//!
//! Nothing else in the corpus comes near the 218-move bound, so `MAX_MOVES`
//! and the width of `MoveList`'s length field are otherwise untested, the
//! `u8`-to-`u16` correction included. A `u8` length wraps to zero at a
//! capacity of 256, so the failure is not a truncated list but **no legal
//! moves at all**, which reads like a stalemate rather than a bug.

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
