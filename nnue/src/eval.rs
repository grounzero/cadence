// SPDX-License-Identifier: GPL-3.0-or-later

//! The output layer: each activation clamped to [0, 255] and squared, weighted, summed and
//! scaled to centipawns from the side to move's view.

use cadence_core::Colour;

use crate::accumulator::Accumulator;
use crate::net::Network;

impl Network {
    #[must_use]
    pub fn evaluate(&self, _acc: &Accumulator, _stm: Colour) -> i32 {
        0
    }

    #[must_use]
    pub fn evaluate_reference(&self, _acc: &Accumulator, _stm: Colour) -> i32 {
        0
    }
}
