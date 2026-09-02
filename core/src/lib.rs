// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-core`: board representation, move generation and the single definition of NNUE
//! feature indexing. `no_std` + `alloc`: the search state stack and the game history are heap
//! allocations.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

/// Maximum search depth in plies. Bounds the accumulator stack, the killer tables and the PV
/// table.
pub const MAX_PLY: usize = 256;

pub mod attacks;
pub mod bitboard;
pub mod castling;
pub mod dirty;
pub mod features;
pub mod fen;
mod magic;
pub mod movegen;
pub mod mv;
pub mod perft;
pub mod position;
pub mod types;
pub mod zobrist;

pub use bitboard::Bitboard;
pub use castling::{CastleSide, CastlingRights};
pub use dirty::{DirtyPiece, DirtyPieces, MAX_DIRTY, MAX_DIRTY_REACHABLE};
pub use features::{NUM_INPUTS, feature_index};
pub use fen::{FenError, FenStyle, START_FEN};
pub use movegen::{generate_legal, generate_noisy};
pub use mv::{MAX_MOVES, Move, MoveList, parse_uci, to_uci};
pub use perft::{perft, perft_divide};
pub use position::{Board, StateInfo};
pub use types::{Colour, OptSquare, Piece, PieceType, Square};
