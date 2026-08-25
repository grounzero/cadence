// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-nnue`: accumulator, quantised forward pass and the SIMD paths.
//!
//! This is the one crate in the workspace where `unsafe` is permitted, and
//! only for SIMD intrinsics. Every `unsafe` block carries a `// SAFETY:`
//! comment naming the invariant it upholds.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
