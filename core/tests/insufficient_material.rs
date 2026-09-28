// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `Board::is_insufficient_material`: a table each way, every row also read with the
//! colours mirrored. The mirror moves every square to the other colour, so it exercises both
//! halves of the one-square-colour test.

use cadence_core::position::Board;

/// No sequence of legal moves mates in any of these.
const DEAD: &[&str] = &[
    "8/8/8/4k3/8/8/8/4K3 w - - 0 1",
    "8/8/8/4k3/8/8/8/2N1K3 w - - 0 1",
    "8/8/8/4k3/8/8/8/2B1K3 b - - 0 1",
    "8/8/2n5/4k3/8/8/8/4K3 w - - 0 1",
    "8/8/2b5/4k3/8/8/8/4K3 w - - 0 1",
    // Bishops on one colour, any number and either side: a1, c3, e5 and h8 are all dark.
    "7b/8/8/4B3/8/2B5/8/B3K2k w - - 0 1",
    "8/8/8/4b3/8/8/8/B3K2k w - - 0 1",
    // The same on light squares: b1, d3 and g8.
    "6b1/8/8/8/8/3B4/8/1B2K2k b - - 0 1",
];

/// A mate exists in every one of these, whether or not either side can force it.
const LIVE: &[&str] = &[
    "8/8/8/4k3/8/8/4P3/4K3 w - - 0 1",
    "8/8/8/4k3/8/8/8/R3K3 w - - 0 1",
    "8/8/8/4k3/8/8/8/Q3K3 w - - 0 1",
    // KN v KN and KB v KN: a helpmate exists in each.
    "8/8/2n5/4k3/8/8/8/2N1K3 w - - 0 1",
    "8/8/2n5/4k3/8/8/8/2B1K3 w - - 0 1",
    // Two knights, which cannot force mate and can still give it.
    "8/8/8/4k3/8/8/8/1NN1K3 w - - 0 1",
    // Bishops on both colours, one side and split across the two: a1 is dark and b1 light.
    "8/8/8/4k3/8/8/8/BB2K3 w - - 0 1",
    "8/8/8/4k3/8/8/8/Bb2K3 w - - 0 1",
    // One colour of bishop but a knight beside it, and one bishop each on opposite colours.
    "8/8/8/4k3/8/8/8/B1N1K3 w - - 0 1",
    "8/8/8/1b2k3/8/8/8/B3K3 w - - 0 1",
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
];

/// Colours swapped and ranks reversed, side to move flipped; no row here has castling or en
/// passant to carry.
fn mirror(fen: &str) -> String {
    let mut fields = fen.split(' ');
    let placement: Vec<String> = fields
        .next()
        .expect("placement")
        .split('/')
        .rev()
        .map(|rank| {
            rank.chars()
                .map(|c| {
                    if c.is_ascii_uppercase() {
                        c.to_ascii_lowercase()
                    } else {
                        c.to_ascii_uppercase()
                    }
                })
                .collect()
        })
        .collect();
    let side = if fields.next() == Some("w") { "b" } else { "w" };
    format!("{} {side} - - 0 1", placement.join("/"))
}

fn read(fen: &str) -> bool {
    Board::from_fen(fen)
        .unwrap_or_else(|e| panic!("{fen}: {e:?}"))
        .is_insufficient_material()
}

#[test]
fn the_dead_positions_are_recognised_both_ways_round() {
    for fen in DEAD {
        assert!(read(fen), "{fen}");
        assert!(read(&mirror(fen)), "mirrored {fen}");
    }
}

#[test]
fn a_position_with_a_mate_in_it_is_not() {
    for fen in LIVE {
        assert!(!read(fen), "{fen}");
        assert!(!read(&mirror(fen)), "mirrored {fen}");
    }
}
