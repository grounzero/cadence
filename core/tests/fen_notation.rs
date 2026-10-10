// SPDX-License-Identifier: GPL-3.0-or-later

//! The castling field is the only part of a FEN whose meaning depends on where the rooks are.

mod support;

// ---------------------------------------------------------------------------
// X-FEN versus Shredder
// ---------------------------------------------------------------------------

macro_rules! fen_notation_tests {
    ($( $name:ident => $selector:literal; )*) => { $(
        /// A parser reading `K` as the h-file rook drops the right silently: a legal-looking
        /// position with the wrong move list.
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

/// A Shredder emitter that always writes `KQkq` loses the rook files; an X-FEN emitter that never
/// falls back writes an ambiguous field.
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
        /// Perft barely exercises the capture cases of the update mask, and under DFRC no
        /// compile-time table exists. Asserted on the emitted FEN, so it cannot pass by agreeing
        /// with itself.
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

/// `{:?}` is the Shredder spelling, so a board in a failing assertion names itself unambiguously.
#[test]
fn a_board_debugs_as_its_shredder_fen() {
    let b = cadence_core::position::Board::from_fen(cadence_core::START_FEN)
        .expect("the start position");
    assert_eq!(
        format!("{b:?}"),
        format!("Board({})", b.to_fen(cadence_core::FenStyle::Shredder))
    );
}
