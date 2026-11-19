// SPDX-License-Identifier: GPL-3.0-or-later

//! The quantised checkpoint's loss, from the engine's own evaluation, against the trainer's: a net
//! read as trained passes, and one whose output perspectives are read the other way round fails.

use cadence_core::chess960::dfrc_fen;
use cadence_core::features::NUM_INPUTS;
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_nnue::net::{HEADER_LEN, PAYLOAD_LEN};
use cadence_nnue::{AccumulatorStack, HIDDEN, Network, OUTPUT_BUCKETS, Provenance, WIDE};
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
        let pieces = board.occupied().count();
        let cp = net.evaluate(stack.current(), board.side_to_move(), pieces);
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
    let loss = quantised_check(&payload, 1, &examples, 0.0, SCALE, 0.0).expect("inside the bar");
    assert!(loss < 1e-6, "{loss}");
}

#[test]
fn an_export_read_with_its_perspectives_swapped_fails_the_bar() {
    let net = Network::seeded(5, Provenance::SelfPlay);
    let examples = labelled(&net);
    let misread = swapped(&net.to_bytes()[HEADER_LEN..]);
    let refused = quantised_check(&misread, 1, &examples, 0.0, SCALE, 0.0);
    let error = refused.expect_err("a misread export passed the bar");
    assert!(error.contains("outside"), "{error}");
}

/// A wide net's checkpoint is read at the width its length names, and passes the bar as trained.
#[test]
fn a_wide_net_read_as_trained_passes_the_bar() {
    let net = Network::<WIDE>::generate(5, Provenance::SelfPlay);
    let examples = labelled(&net);
    let payload = net.to_bytes()[HEADER_LEN..].to_vec();
    let loss = quantised_check(&payload, 1, &examples, 0.0, SCALE, 0.0).expect("inside the bar");
    assert!(loss < 1e-6, "{loss}");
}

/// A bucketed payload with its output layer an input at a time, as the trainer holds it before the
/// recipe's transpose.
fn untransposed(payload: &[u8], hidden: usize) -> Vec<u8> {
    let mut out = payload.to_vec();
    let first = 2 * (NUM_INPUTS * hidden + hidden);
    let row = 2 * hidden;
    for bucket in 0..OUTPUT_BUCKETS {
        for input in 0..row {
            let from = first + 2 * (bucket * row + input);
            let to = first + 2 * (input * OUTPUT_BUCKETS + bucket);
            out[to..to + 2].copy_from_slice(&payload[from..from + 2]);
        }
    }
    out
}

/// A bucketed checkpoint at either width is read at the run's bucket count and passes the bar as
/// trained, and the same weights left untransposed fail it.
#[test]
fn a_bucketed_net_read_as_trained_passes_the_bar_and_untransposed_fails_it() {
    let narrow = Network::<HIDDEN>::generate_bucketed(5, Provenance::SelfPlay);
    let wide = Network::<WIDE>::generate_bucketed(5, Provenance::SelfPlay);
    for (examples, payload, hidden) in [
        (
            labelled(&narrow),
            narrow.to_bytes()[HEADER_LEN..].to_vec(),
            HIDDEN,
        ),
        (
            labelled(&wide),
            wide.to_bytes()[HEADER_LEN..].to_vec(),
            WIDE,
        ),
    ] {
        let loss = quantised_check(&payload, OUTPUT_BUCKETS, &examples, 0.0, SCALE, 0.0)
            .expect("inside the bar");
        assert!(loss < 1e-6, "{loss}");
        let misread = untransposed(&payload, hidden);
        let refused = quantised_check(&misread, OUTPUT_BUCKETS, &examples, 0.0, SCALE, 0.0);
        let error = refused.expect_err("an untransposed export passed the bar");
        assert!(error.contains("outside"), "{error}");
        let one = quantised_check(&payload, 1, &examples, 0.0, SCALE, 0.0);
        assert!(
            one.is_err(),
            "a bucketed checkpoint read as one bucket passed"
        );
    }
}
