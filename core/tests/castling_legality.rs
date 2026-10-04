// SPDX-License-Identifier: GPL-3.0-or-later

//! Start-array perft barely reaches the degenerate geometries, so each position here isolates one
//! rule, per colour, and asserts the counts to depth 3, the verdict, and the exact castle. Legality
//! lifts both king and castling rook from the occupancy; two positions differ under a naive
//! implementation, so a systematic disagreement means checking the convention before the magics.

mod support;

/// The list is also emitted as a constant, so the block's coverage can be asserted.
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

/// With the king beside its own rook, the quiet king move and the castle share a destination and
/// are both legal, so the destination cannot identify the move. King-takes-rook is injective: the
/// destination always holds a friendly rook.
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

/// Per-selector uniqueness is not enough: two selectors can resolve to one rule and leave a third
/// untested.
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
