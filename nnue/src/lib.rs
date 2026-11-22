// SPDX-License-Identifier: GPL-3.0-or-later

//! The network's quantised inference: the accumulator a search updates move by move and the output
//! layer read from it, one portable integer source compiled for each target with no `unsafe`.

#![forbid(unsafe_code)]

pub mod accumulator;
#[cfg(any(feature = "simd-wide", feature = "simd-pulp"))]
pub mod diag;
pub mod eval;
pub mod net;

pub use accumulator::{Accumulator, AccumulatorStack};
pub use net::{AnyNetwork, HIDDEN, LoadError, Network, Provenance, WIDE};
