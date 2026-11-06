// SPDX-License-Identifier: GPL-3.0-or-later

use std::iter::Peekable;
use std::num::IntErrorKind;
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
    /// Stop once an iteration proves mate in this many moves or fewer.
    pub mate: Option<u32>,
    /// Kept back from every budget; `None` is `time::MOVE_OVERHEAD_MS`, so a literal leaves it alone.
    pub move_overhead: Option<u64>,
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
                "mate" => limits.mate = small(&mut it),
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
/// A number past either end of `i64` is a limit and not an absence, so it reads as that end.
fn large<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> Option<u64> {
    let n = match it.peek()?.parse::<i64>() {
        Ok(n) => n,
        Err(e) if *e.kind() == IntErrorKind::PosOverflow => i64::MAX,
        Err(e) if *e.kind() == IntErrorKind::NegOverflow => i64::MIN,
        Err(_) => return None,
    };
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
            && !self.owes_first_line()
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

    /// Helpers can spend a group's node count before its primary has a line, so the primary answers
    /// from its first completed iteration whatever the count says.
    fn owes_first_line(&self) -> bool {
        self.shared_nodes.is_some() && self.worker_index == 0 && self.completed_depth == 0
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

    /// Under `infinite` or an unhit ponder, both of which mean do not answer until told. A hit is a
    /// telling too: a ponder that ran out of iterations before it would otherwise never move.
    pub(super) fn wait_if_open_ended(&mut self) {
        while (self.limits.infinite || self.pondering) && !self.stop_requested() {
            self.absorb_ponder_hit();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use super::{CLOCK_INTERVAL, Limits, Search};
    use crate::time::Budget;
    use crate::tt::Table;

    fn member<'a>(
        stop: &'a AtomicBool,
        tt: &'a Table,
        slots: &'a [AtomicU64],
        worker_index: usize,
    ) -> Search<'a> {
        let mut search = Search::new(stop, tt);
        search.set_limits(Limits {
            nodes: Some(100),
            ..Limits::default()
        });
        search.set_parallel(worker_index, slots);
        search
    }

    /// A third worker has spent the limit; only the primary still owes a line, and the hard
    /// deadline and the stop flag end it all the same.
    #[test]
    fn a_spent_node_limit_holds_back_only_a_primary_without_a_line() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let slots: Vec<AtomicU64> = (0..3).map(|_| AtomicU64::new(0)).collect();
        slots[2].store(1_000, Ordering::Relaxed);

        let mut alone = Search::new(&stop, &tt);
        alone.set_limits(Limits {
            nodes: Some(100),
            ..Limits::default()
        });
        alone.nodes = 100;
        assert!(alone.out_of_time(), "one thread stops at the limit");

        let mut helper = member(&stop, &tt, &slots, 1);
        assert!(helper.out_of_time(), "a helper stops");

        let mut primary = member(&stop, &tt, &slots, 0);
        primary.nodes = 1;
        assert!(!primary.out_of_time(), "the primary owes its first line");
        primary.completed_depth = 1;
        assert!(primary.out_of_time(), "and stops once it has one");

        let mut clocked = member(&stop, &tt, &slots, 0);
        clocked.budget = Some(Budget { soft: 0, hard: 0 });
        clocked.nodes = CLOCK_INTERVAL;
        assert!(clocked.out_of_time(), "a hard deadline still ends it");

        stop.store(true, Ordering::Relaxed);
        let mut stopped = member(&stop, &tt, &slots, 0);
        assert!(stopped.out_of_time(), "so does stop");
    }
}
