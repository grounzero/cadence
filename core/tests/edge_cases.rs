// SPDX-License-Identifier: GPL-3.0-or-later

//! Corpus section 4: check, evasion, promotion and en-passant edge cases.
//!
//! These are the positions that silently corrupt perft at depth 4 and beyond
//! while shallow depths stay green. Each gets two tests (the node counts,
//! and the exact depth-1 move list) because an aggregate can be right by
//! cancellation. One missing evasion plus one illegal move generated is a
//! perfect total and a broken engine; a move list cannot lie that way.
//!
//! The named properties the corpus states in prose are asserted too, and are
//! derived from the parsed document rather than transcribed: the checking
//! moves come from the annotation's `<move> and <move> give check` clause, and
//! everything else in the same move list is then required *not* to give
//! check.
//!
//! All of section 4 is in the fast tier. The deepest value here is 15,495 nodes.

mod support;

// ---------------------------------------------------------------------------
// Node counts
// ---------------------------------------------------------------------------

macro_rules! edge_perft_tests {
    ($( $name:ident => $selector:literal; )*) => {
        const PERFT_SELECTORS: &[&str] = &[$($selector),*];
        $(
        #[test]
        fn $name() {
            let c = support::edge_case($selector);
            support::assert_perft(concat!("edge [", $selector, "]"), &c.fen, &c.nodes);
        }
        )*
    };
}

edge_perft_tests! {
    perft_king_may_not_retreat_along_the_checking_ray => "retreat ALONG the checking ray";
    perft_en_passant_vertical_pin                     => "VERTICAL pin";
    perft_en_passant_horizontal_double_vacate         => "HORIZONTAL double-vacate";
    perft_en_passant_legal_control                    => "POSITIVE CONTROL: d5xc6";
    perft_en_passant_legal_control_with_bishop        => "POSITIVE CONTROL with a bishop";
    perft_double_check                                => "DOUBLE CHECK";
    perft_underpromotion_interposition                => "UNDERPROMOTION RESOLVING CHECK";
    perft_promotion_giving_check                      => "PROMOTION GIVING CHECK";
    perft_promotion_capture_giving_check              => "PROMOTION-CAPTURE GIVING CHECK";
    perft_en_passant_as_a_check_evasion               => "EN PASSANT AS A CHECK EVASION";
    perft_two_pawns_capturing_the_same_ep_square      => "TWO PAWNS CAPTURING";
}

// ---------------------------------------------------------------------------
// Full depth-1 move lists
// ---------------------------------------------------------------------------

macro_rules! edge_move_list_tests {
    ($( $name:ident => $selector:literal; )*) => {
        const MOVE_LIST_SELECTORS: &[&str] = &[$($selector),*];
        $(
        #[test]
        fn $name() {
            let c = support::edge_case($selector);
            let expected = support::expected_moves(&c.fen);
            let label = concat!("edge moves [", $selector, "]");

            // The document states the count twice: in the TSV `d1` column
            // and in parentheses above the list. Neither is trusted over the
            // other; they are checked against each other in the corpus
            // integrity tests.
            let mut want = expected.moves.clone();
            want.sort();
            support::assert_move_list(label, &want, &support::legal_uci(label, &c.fen));
        }
        )*
    };
}

edge_move_list_tests! {
    moves_king_may_not_retreat_along_the_checking_ray => "retreat ALONG the checking ray";
    moves_en_passant_vertical_pin                     => "VERTICAL pin";
    moves_en_passant_horizontal_double_vacate         => "HORIZONTAL double-vacate";
    moves_en_passant_legal_control                    => "POSITIVE CONTROL: d5xc6";
    moves_en_passant_legal_control_with_bishop        => "POSITIVE CONTROL with a bishop";
    moves_double_check                                => "DOUBLE CHECK";
    moves_underpromotion_interposition                => "UNDERPROMOTION RESOLVING CHECK";
    moves_promotion_giving_check                      => "PROMOTION GIVING CHECK";
    moves_promotion_capture_giving_check              => "PROMOTION-CAPTURE GIVING CHECK";
    moves_en_passant_as_a_check_evasion               => "EN PASSANT AS A CHECK EVASION";
    moves_two_pawns_capturing_the_same_ep_square      => "TWO PAWNS CAPTURING";
}

// ---------------------------------------------------------------------------
// The named properties
// ---------------------------------------------------------------------------

/// The only position in the corpus that exercises the double-check branch.
///
/// Black's king is attacked by a rook and a knight at once. No capture of one
/// checker and no interposition can resolve both, so generation must restrict
/// to king moves the moment the checker count reaches two, and it must reach
/// two, which is what this asserts directly rather than inferring from the
/// move list.
#[test]
fn double_check_position_has_exactly_two_checkers() {
    let c = support::edge_case("DOUBLE CHECK");
    let label = "Section 4 checkers [DOUBLE CHECK]";
    let board = cadence_core::Board::from_fen(&c.fen)
        .unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})\n  {}", c.fen));
    assert_eq!(
        board.checkers().count(),
        2,
        "{label}: the corpus calls this a double check\n  {}\n  {}",
        c.fen,
        c.reason
    );
}

/// All four promotion pieces block the check equally well.
///
/// A generator that emits only queen promotions when in check, or that
/// computes block squares without expanding the promotion pieces, returns
/// four moves here instead of seven.
#[test]
fn underpromotion_evasion_generates_all_four_pieces() {
    let c = support::edge_case("UNDERPROMOTION RESOLVING CHECK");
    let expected = support::expected_moves(&c.fen);
    let label = "Section 4 promotions [UNDERPROMOTION]";

    let want = expected.promotions();
    assert_eq!(
        want.len(),
        4,
        "the corpus list should hold four promotions, got {want:?}"
    );

    let got: Vec<String> = support::legal_uci(label, &c.fen)
        .into_iter()
        .filter(|m| m.len() == 5)
        .collect();
    support::assert_move_list(label, &want, &got);
}

macro_rules! check_claim_tests {
    ($( $name:ident => $selector:literal; )*) => { $(
        /// Every move the corpus annotation says gives check must give check,
        /// and every *other* promotion in the same position must not.
        ///
        /// The negative half is derived, not transcribed: the annotation only
        /// names the checking moves, so the complement is computed from the
        /// parsed move list. That makes it stricter than the prose: the
        /// prose says "the quiet b7b8 promotions do not", the test says every
        /// promotion that is not named does not.
        #[test]
        fn $name() {
            let c = support::edge_case($selector);
            let expected = support::expected_moves(&c.fen);
            let label = concat!("Section 4 gives_check [", $selector, "]");

            let checking = expected.checking_moves();
            assert!(
                !checking.is_empty(),
                "{label}: the corpus annotation names no checking move, so there is \
                 nothing to assert: the annotation format has changed\n  {}",
                expected.annotation
            );

            let board = cadence_core::Board::from_fen(&c.fen)
                .unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})\n  {}", c.fen));

            for (uci, mv) in support::legal_moves(label, &c.fen) {
                let is_promotion = uci.len() == 5;
                if !is_promotion && !checking.contains(&uci) {
                    continue;
                }
                let want = checking.contains(&uci);
                assert_eq!(
                    board.gives_check(mv),
                    want,
                    "{label}: `{uci}` {} give check\n  {}\n  {}",
                    if want { "must" } else { "must not" },
                    c.fen,
                    expected.annotation
                );
            }
        }
    )* };
}

check_claim_tests! {
    promotion_check_claims_hold         => "PROMOTION GIVING CHECK";
    promotion_capture_check_claims_hold => "PROMOTION-CAPTURE GIVING CHECK";
}

/// An en-passant capture can resolve a check, and its destination is outside
/// the target set a naive evasion generator searches.
///
/// This is the case the check-count dispatch omits. The checker is a pawn that
/// has just
/// double-pushed; the capture removes it, but lands on the square *behind* it,
/// so it is neither "capture the checker" nor "interpose on BETWEEN". The
/// corpus supplies the mask explicitly so the claim is checked rather than
/// described.
#[test]
fn en_passant_capture_is_a_legal_check_evasion() {
    let cases = support::ep_evasions();
    assert_eq!(cases.len(), 1, "expected one ep-evasion row");

    for c in cases {
        let label = format!("ep evasion [{}]", c.mv);
        let board = cadence_core::Board::from_fen(&c.fen)
            .unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})\n  {}", c.fen));

        assert_eq!(
            board.checkers().count(),
            1,
            "{label}: this must be a single check\n  {}",
            c.fen
        );

        let moves = support::legal_uci(&label, &c.fen);
        assert!(
            moves.contains(&c.mv),
            "{label}: `{}` must be legal; it is the move a `checker | BETWEEN` \
             target mask cannot reach\n  {}\n  generated: {moves:?}",
            c.mv,
            c.fen
        );

        let dest = c.mv[2..4].to_string();
        assert!(
            !c.mask.contains(&dest),
            "{label}: the corpus mask {:?} contains the destination {dest}, so this \
             position no longer demonstrates anything",
            c.mask
        );
    }
}

/// Two pawns can capture the same en-passant square.
///
/// The invariant "at most one ep capture per position" is false, and an
/// implementation written against it generates one of the two.
#[test]
fn both_pawns_may_capture_the_same_ep_square() {
    let c = support::edge_case("TWO PAWNS CAPTURING");
    let label = "two ep captures";
    let expected = support::expected_moves(&c.fen);
    let moves = support::legal_uci(label, &c.fen);

    // Whichever two the corpus lists as landing on the ep square.
    let ep_square = c
        .fen
        .split_whitespace()
        .nth(3)
        .expect("FEN has an en-passant field");
    let want: Vec<&String> = expected
        .moves
        .iter()
        .filter(|m| m.len() == 4 && &m[2..4] == ep_square)
        .collect();
    assert_eq!(
        want.len(),
        2,
        "the corpus list should hold two captures onto {ep_square}"
    );

    for m in want {
        assert!(
            moves.contains(m),
            "{label}: `{m}` is missing; both pawns capture onto {ep_square}\n  {}\n  \
             generated: {moves:?}",
            c.fen
        );
    }
}

/// Both selector lists must partition the edge-case block, and must agree with
/// each other: a position with a node-count test and no move-list test is
/// exactly the gap the move-list tests exist to close.
#[test]
fn selectors_partition_the_edge_case_block() {
    let reasons: Vec<String> = support::edge_cases()
        .into_iter()
        .map(|c| c.reason)
        .collect();
    support::assert_selectors_partition("edge cases (perft)", PERFT_SELECTORS, &reasons);
    support::assert_selectors_partition("edge cases (move lists)", MOVE_LIST_SELECTORS, &reasons);
    assert_eq!(
        PERFT_SELECTORS.len(),
        MOVE_LIST_SELECTORS.len(),
        "every edge case needs both a node-count test and a move-list test"
    );
    assert_eq!(PERFT_SELECTORS.len(), reasons.len(), "one selector per row");
}
