// SPDX-License-Identifier: GPL-3.0-or-later

use std::iter::Peekable;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use cadence_core::Colour;

use super::{CLOCK_INTERVAL, Search};

/// A `None` field did not appear, and the search never writes one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// After completing iteration N.
    pub depth: Option<u32>,
    pub nodes: Option<u64>,
    /// Exactly N milliseconds.
    pub movetime: Option<u64>,
    /// Do not return before `stop`.
    pub infinite: bool,
    /// Bounded like `infinite`, reading no clock: a ponder that answered on a budget would answer a
    /// question not yet asked.
    pub ponder: bool,
    /// Milliseconds, indexed by `Colour`.
    pub time: [Option<u64>; 2],
    /// Milliseconds, indexed by `Colour`.
    pub inc: [Option<u64>; 2],
    pub movestogo: Option<u32>,
}

impl Limits {
    /// A known token without a number is skipped, like an unknown one.
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
                // Accepted and ignored; `mate`'s number is consumed so it is not misread, and
                // `searchmoves`' moves fall through as unknown tokens.
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

    #[must_use]
    pub fn clock(&self, us: Colour) -> Option<(u64, u64)> {
        self.time[us.index()].map(|t| (t, self.inc[us.index()].unwrap_or(0)))
    }

    /// Either side's: with only the other's, our own time reads as zero rather than unlimited.
    #[must_use]
    pub fn is_clocked(&self) -> bool {
        self.time.iter().chain(self.inc.iter()).any(Option::is_some) || self.movestogo.is_some()
    }
}

/// Consumed only if it parses; otherwise left for the main loop, since it may be the next keyword.
fn large<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> Option<u64> {
    let n: i64 = it.peek()?.parse().ok()?;
    it.next();
    Some(u64::try_from(n.max(0)).expect("non-negative"))
}

fn small<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> Option<u32> {
    large(it).map(|n| u32::try_from(n).unwrap_or(u32::MAX))
}

impl Search<'_> {
    /// The limits and stop flag at every node; the clock only every `CLOCK_INTERVAL` nodes, and
    /// only with a budget.
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
            // On the existing interval: a hit that waited for the iteration's end would spend the
            // clock it just took.
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

    /// The ladder is cleared: an entry measured from the ponder's origin gives a branching factor
    /// below one and starts an iteration on it.
    pub(super) fn absorb_ponder_hit(&mut self) {
        if !self.pondering || !self.ponder_hit.is_some_and(|f| f.load(Ordering::Relaxed)) {
            return;
        }
        self.pondering = false;
        self.start = Instant::now();
        self.iterations.clear();
        self.budget = self.budget_on_hit;
    }

    /// Under `infinite` or an unhit ponder, both of which mean do not answer until told.
    pub(super) fn wait_if_open_ended(&self) {
        if self.limits.infinite || self.pondering {
            while !self.stop_requested() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}
