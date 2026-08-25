// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `zobrist`'s tables. The rule about *when* each key is mixed
//! in is gated with `position`, where the incremental key is compared with a
//! from-scratch recomputation at every node of a walk; what is checked here
//! is that the keys themselves can carry that test.
//!
//! 793 keys (768 piece-square, 1 side, 16 castling, 8 en-passant), all
//! non-zero and pairwise distinct. A zero key is a piece that does not hash;
//! two equal keys are two positions that collide by construction. Neither
//! is caught by anything downstream: perft does not hash, and a
//! transposition table returns a wrong entry silently.

use cadence_core::castling::CastlingRights;
use cadence_core::types::{File, Piece, Square};
use cadence_core::zobrist;

fn all_keys() -> Vec<(String, u64)> {
    let mut out = Vec::with_capacity(793);
    for p in Piece::ALL {
        for sq in Square::all() {
            out.push((format!("{p:?} on {sq}"), zobrist::piece(p, sq)));
        }
    }
    out.push(("side".to_string(), zobrist::side()));
    for bits in 0..16u8 {
        out.push((
            format!("castling {bits:04b}"),
            zobrist::castling(CastlingRights::from_bits(bits)),
        ));
    }
    for f in File::ALL {
        out.push((format!("ep {}", f.to_char()), zobrist::ep(f)));
    }
    assert_eq!(out.len(), 768 + 1 + 16 + 8);
    out
}

#[test]
fn every_key_is_non_zero() {
    for (name, key) in all_keys() {
        assert_ne!(key, 0, "{name} has a zero key");
    }
}

#[test]
fn every_key_is_distinct() {
    let keys = all_keys();
    let mut seen = std::collections::HashMap::new();
    for (name, key) in &keys {
        if let Some(other) = seen.insert(*key, name.clone()) {
            panic!("{name} and {other} share the key 0x{key:016x}");
        }
    }
    assert_eq!(seen.len(), keys.len());
}

/// The keys are a function of the crate, not of the run: the same call
/// returns the same key, and the piece table is indexed by piece then square.
#[test]
fn keys_are_stable_and_typed() {
    assert_eq!(
        zobrist::piece(Piece::WPawn, Square::E4),
        zobrist::piece(Piece::WPawn, Square::E4)
    );
    assert_ne!(
        zobrist::piece(Piece::WPawn, Square::E4),
        zobrist::piece(Piece::BPawn, Square::E4)
    );
    assert_ne!(
        zobrist::piece(Piece::WPawn, Square::E4),
        zobrist::piece(Piece::WPawn, Square::E5)
    );
    assert_eq!(zobrist::side(), zobrist::side());
    assert_eq!(
        zobrist::castling(CastlingRights::ALL),
        zobrist::castling(CastlingRights::from_bits(0b1111))
    );
    assert_ne!(
        zobrist::castling(CastlingRights::NONE),
        zobrist::castling(CastlingRights::ALL)
    );
    assert_ne!(zobrist::ep(File::A), zobrist::ep(File::H));
    // Spread: no key is a small number, and the high halves vary. A table
    // built from a broken generator (all keys equal to the seed, or a
    // counter) fails here.
    let keys = all_keys();
    let high_halves: std::collections::HashSet<u32> =
        keys.iter().map(|(_, k)| (*k >> 32) as u32).collect();
    assert!(
        high_halves.len() > keys.len() / 2,
        "high halves barely vary"
    );
    assert!(
        keys.iter().all(|(_, k)| *k > 1 << 32),
        "a key with an empty high half"
    );
}
