// SPDX-License-Identifier: GPL-3.0-or-later

//! The settings that shape every game, defined once so the writer, `verify` and the converter cannot
//! disagree. The engine declares each under its own name with the value from here.

/// Nodes a move, compiled in so a record's provenance names every setting that shaped it.
pub const NODES: u64 = 5_000;

/// Cleared at the start of every game.
pub const HASH_MB: usize = 16;

/// Half for each side.
pub const RANDOM_PLIES: usize = 8;

/// The same as a game move's.
pub const SCREEN_NODES: u64 = 5_000;

/// Drops gross imbalance without trying to keep a start balanced.
pub const WINDOW: i32 = 200;

/// Plies of the game proper skipped before the window opens. The random plies are not in a
/// record's moves, so they are not among these.
pub const FIRST_PLY: usize = 8;

pub const LAST_PLY: usize = 240;

/// At or above this in magnitude a score is a mate, the engine's `MATE_IN_MAX_PLY`, held equal to it
/// by a test.
pub const MATE_BOUND: i16 = 31_744;
