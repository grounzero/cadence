// SPDX-License-Identifier: GPL-3.0-or-later

//! Two Chess960 positions with the same pieces and rights bits can castle with different rooks, so
//! the key names the rook; a standard layout's rooks add nothing, which keeps every standard key.

use cadence_core::position::Board;
use cadence_core::types::File;
use cadence_core::{CastlingRights, START_FEN, Square, zobrist};

/// White may castle queenside, with the rook on a1 in the first and the rook on b1 in the second.
const WITH_A: &str = "4k3/8/8/8/8/8/8/RR2K3 w A - 0 1";
const WITH_B: &str = "4k3/8/8/8/8/8/8/RR2K3 w B - 0 1";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

#[test]
fn rights_held_by_different_rooks_are_different_keys() {
    let (a, b) = (board(WITH_A), board(WITH_B));
    assert_eq!(a.recompute_key(), a.key());
    assert_eq!(b.recompute_key(), b.key());
    assert_ne!(a.key(), b.key());
}

/// Each right's rook off its standard file is a key of its own, and on it adds nothing.
#[test]
fn a_rook_key_is_zero_on_the_standard_file_alone() {
    let mut seen = Vec::new();
    for right in 0..4 {
        let standard = if right % 2 == 0 { File::H } else { File::A };
        for file in File::ALL {
            let key = zobrist::castling_rook(right, file);
            assert_eq!(key == 0, file == standard, "right {right}, file {file:?}");
            if key != 0 {
                seen.push(key);
            }
        }
    }
    let count = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), count, "two rook keys are equal");
}

/// The pieces and the rights set alone, as every key was before the rooks were named.
#[test]
fn a_standard_layout_keys_its_rights_alone() {
    let b = board(START_FEN);
    let mut expected = zobrist::castling(CastlingRights::ALL);
    for sq in Square::all() {
        if let Some(p) = b.piece_at(sq) {
            expected ^= zobrist::piece(p, sq);
        }
    }
    assert_eq!(b.key(), expected);
}
