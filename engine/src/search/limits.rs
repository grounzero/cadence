// SPDX-License-Identifier: GPL-3.0-or-later

//! What a `go` command asked for, parsed, and how a search keeps to it. The search never writes a
//! limit, so one that did not appear stays `None` all the way down.

use std::iter::Peekable;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cadence_core::Colour;

use super::{CLOCK_INTERVAL, Search};

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
    /// `go ponder`: search the opponent's time until `stop` or `ponderhit`, reading no clock.
    /// It bounds the search the way `infinite` does, because a ponder that returns a move on a
    /// budget has answered a question nobody asked yet.
    pub ponder: bool,
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
                "ponder" => limits.ponder = true,
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

    #[must_use]
    pub fn depth(depth: u32) -> Limits {
        Limits {
            depth: Some(depth),
            ..Limits::default()
        }
    }

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

impl Search<'_> {
    /// Whether a limit or the stop flag ends the search here, at any node from the first; the
    /// clock only every `CLOCK_INTERVAL` nodes, and only when there is a budget.
    pub(super) fn out_of_time(&mut self) -> bool {
        if self.aborted {
            return true;
        }
        if self.stop.load(Ordering::Relaxed) {
            self.aborted = true;
            return true;
        }
        if self.limits.infinite {
            return false;
        }
        if let Some(n) = self.limits.nodes
            && self.reported_nodes() >= n
        {
            self.aborted = true;
            return true;
        }
        if self.nodes & (CLOCK_INTERVAL - 1) == 0 {
            // On the interval that already exists rather than on one of its own: a hit that
            // waited for the end of a deep iteration would spend the clock it just took.
            self.absorb_ponder_hit();
            if let Some(b) = self.budget
                && self.elapsed_ms() >= b.hard
            {
                self.aborted = true;
                return true;
            }
        }
        false
    }

    /// Take a `ponderhit` if one has arrived: the search stops pondering, the clock runs from
    /// this moment, and the iteration ladder starts again. The ladder is cleared because an
    /// entry measured from the ponder's origin, read against the new one, gives a branching
    /// factor below one and starts an iteration on it.
    pub(super) fn absorb_ponder_hit(&mut self) {
        if !self.pondering || !self.ponder_hit.is_some_and(|f| f.load(Ordering::Relaxed)) {
            return;
        }
        self.pondering = false;
        self.start = Instant::now();
        self.iterations.clear();
        self.budget = self.budget_on_hit;
    }

    /// Hold the finished search until `stop`, in the two states that say so. `go infinite` and
    /// a ponder nobody has hit both mean "do not answer until told", and a ponder that returned
    /// early would be answering a question the opponent has not yet asked.
    pub(super) fn wait_if_open_ended(&self) {
        if self.limits.infinite || self.pondering {
            while !self.stop_requested() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
