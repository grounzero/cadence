// SPDX-License-Identifier: GPL-3.0-or-later

//! Zobrist keys: const splitmix64 tables and typed key operations.
//!
//! What is in the hash, and when:
//!
//! | Table            | Mixed in when                                    |
//! |------------------|--------------------------------------------------|
//! | piece-square     | a piece occupies a square                        |
//! | side to move     | Black to move                                    |
//! | castling rights  | always, indexed by the whole 4-bit set           |
//! | en-passant file  | **only when an en-passant capture is available** |
//!
//! The ep condition is the one that is invisible to perft and surfaces later
//! as a merely-poor transposition-table hit rate. It lives here as a named
//! function so the incremental update and the from-scratch recomputation read
//! the same rule.
//!
//! The tables are const-evaluated from a fixed seed by splitmix64, so the keys
//! are a function of the crate, not of the process. There is no `rand`
//! dependency and no runtime initialisation.

use crate::castling::CastlingRights;
use crate::types::{File, Piece, Square};

/// The generator state after each draw. splitmix64: twelve lines, and its
/// output is a bijection of a counter, so distinct draws are distinct until
/// the counter wraps at 2^64.
const fn splitmix64(state: u64) -> (u64, u64) {
    let state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31), state)
}

/// Fixed for the life of the crate. Changing it changes every key, which is
/// harmless to the engine and fatal to any stored table or datagen record
/// that carried keys. There are none yet, which is the time to pick it.
const SEED: u64 = 0x00CA_DE7C_E5EE_D002;

/// The four tables, drawn in one sequence: piece-square, side, castling, ep.
struct Tables {
    piece: [[u64; 64]; 12],
    side: u64,
    castling: [u64; 16],
    ep: [u64; 8],
}

const fn build() -> Tables {
    let mut state = SEED;
    let mut piece = [[0u64; 64]; 12];
    let mut p = 0;
    while p < 12 {
        let mut sq = 0;
        while sq < 64 {
            let (k, s) = splitmix64(state);
            piece[p][sq] = k;
            state = s;
            sq += 1;
        }
        p += 1;
    }
    let (side, s) = splitmix64(state);
    state = s;
    let mut castling = [0u64; 16];
    let mut i = 0;
    while i < 16 {
        let (k, s) = splitmix64(state);
        castling[i] = k;
        state = s;
        i += 1;
    }
    let mut ep = [0u64; 8];
    let mut i = 0;
    while i < 8 {
        let (k, s) = splitmix64(state);
        ep[i] = k;
        state = s;
        i += 1;
    }
    Tables {
        piece,
        side,
        castling,
        ep,
    }
}

static TABLES: Tables = build();

/// The key for `piece` standing on `sq`.
#[inline]
#[must_use]
pub fn piece(piece: Piece, sq: Square) -> u64 {
    TABLES.piece[piece.index()][sq.index()]
}

/// Mixed in when Black is to move.
#[inline]
#[must_use]
pub fn side() -> u64 {
    TABLES.side
}

/// The key for the whole rights set. Losing a right is one XOR of the old
/// index's key and the new one's.
#[inline]
#[must_use]
pub fn castling(rights: CastlingRights) -> u64 {
    TABLES.castling[rights.zobrist_index()]
}

/// The key for an available en-passant capture on `file`.
#[inline]
#[must_use]
pub fn ep(file: File) -> u64 {
    TABLES.ep[file.index()]
}
