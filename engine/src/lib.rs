// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-engine`: search, UCI and the command-line surface.
//!
//! The binary in `main.rs` is a thin dispatcher over this library. The split
//! exists so that the integration tests can drive the UCI handlers and the
//! search as functions as well as driving the binary as a subprocess: a
//! `position` handler that can only be reached through a pipe can only be
//! tested one line at a time.

#![forbid(unsafe_code)]

pub mod bench;
pub mod eval;
pub mod history;
pub mod perft;
pub mod picker;
pub mod score;
pub mod search;
pub mod see;
pub mod time;
pub mod tt;
pub mod uci;
pub mod version;
