// SPDX-License-Identifier: GPL-3.0-or-later

//! What a node is expected to return, read from how its parent searched it. The late move
//! reduction reads it, taking one ply more from a late move at an expected cut node.

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

/// One call into the main search, as a debug build records it for the gates.
#[cfg(debug_assertions)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Call {
    pub ply: usize,
    pub depth: u32,
    /// The parent's kind and how it searched the child; `None` where the caller named the kind
    /// outright, as the root, the null move and the gates' seam do.
    pub asked: Option<(Expected, Via)>,
    pub kind: Expected,
    /// Whether the window it was given is open.
    pub open: bool,
}
