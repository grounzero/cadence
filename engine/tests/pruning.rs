// SPDX-License-Identifier: GPL-3.0-or-later

//! Null-move pruning: the search hands the opponent the move, and where it
//! still stands at or above beta at reduced depth, the node is cut without
//! its move list being searched.
//!
//! What these gates demonstrate is that the pruning **happens**, and that
//! the one position class where handing over the move is known to be
//! misleading -- a side with nothing but pawns beside its king, where
//! zugzwang lives -- **refuses** it. Neither is "the code runs": the first
//! asserts cutoffs were taken, and the second asserts, through its own
//! counter, that the refusal was the material guard deciding and not the
//! question never coming up.
//!
//! The counters these gates read are written wherever the rule runs and
//! read on no decision path, so a depth-limited search here reads no clock
//! and the assertions are exact, not statistical.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{START_FEN, generate_legal};
use cadence_engine::search::{Limits, Search};
use support::{PAWN_ENDGAMES, table};

/// The depth the gates search to. Six: deep enough that null-window nodes
/// with the evaluation above beta are plentiful, shallow enough that no
/// pawn in the endgame positions below can promote inside the main search,
/// which is what keeps those subtrees pawn-and-king only.
const GATE_DEPTH: u32 = 6;

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The pruning happens: a middlegame search tries null moves and cuts on
/// them.
///
/// Coverage first: the search completed the depth it was asked for, so the
/// counters were read off a finished tree. Then the property, in two
/// halves that fail differently: attempts prove the conditions admit the
/// null move somewhere real, and cutoffs prove the reduced search answers
/// at or above beta somewhere, which is the entire mechanism. A rule wired
/// in but never admitted passes neither; one admitted but never cutting
/// passes only the first.
#[test]
fn a_middlegame_search_prunes_through_the_null_move() {
    for fen in [START_FEN.to_string(), support::standard_fen("kiwipete")] {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = board(&fen);
        let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
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

/// A zugzwang position refuses the null move, and for the right reason.
///
/// In a pawn-and-king position, passing reads as strength exactly where
/// the obligation to move is the losing condition, so the material guard
/// refuses the null move everywhere in these trees. The property is that
/// not one was tried. The coverage assertion is the refusal counter:
/// every other condition admitted the null move somewhere in the tree, so
/// what kept the count at zero was the guard and not a tree where the
/// question never arose. Without that counter this gate would pass on any
/// position quiet enough, guard or no guard.
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
        let mut b = b0.duplicate();
        let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
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
