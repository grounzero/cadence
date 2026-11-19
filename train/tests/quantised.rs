// SPDX-License-Identifier: GPL-3.0-or-later

//! The quantised checkpoint's loss, from the engine's own evaluation, against the trainer's: a net
//! read as trained passes, and one whose output perspectives are read the other way round fails.

use cadence_core::chess960::dfrc_fen;
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_nnue::net::{HEADER_LEN, PAYLOAD_LEN};
use cadence_nnue::{AccumulatorStack, HIDDEN, Network, Provenance};
use cadence_train::validate::{Example, quantised_check};

const SCALE: f32 = 400.0;

/// Positions with White to move from random games, each labelled with the net's own score, so a
/// net read as trained scores them at a loss of zero with the result left out.
fn labelled(net: &Network) -> Vec<Example> {
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
    let loss = quantised_check(&payload, &examples, 0.0, SCALE, 0.0).expect("inside the bar");
    assert!(loss < 1e-6, "{loss}");
}

#[test]
fn an_export_read_with_its_perspectives_swapped_fails_the_bar() {
    let net = Network::seeded(5, Provenance::SelfPlay);
    let examples = labelled(&net);
    let misread = swapped(&net.to_bytes()[HEADER_LEN..]);
    let refused = quantised_check(&misread, &examples, 0.0, SCALE, 0.0);
    let error = refused.expect_err("a misread export passed the bar");
    assert!(error.contains("outside"), "{error}");
}
