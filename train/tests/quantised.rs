// SPDX-License-Identifier: GPL-3.0-or-later

//! The quantised checkpoint's loss, from the engine's own evaluation, against the trainer's: a net
//! read as trained passes, and one whose output perspectives are read the other way round fails.

use cadence_core::chess960::dfrc_fen;
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_nnue::net::{HEADER_LEN, PAYLOAD_LEN};
use cadence_nnue::{AccumulatorStack, HIDDEN, Inputs, Network, Provenance, WIDE};
use cadence_train::validate::{Example, quantised_check};

const SCALE: f32 = 400.0;

/// Positions with White to move from random games, each labelled with the net's own score, so a
/// net read as trained scores them at a loss of zero with the result left out.
fn labelled<const H: usize>(net: &Network<H>) -> Vec<Example> {
    let mut rng = Rng::new(17);
    let mut out = Vec::new();
    while out.len() < 200 {
        let mut board = Board::from_fen(&dfrc_fen(518, 518)).expect("the start");
        for _ in 0..2 * (1 + rng.below(20)) {
            let legal = generate_legal(&board);
            let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                break;
            };
            board.play(m);
        }
        let fen = board.to_fen(cadence_core::FenStyle::XFen);
        if !fen.contains(" w ") {
            continue;
        }
        let placement = fen.split(' ').next().expect("a placement");
        let stack = AccumulatorStack::new(net, &board);
        let cp = net.evaluate(stack.current(), board.side_to_move());
        let score = i16::try_from(cp).expect("a seeded net scores inside i16");
        out.push(Example {
            fen: format!("{placement} w - - 0 1"),
            score,
            result: 1,
        });
    }
    out
}

/// The net's payload with its two perspectives' output weights exchanged, as an export read in the
/// other order would give it.
fn swapped(payload: &[u8]) -> Vec<u8> {
    let mut out = payload.to_vec();
    let first = 2 * (PAYLOAD_LEN / 2 - 2 * HIDDEN - 1);
    let half = 2 * HIDDEN;
    out[first..first + 2 * half].rotate_left(half);
    out
}

#[test]
fn a_net_read_as_trained_passes_the_bar() {
    let net = Network::seeded(5, Provenance::SelfPlay);
    let examples = labelled(&net);
    let payload = net.to_bytes()[HEADER_LEN..].to_vec();
    let loss = quantised_check(&payload, Inputs::Plain, &examples, 0.0, SCALE, 0.0)
        .expect("inside the bar");
    assert!(loss < 1e-6, "{loss}");
}

#[test]
fn an_export_read_with_its_perspectives_swapped_fails_the_bar() {
    let net = Network::seeded(5, Provenance::SelfPlay);
    let examples = labelled(&net);
    let misread = swapped(&net.to_bytes()[HEADER_LEN..]);
    let refused = quantised_check(&misread, Inputs::Plain, &examples, 0.0, SCALE, 0.0);
    let error = refused.expect_err("a misread export passed the bar");
    assert!(error.contains("outside"), "{error}");
}

/// A wide net's checkpoint is read at the width its length names, and passes the bar as trained.
#[test]
fn a_wide_net_read_as_trained_passes_the_bar() {
    let net = Network::<WIDE>::generate(5, Provenance::SelfPlay);
    let examples = labelled(&net);
    let payload = net.to_bytes()[HEADER_LEN..].to_vec();
    let loss = quantised_check(&payload, Inputs::Plain, &examples, 0.0, SCALE, 0.0)
        .expect("inside the bar");
    assert!(loss < 1e-6, "{loss}");
}

/// The seeded net's weights behind a header naming mirrored inputs.
fn mirrored() -> (Vec<u8>, Network) {
    let payload = Network::seeded(5, Provenance::SelfPlay).to_bytes()[HEADER_LEN..].to_vec();
    let net = Network::<HIDDEN>::load_payload_as(
        &payload,
        Inputs::Mirrored,
        Provenance::SelfPlay,
        &"0".repeat(40),
        [0; 32],
    )
    .expect("loads");
    (payload, net)
}

/// A checkpoint is scored under the layout its run recorded: weights trained as mirrored pass the
/// bar read as mirrored and fail it read as plain, and plain weights fail it read as mirrored.
#[test]
fn a_net_is_scored_under_the_layout_it_was_trained_with() {
    let (payload, net) = mirrored();
    let examples = labelled(&net);
    let loss = quantised_check(&payload, Inputs::Mirrored, &examples, 0.0, SCALE, 0.0)
        .expect("inside the bar");
    assert!(loss < 1e-9, "{loss}");
    let as_plain = quantised_check(&payload, Inputs::Plain, &examples, 0.0, SCALE, 0.0);
    assert!(
        as_plain.as_ref().is_err_and(|e| e.contains("outside")),
        "{as_plain:?}"
    );
    let plain = Network::seeded(5, Provenance::SelfPlay);
    let as_mirrored = quantised_check(
        &payload,
        Inputs::Mirrored,
        &labelled(&plain),
        0.0,
        SCALE,
        0.0,
    );
    assert!(
        as_mirrored.as_ref().is_err_and(|e| e.contains("outside")),
        "{as_mirrored:?}"
    );
}

/// Castling positions, Chess960's among them, each with the file White's king and then Black's
/// lands on, as the index test has them.
const CASTLES: [(&str, [u8; 2]); 9] = [
    (
        "r3k2r/pppppppp/2n5/8/8/5N2/PPPPPPPP/R3K2R w KQkq - 0 1",
        *b"gc",
    ),
    (
        "r3k2r/pppppppp/2n5/8/8/5N2/PPPPPPPP/R3K2R w KQkq - 0 1",
        *b"cg",
    ),
    ("1k5r/pppppppp/8/8/8/8/PPPPPPPP/1K5R w Hh - 0 1", *b"gg"),
    ("r4k2/pppppppp/8/8/8/8/PPPPPPPP/R4K2 w Aa - 0 1", *b"cc"),
    ("6kr/pppppppp/8/8/8/8/PPPPPPPP/6KR w Hh - 0 1", *b"gg"),
    ("r1k5/pppppppp/8/8/8/8/PPPPPPPP/R1K5 w Aa - 0 1", *b"cc"),
    ("3k3r/pppppppp/8/8/8/8/PPPPPPPP/3K3R w Hh - 0 1", *b"gg"),
    ("r2k4/pppppppp/8/8/8/8/PPPPPPPP/R2K4 w Aa - 0 1", *b"cc"),
    ("2rk4/pppppppp/8/8/8/8/PPPPPPPP/2RK4 w Cc - 0 1", *b"cc"),
];

/// Each castling position before any castle and after both sides have castled, White to move in
/// both, labelled with the net's own score.
fn castled<const H: usize>(net: &Network<H>) -> Vec<Example> {
    let mut out = Vec::new();
    for (fen, files) in CASTLES {
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        for step in 0..2 {
            if step == 1 {
                for file in files {
                    let us = board.side_to_move();
                    let castles: Vec<_> = generate_legal(&board)
                        .as_slice()
                        .iter()
                        .copied()
                        .filter(|m| m.is_castle())
                        .collect();
                    let found = castles.into_iter().find(|&m| {
                        board.make_move(m);
                        let lands = board.king_square(us).file().index();
                        board.unmake_move(m);
                        lands == usize::from(file - b'a')
                    });
                    board.make_move(found.unwrap_or_else(|| panic!("{fen}: no castle")));
                }
            }
            let placement = board.to_fen(cadence_core::FenStyle::XFen);
            let placement = placement
                .split(' ')
                .next()
                .expect("a placement")
                .to_string();
            let stack = AccumulatorStack::new(net, &board);
            let cp = net.evaluate(stack.current(), board.side_to_move());
            out.push(Example {
                fen: format!("{placement} w - - 0 1"),
                score: i16::try_from(cp).expect("a seeded net scores inside i16"),
                result: 1,
            });
        }
    }
    out
}

/// The same on castling positions, Chess960's among them, where a king has crossed the middle,
/// stayed on its half, stayed on its square or traded squares with its rook.
#[test]
fn a_mirrored_net_is_scored_as_trained_on_castling_positions() {
    let (payload, net) = mirrored();
    let examples = castled(&net);
    assert_eq!(examples.len(), 18);
    let loss = quantised_check(&payload, Inputs::Mirrored, &examples, 0.0, SCALE, 0.0)
        .expect("inside the bar");
    assert!(loss < 1e-9, "{loss}");
    let as_plain = quantised_check(&payload, Inputs::Plain, &examples, 0.0, SCALE, 0.0);
    assert!(
        as_plain.as_ref().is_err_and(|e| e.contains("outside")),
        "{as_plain:?}"
    );
}
