// SPDX-License-Identifier: GPL-3.0-or-later

//! Built at compile time from a fixed seed, so two builds agree.

use crate::castling::CastlingRights;
use crate::rng::splitmix64;
use crate::types::{File, Piece, Square};

/// Changing it invalidates any stored record that carried keys.
const SEED: u64 = 0x00CA_DE7C_E5EE_D002;

/// Drawn in this order, which the keys depend on.
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

/// One key per rights set: losing a right XORs the old set's key out and the new one's in.
#[inline]
#[must_use]
pub fn castling(rights: CastlingRights) -> u64 {
    TABLES.castling[rights.zobrist_index()]
}

/// Only when a capture is available.
#[inline]
#[must_use]
pub fn ep(file: File) -> u64 {
    TABLES.ep[file.index()]
}
