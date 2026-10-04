// SPDX-License-Identifier: GPL-3.0-or-later

//! What a round trip catches is an emitter that loses information, as `KQkq` for a DFRC array with
//! rooks off the a- and h-files does. Compared on the full fingerprint, so the same text from a
//! different board does not pass.

mod support;

use cadence_core::FenStyle;
use support::generative as generate;

const STYLES: [FenStyle; 2] = [FenStyle::XFen, FenStyle::Shredder];

fn round_trip(label: &str, fen: &str) {
    let original = cadence_core::Board::from_fen(fen)
        .unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})\n  {fen}"));
    let want = generate::fingerprint(&original);

    for style in STYLES {
        let emitted = original.to_fen(style);
        let reparsed = cadence_core::Board::from_fen(&emitted).unwrap_or_else(|e| {
            panic!("{label}: {style:?} emitted `{emitted}`, which does not parse ({e:?})\n  {fen}")
        });
        assert_eq!(
            generate::fingerprint(&reparsed),
            want,
            "{label}: {style:?} round trip changed the position\n  in:  {fen}\n  out: {emitted}"
        );
        assert_eq!(
            reparsed.to_fen(style),
            emitted,
            "{label}: {style:?} is not idempotent\n  {fen}"
        );
    }

    // The two notations describe the same position, so each must parse to a
    // board the other can emit unchanged.
    let from_xfen = cadence_core::Board::from_fen(&original.to_fen(FenStyle::XFen))
        .expect("X-FEN emitted above parses");
    let from_shredder = cadence_core::Board::from_fen(&original.to_fen(FenStyle::Shredder))
        .expect("Shredder emitted above parses");
    assert_eq!(
        generate::fingerprint(&from_xfen),
        generate::fingerprint(&from_shredder),
        "{label}: the two notations disagree about the position\n  {fen}"
    );
}

#[test]
fn fen_round_trips_over_the_corpus() {
    for p in support::standard_positions() {
        round_trip(&format!("standard {}", p.name), &p.fen);
    }
    for a in support::dfrc_arrays() {
        round_trip(&format!("array {}/{}", a.wid, a.bid), &a.fen);
    }
    for c in support::castling_cases() {
        round_trip("castling case", &c.fen);
    }
    for c in support::edge_cases() {
        round_trip("edge case", &c.fen);
    }
    for f in support::fen_notations() {
        round_trip("notation (shredder)", &f.shredder);
        round_trip("notation (xfen)", &f.xfen);
    }
}

#[test]
fn fen_round_trips_over_all_960_start_arrays() {
    for (n, fen) in generate::all_960_start_fens().into_iter().enumerate() {
        round_trip(&format!("array {n}"), &fen);
    }
}
