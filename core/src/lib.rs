// SPDX-License-Identifier: GPL-3.0-or-later

//! The one definition of NNUE feature indexing lives here. `alloc` because the state stack and
//! game history are on the heap.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

/// Sizes the state stack, the killers and the principal-variation table.
pub const MAX_PLY: usize = 256;

pub mod attacks;
pub mod bitboard;
pub mod castling;
pub mod chess960;
pub mod cycles;
pub mod dirty;
pub mod features;
pub mod fen;
mod magic;
pub mod movegen;
pub mod mv;
pub mod perft;
pub mod position;
pub mod rng;
pub mod types;
pub mod zobrist;

pub use bitboard::Bitboard;
pub use castling::{CastleSide, CastlingRights};
pub use dirty::{DirtyPiece, DirtyPieces, MAX_DIRTY, MAX_DIRTY_REACHABLE};
pub use features::{
    KING_BUCKET, KING_BUCKETS, NUM_INPUTS, bucketed_feature_index, feature_index, king_bucket,
    mirrored_feature_index, mirrors,
};
pub use fen::{FenError, FenStyle, START_FEN};
pub use movegen::{generate_legal, generate_noisy, has_legal_move, is_legal};
pub use mv::{MAX_MOVES, Move, MoveList, parse_uci, to_uci};
pub use perft::{perft, perft_divide};
pub use position::{Board, StateInfo};
pub use types::{Colour, OptSquare, Piece, PieceType, Square};
