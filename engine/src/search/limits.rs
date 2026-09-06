// SPDX-License-Identifier: GPL-3.0-or-later

//! What a `go` command asked for, parsed. The search reads it and never writes it, so a limit
//! that did not appear stays `None` all the way down.

use std::iter::Peekable;

use cadence_core::Colour;

/// What a `go` command asked for. A field that is `None` did not appear and must not influence
/// the search.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// `go depth N`: stop after completing iteration N.
    pub depth: Option<u32>,
    /// `go nodes N`: stop after N nodes.
    pub nodes: Option<u64>,
    /// `go movetime N`: search for exactly N milliseconds.
    pub movetime: Option<u64>,
    /// `go infinite`: search until `stop`, and do not return before it.
    pub infinite: bool,
    /// `wtime` / `btime`, in milliseconds, indexed by `Colour`.
    pub time: [Option<u64>; 2],
    /// `winc` / `binc`, in milliseconds, indexed by `Colour`.
    pub inc: [Option<u64>; 2],
    /// `movestogo N`: moves until the next time control.
    pub movestogo: Option<u32>,
}

impl Limits {
    /// Parse the tokens that follow `go`. Unknown tokens are skipped; a known token without a
    /// number, or with something that is not a number, is skipped too.
    #[must_use]
    pub fn parse<'a>(tokens: impl Iterator<Item = &'a str>) -> Limits {
        let mut limits = Limits::default();
        let mut it = tokens.peekable();
        while let Some(token) = it.next() {
            match token {
                "infinite" => limits.infinite = true,
                "depth" => limits.depth = small(&mut it),
                "nodes" => limits.nodes = large(&mut it),
                "movetime" => limits.movetime = large(&mut it),
                "wtime" => limits.time[Colour::White.index()] = large(&mut it),
                "btime" => limits.time[Colour::Black.index()] = large(&mut it),
                "winc" => limits.inc[Colour::White.index()] = large(&mut it),
                "binc" => limits.inc[Colour::Black.index()] = large(&mut it),
                "movestogo" => limits.movestogo = small(&mut it),
                // Accepted and ignored. `mate` takes a number, which is consumed so it is not
                // mistaken for anything else; the moves after `searchmoves` are unknown tokens
                // and fall through to the arm below.
                "mate" => {
                    let _ = small(&mut it);
                }
                _ => {}
            }
        }
        limits
    }

    /// A fixed-depth search.
    #[must_use]
    pub fn depth(depth: u32) -> Limits {
        Limits {
            depth: Some(depth),
            ..Limits::default()
        }
    }

    /// Search until `stop`.
    #[must_use]
    pub fn infinite() -> Limits {
        Limits {
            infinite: true,
            ..Limits::default()
        }
    }

    /// The side to move's clock and increment, if a clock was given.
    #[must_use]
    pub fn clock(&self, us: Colour) -> Option<(u64, u64)> {
        self.time[us.index()].map(|t| (t, self.inc[us.index()].unwrap_or(0)))
    }

    /// Whether this `go` named a clock at all, for either side. A `go` that named the other
    /// side's clock and not ours is clocked: nothing is known about our own time, and the safe
    /// reading of that is zero rather than unlimited.
    #[must_use]
    pub fn is_clocked(&self) -> bool {
        self.time.iter().chain(self.inc.iter()).any(Option::is_some) || self.movestogo.is_some()
    }
}

/// The next token as a non-negative number, consumed only if it is one. A token that does not
/// parse is left for the main loop, which skips it -- it may be the next keyword.
fn large<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> Option<u64> {
    let n: i64 = it.peek()?.parse().ok()?;
    it.next();
    Some(u64::try_from(n.max(0)).expect("non-negative"))
}

fn small<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> Option<u32> {
    large(it).map(|n| u32::try_from(n).unwrap_or(u32::MAX))
}
