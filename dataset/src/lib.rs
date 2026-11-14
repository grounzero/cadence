// SPDX-License-Identifier: GPL-3.0-or-later

//! One definition of how the writer, the checks and the converter hash and frame training data.

#![forbid(unsafe_code)]

pub mod chunk;
pub mod constants;
pub mod crc32;
pub mod sha256;
pub mod trainer;
