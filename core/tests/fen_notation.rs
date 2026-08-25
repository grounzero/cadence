// SPDX-License-Identifier: GPL-3.0-or-later

//! Corpus FEN notation and castling-rights removal.
//!
//! Two concerns that share a cause: the castling field is the only part of a
//! FEN whose meaning depends on where the rooks are, and DFRC is where that
//! stops being a formality.

mod support;

// ---------------------------------------------------------------------------
// X-FEN versus Shredder
// ---------------------------------------------------------------------------

macro_rules! fen_notation_tests {
    ($( $name:ident => $selector:literal; )*) => { $(
        /// Both spellings denote the same position, so both must parse to the
        /// same rights and the same node counts. A parser that reads `K` as
        /// "the h-file rook" finds nothing there and silently drops the right,
        /// which is a legal-looking position with the wrong move list.
        #[test]
        fn $name() {
            let f = support::fen_notation($selector);
            support::assert_perft(concat!("shredder [", $selector, "]"), &f.shredder, &f.nodes);
            support::assert_perft(concat!("xfen [", $selector, "]"), &f.xfen, &f.nodes);

            let label = concat!("notation agreement [", $selector, "]");
            let from_shredder = support::legal_uci(label, &f.shredder);
            let from_xfen = support::legal_uci(label, &f.xfen);
            support::assert_move_list(label, &from_shredder, &from_xfen);
        }
    )* };
}

fen_notation_tests! {
    xfen_kq_does_not_mean_the_h_file_rook => "not the h-file";
    xfen_falls_back_to_the_file_letter    => "THE FALLBACK";
}

/// Round-trip in both notations, over every position the corpus names.
///
/// Emission is where the two notations diverge in the other direction: a
/// Shredder emitter that always writes `KQkq` loses the rook files, and an
/// X-FEN emitter that never falls back writes an ambiguous field.
#[test]
fn fen_round_trips_in_both_notations() {
    for f in support::fen_notations() {
        for (style, want) in [
            (cadence_core::FenStyle::Shredder, &f.shredder),
            (cadence_core::FenStyle::XFen, &f.xfen),
        ] {
            let label = format!("round trip {style:?} [{want}]");
            let board = cadence_core::Board::from_fen(want)
                .unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})"));
            assert_eq!(&board.to_fen(style), want, "{label}");
        }
    }
}

// ---------------------------------------------------------------------------
// Castling rights removed by a capture
// ---------------------------------------------------------------------------

macro_rules! rights_capture_tests {
    ($( $name:ident => $selector:literal; )*) => { $(
        /// `update_mask[from] & update_mask[to]` is one branchless line
        /// claimed to cover king moves, rook moves, rook captures and
        /// rook-takes-rook. Perft exercises the first two constantly and the
        /// capture cases barely at all, and under DFRC the rook files are
        /// arbitrary, so the compile-time table the standard trick uses does
        /// not exist.
        ///
        /// The assertion is on the emitted FEN rather than on any internal
        /// representation, so it cannot pass by agreeing with itself.
        #[test]
        fn $name() {
            let r = support::rights_capture($selector);
            let label = concat!("rights capture [", $selector, "]");
            support::assert_perft(label, &r.fen, &r.nodes);

            let mut board = cadence_core::Board::from_fen(&r.fen)
                .unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})\n  {}", r.fen));
            assert_eq!(
                castling_field(&board.to_fen(cadence_core::FenStyle::Shredder)),
                r.before,
                "{label}: castling field before the move\n  {}",
                r.fen
            );

            let mv = support::legal_move_named(label, &r.fen, &r.mv);
            board.make_move(mv);
            assert_eq!(
                castling_field(&board.to_fen(cadence_core::FenStyle::Shredder)),
                r.after,
                "{label}: after {}\n  {}\n  {}",
                r.mv,
                r.fen,
                r.reason
            );
        }
    )* };
}

rights_capture_tests! {
    capture_removes_the_opponents_right => "OPPONENT'S RIGHT DIES";
    rook_takes_rook_removes_both_rights => "ROOK-TAKES-ROOK";
}

fn castling_field(fen: &str) -> String {
    fen.split_whitespace()
        .nth(2)
        .expect("a FEN has a castling field")
        .to_string()
}
