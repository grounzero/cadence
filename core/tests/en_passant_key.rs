// SPDX-License-Identifier: GPL-3.0-or-later

//! The rules count two positions as one unless a capture en passant is legal in one of them, so the
//! key carries the square exactly where one is. A pinned capturer is what a pawn's attack misreads.

use cadence_core::position::Board;
use cadence_core::{FenStyle, START_FEN, generate_legal, parse_uci};

/// White's b-pawn attacks c6, but taking there opens the fifth rank from h5 to the king on a5.
const PINNED: &str = "7k/8/8/KPp4r/8/8/8/8 w - c6 0 2";

/// The same with the king on a4, where the capture is legal.
const FREE: &str = "7k/8/8/1Pp4r/K7/8/8/8 w - c6 0 2";

/// One move before `PINNED`, Black to play c7c5.
const BEFORE: &str = "7k/2p5/8/KP5r/8/8/8/8 b - - 0 1";

/// Pawns pinned along a rank and a file, where en passant captures are often illegal.
const PINS: &str = "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// `fen` with no en passant square.
fn without_square(fen: &str) -> String {
    let mut fields: Vec<&str> = fen.split_whitespace().collect();
    fields[3] = "-";
    fields.join(" ")
}

fn make(board: &mut Board, uci: &str) {
    let legal = generate_legal(board);
    let m = parse_uci(&legal, uci).expect("a legal move");
    board.make_move(m);
}

#[test]
fn a_pinned_capture_leaves_the_key_as_without_the_square() {
    assert_eq!(board(PINNED).key(), board(&without_square(PINNED)).key());
}

#[test]
fn a_legal_capture_puts_the_square_in_the_key() {
    assert_ne!(board(FREE).key(), board(&without_square(FREE)).key());
}

/// The double push hashes as the position it reaches does, and taking it back restores the key.
#[test]
fn the_double_push_hashes_as_the_position_it_reaches() {
    let mut b = board(BEFORE);
    let before = b.key();
    let legal = generate_legal(&b);
    let push = parse_uci(&legal, "c7c5").expect("c7c5 is legal");
    b.make_move(push);
    assert_eq!(b.key(), b.recompute_key());
    assert_eq!(b.key(), board(&without_square(PINNED)).key());
    b.unmake_move(push);
    assert_eq!(b.key(), before);
}

/// The kings walk away and back after the pinned double push: the capture was never a possible
/// move, so the rules count the return as the same position.
#[test]
fn the_return_after_a_pinned_double_push_is_a_repetition() {
    let mut b = board(BEFORE);
    let legal = generate_legal(&b);
    let push = parse_uci(&legal, "c7c5").expect("c7c5 is legal");
    b.play(push);
    for uci in ["a5a6", "h8g8", "a6a5", "g8h8"] {
        make(&mut b, uci);
    }
    assert!(b.is_repetition());
}

/// Along seeded random games, the key carries the square exactly where the generator offers a
/// capture there.
#[test]
fn the_key_carries_the_square_exactly_where_a_capture_is_legal() {
    let mut state = 0x5EED_0127_u64;
    let mut takes_seen = 0;
    for start in [START_FEN, PINS, PINNED, FREE] {
        for _ in 0..200 {
            let mut b = board(start);
            for _ in 0..60 {
                let legal = generate_legal(&b);
                if legal.is_empty() {
                    break;
                }
                if b.ep_square().is_some() {
                    let fen = b.to_fen(FenStyle::Shredder);
                    let takes = legal.iter().any(|m| m.is_en_passant());
                    let differs = b.key() != board(&without_square(&fen)).key();
                    assert_eq!(differs, takes, "{fen}");
                    takes_seen += usize::from(takes);
                }
                state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = state;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                let m = legal.as_slice()[(z % legal.len() as u64) as usize];
                b.make_move(m);
            }
        }
    }
    assert!(takes_seen > 0, "no legal capture en passant was reached");
}
