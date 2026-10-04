// SPDX-License-Identifier: GPL-3.0-or-later

//! The pruning happens, and a pawn-and-king side, where zugzwang lives, refuses it through a
//! counter that sees the material guard decide. The counters are on no decision path, so a
//! depth-limited search reads no clock and the assertions are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{START_FEN, generate_legal};
use cadence_engine::position::Position;
use cadence_engine::search::Limits;
use support::{PAWN_ENDGAMES, table};

/// Deep enough for plentiful null-window nodes above beta, shallow enough that no endgame pawn
/// promotes inside the main search.
const GATE_DEPTH: u32 = 6;

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// Attempts prove the conditions admit the null move somewhere real; cutoffs prove the reduced
/// search answers at or above beta. A rule never admitted passes neither, one never cutting only
/// the first.
#[test]
fn a_middlegame_search_prunes_through_the_null_move() {
    for fen in [START_FEN.to_string(), support::standard_fen("kiwipete")] {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = support::position(&fen);
        let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
        let best = s.run(&mut b, &mut Vec::new());
        assert!(!best.is_null(), "{fen}: no move");
        assert_eq!(
            s.completed_depth(),
            GATE_DEPTH,
            "{fen}: the search did not complete depth {GATE_DEPTH}"
        );
        assert!(
            s.null_attempts() > 0,
            "{fen}: depth {GATE_DEPTH} searched {} nodes and never tried a null move",
            s.nodes()
        );
        assert!(
            s.null_cutoffs() > 0,
            "{fen}: {} null moves tried and not one cut",
            s.null_attempts()
        );
    }
}

/// Not one null move tried, and the refusal counter shows every other condition admitted it, so the
/// guard decided. Without that counter any quiet enough position passes.
#[test]
fn a_pawn_endgame_refuses_the_null_move() {
    for fen in PAWN_ENDGAMES {
        let b0 = board(fen);
        // The premise, asserted rather than trusted to the FEN string:
        // pawns and kings only, and the side to move has legal moves.
        for c in [cadence_core::Colour::White, cadence_core::Colour::Black] {
            let heavy = b0.by_colour(c)
                & !(b0.by_type(cadence_core::PieceType::Pawn)
                    | b0.by_type(cadence_core::PieceType::King));
            assert!(heavy.is_empty(), "{fen}: {c:?} holds more than pawns");
        }
        assert!(!generate_legal(&b0).is_empty(), "{fen}: no legal moves");

        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = Position::new(b0.duplicate());
        let mut s = support::search(Limits::depth(GATE_DEPTH), &stop, &tt);
        let _ = s.run(&mut b, &mut Vec::new());
        assert_eq!(
            s.completed_depth(),
            GATE_DEPTH,
            "{fen}: the search did not complete depth {GATE_DEPTH}"
        );
        assert_eq!(
            s.null_attempts(),
            0,
            "{fen}: a pawn endgame tried the null move"
        );
        assert!(
            s.null_refused_by_material() > 0,
            "{fen}: the material guard never fired, so this tree presented \
             no case and the zero above covers nothing"
        );
    }
}
