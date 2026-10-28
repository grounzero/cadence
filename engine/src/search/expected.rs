// SPDX-License-Identifier: GPL-3.0-or-later

//! What a node is expected to return, read from how its parent searched it. Nothing decides on it
//! yet, so a search returns the same with it as without.

/// A PV node has the window open; a cut node is expected to fail high, and an all node to fail low.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expected {
    Pv,
    Cut,
    All,
}

/// How a parent searched the child.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Via {
    /// The first move, with the parent's own window.
    First,
    /// The open window again, after a null window was beaten.
    Open,
    /// A late move's reduced null-window search.
    Reduced,
    /// Any other null window at the full depth, the capture probe's included.
    Null,
}

impl Expected {
    /// A full-depth null window expects the opposite of a cut parent, and a cut under any other.
    #[must_use]
    pub const fn child(self, via: Via) -> Expected {
        match (via, self) {
            (Via::Open, _) | (Via::First, Expected::Pv) => Expected::Pv,
            (Via::First | Via::Null, Expected::Cut) => Expected::All,
            (Via::Reduced | Via::First | Via::Null, _) => Expected::Cut,
        }
    }
}
