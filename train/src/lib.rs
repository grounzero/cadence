// SPDX-License-Identifier: GPL-3.0-or-later

//! The trainer's checkpoint to the engine's net file, apart from the recipe so that its tests run
//! with no device.

#![forbid(unsafe_code)]

pub mod book;
pub mod checkpoint;
pub mod data;
pub mod export;
pub mod overfit;
pub mod validate;
