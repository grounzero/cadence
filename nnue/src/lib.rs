// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-nnue`: empty today, and holding the workspace's one `unsafe`
//! exception.
//!
//! There is no code here yet. What the crate is for, when it is written, is
//! the accumulator, the quantised forward pass and the SIMD paths.
//!
//! The policy is stated ahead of the code because it is the reason the crate
//! is separate at all. This is the one crate in the workspace where `unsafe`
//! is permitted, and only for SIMD intrinsics; every `unsafe` block will
//! carry a `// SAFETY:` comment naming the invariant it upholds. `core` and
//! `engine` carry `#![forbid(unsafe_code)]`, which cannot be relaxed from
//! inside a crate that has it, so the intrinsics could not live there.

#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
