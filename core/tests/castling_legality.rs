// SPDX-License-Identifier: GPL-3.0-or-later

//! Corpus section 3: DFRC castling legality.
//!
//! Each position isolates one rule. Together they are the reason section 2 is not
//! enough: start-array perft under-tests castling badly at any depth, because
//! the degenerate geometries (king that does not move, rook that does not
//! move, king and rook swapping squares, rook landing on the king's origin)
//! barely occur in the first few plies of a game.
//!
//! Each test asserts three things: the node counts to depth 3, whether *any*
//! castling move is legal, and, when one is, that it is exactly the move the
//! corpus names. The last is what catches an engine that finds a castle but
//! the wrong one.
//!
//! Every rule appears twice, once per colour. Every position in this block was
//! White to move until the rank-8 mirrors were added, which meant a mirroring
//! bug passed all fifteen castling tests and reported as "DFRC start array
//! 400/55 has the wrong node count at depth 2", a true statement that names
//! the wrong subsystem.
//!
//! Tests are selected by a distinctive phrase from the row's `reason` column
//! plus the side to move, never by row index, so reordering the corpus cannot
//! silently repoint a test at a different position. A selector matching zero
//! or two rows fails loudly.
//!
//! Castling legality here is evaluated with **both the king and the castling
//! rook lifted from the occupancy**. Two of these positions are legal under a
//! naive implementation that leaves the rook on the board and illegal under
//! this convention, so a systematic disagreement with section 3 means checking the
//! convention before the magics.

mod support;

/// Each rule generates two tests, one per colour, from a single selector, and
/// the same list is emitted as a constant so that coverage of the block can be
/// asserted rather than assumed.
macro_rules! castling_tests {
    ($( $white:ident / $black:ident => $selector:literal; )*) => {
        const SELECTORS: &[&str] = &[$($selector),*];

        $(
            #[test]
            fn $white() {
                support::assert_castling_case($selector, support::Stm::White);
            }

            #[test]
            fn $black() {
                support::assert_castling_case($selector, support::Stm::Black);
            }
        )*
    };
}

castling_tests! {
    // --- the degenerate geometries -------------------------------------
    white_kingside_king_does_not_move / black_kingside_king_does_not_move
        => "g1->g1";
    white_kingside_rook_does_not_move / black_kingside_rook_does_not_move
        => "ROOK DOES NOT MOVE";
    white_kingside_pure_swap / black_kingside_pure_swap
        => "PURE SWAP";
    white_queenside_king_does_not_move / black_queenside_king_does_not_move
        => "c1->c1";
    white_queenside_rook_lands_on_king_origin / black_queenside_rook_lands_on_king_origin
        => "LANDS ON THE KING'S ORIGIN";
    white_side_is_derived_from_rook_file / black_side_is_derived_from_rook_file
        => "Rook is KINGSIDE";

    // --- what must be empty, and what must merely be unattacked --------
    white_rook_path_must_be_empty / black_rook_path_must_be_empty
        => "knight BLOCKS the rook path";
    white_rook_path_may_be_attacked / black_rook_path_may_be_attacked
        => "rook path must be EMPTY, not unattacked";
    white_square_off_the_king_path_may_be_attacked / black_square_off_the_king_path_may_be_attacked
        => "attacked and irrelevant";

    // --- the lifted-rook occupancy, which is the whole convention ------
    white_back_rank_pinned_rook_is_illegal / black_back_rank_pinned_rook_is_illegal
        => "BACK-RANK PINNED ROOK";
    white_rook_vacating_exposes_the_king_path / black_rook_vacating_exposes_the_king_path
        => "rook VACATING e1";
    white_rook_destination_attacked_once_lifted / black_rook_destination_attacked_once_lifted
        => "the rook's destination IS f1";

    // --- ordinary check rules, in Chess960 geometry --------------------
    white_castling_out_of_check_is_illegal / black_castling_out_of_check_is_illegal
        => "OUT OF check";
    white_king_path_crossing_attack_is_illegal / black_king_path_crossing_attack_is_illegal
        => "king's path b1->g1";

    // --- the notation proof --------------------------------------------
    white_ambiguity_proof_position / black_ambiguity_proof_position
        => "THE AMBIGUITY PROOF";
}

/// The positions that prove "the king moves two squares" cannot encode
/// castling in Chess960.
///
/// With the king one square from its own rook and the square between them
/// empty, the quiet king move and the castle have the same king destination
/// and are **both legal at once**, so the destination does not identify the
/// move. King-takes-rook is injective by construction: the destination always
/// holds a friendly rook.
///
/// The full move list is asserted, not just the two moves: the surrounding
/// king and rook moves are what make the position ordinary rather than
/// contrived. Both colours, because a rank-8 mirroring bug is exactly what
/// this file exists to catch.
#[test]
fn king_destination_notation_is_ambiguous() {
    let proofs = support::ambiguity_proofs();
    assert_eq!(
        proofs.len(),
        2,
        "the proof should be stated for both colours"
    );

    for (fen, expected) in proofs {
        let label = format!("ambiguity proof [{fen}]");
        let got = support::legal_uci(&label, &fen);

        let mut sorted = expected.clone();
        sorted.sort();
        support::assert_move_list(&label, &sorted, &got);

        // The quiet king move and the castle share a king destination.
        let rank = if fen.contains(" w ") { '1' } else { '8' };
        let quiet = format!("f{rank}g{rank}");
        let castle = format!("f{rank}h{rank}");
        for m in [&quiet, &castle] {
            assert!(
                got.iter().any(|g| g == m),
                "{label}: `{m}` must be legal; the whole point is that both are\n  {fen}"
            );
        }
    }
}

/// The selectors above must partition the castling block.
///
/// Every selector is separately asserted to match exactly one row per colour,
/// which sounds like it covers everything and does not: two selectors can
/// resolve to the same rule, leaving a third rule with no test, and every
/// individual assertion still passes. This is the test that the block is
/// covered rather than that each test found something.
///
/// It passes today: it is a check on the corpus and the selector list, not on
/// move generation.
#[test]
fn selectors_partition_the_castling_block() {
    let reasons: Vec<String> = support::castling_cases()
        .into_iter()
        .map(|c| c.reason)
        .collect();
    support::assert_selectors_partition("castling block", SELECTORS, &reasons);
    assert_eq!(
        SELECTORS.len() * 2,
        reasons.len(),
        "every rule should have exactly two rows, one per colour"
    );
}
