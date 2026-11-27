// SPDX-License-Identifier: GPL-3.0-or-later

use std::iter::Peekable;
use std::num::IntErrorKind;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cadence_core::Colour;

use super::{CLOCK_INTERVAL, Search};
use crate::time;

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
    /// Milliseconds a move loses beyond the engine's own thinking, as the session measured it; set
    /// by the session and never by `go`.
    pub lag: Option<u64>,
    /// The game's move number, from which the moves likely left are reckoned; set by the session.
    pub move_number: Option<u32>,
}

impl Limits {
    /// A known token without a number is skipped, like an unknown one, except a clock's, which
    /// reads as zero.
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
                "movetime" => limits.movetime = Some(clock(&mut it)),
                "wtime" => limits.time[Colour::White.index()] = Some(clock(&mut it)),
                "btime" => limits.time[Colour::Black.index()] = Some(clock(&mut it)),
                "winc" => limits.inc[Colour::White.index()] = Some(clock(&mut it)),
                "binc" => limits.inc[Colour::Black.index()] = Some(clock(&mut it)),
                "movestogo" => limits.movestogo = Some(moves_to_go(&mut it)),
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

/// The keywords of `go`, so a clock missing its value never takes the next keyword as one.
const KEYWORDS: [&str; 12] = [
    "searchmoves",
    "ponder",
    "wtime",
    "btime",
    "winc",
    "binc",
    "movestogo",
    "depth",
    "nodes",
    "mate",
    "movetime",
    "infinite",
];

/// A clock's value, zero where it does not parse: a clock read as absent leaves the search with no
/// limit on a real clock. A keyword in the value's place is left for the main loop.
fn clock<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> u64 {
    if let Some(n) = large(it) {
        return n;
    }
    if it.peek().is_some_and(|t| !KEYWORDS.contains(t)) {
        it.next();
    }
    0
}

fn moves_to_go<'a>(it: &mut Peekable<impl Iterator<Item = &'a str>>) -> u32 {
    u32::try_from(clock(it)).unwrap_or(u32::MAX)
}

impl<const SOFT_STAND_PAT: bool> Search<'_, SOFT_STAND_PAT> {
    /// The limits and stop flag at every node; the clock only every `CLOCK_INTERVAL` nodes, and
    /// only with a budget.
    pub(super) fn out_of_time(&mut self) -> bool {
        if self.run.aborted {
            return true;
        }
        if self.stop.load(Ordering::Relaxed) {
            self.run.aborted = true;
            return true;
        }
        if self.limits.infinite {
            return false;
        }
        if let Some(n) = self.limits.nodes
            && self.reported_nodes() >= n
            && !self.owes_first_line()
        {
            self.run.aborted = true;
            return true;
        }
        if self.run.nodes & (CLOCK_INTERVAL - 1) == 0 {
            // On the existing interval, so a hit does not wait for the iteration's end. Where a fresh
            // search with the same times would not have started the iteration running, its last
            // completed one answers.
            if self.absorb_ponder_hit() && !self.would_have_started() {
                self.run.aborted = true;
                return true;
            }
            if let Some(b) = self.clock.budget
                && self.elapsed_ms() >= b.moved(self.clock.hit_at).hard
            {
                self.run.aborted = true;
                return true;
            }
        }
        false
    }

    /// Helpers can spend a group's node count before its primary has a line, so the primary answers
    /// from its first completed iteration whatever the count says.
    fn owes_first_line(&self) -> bool {
        self.shared_nodes.is_some() && self.worker_index == 0 && self.run.completed_depth == 0
    }

    /// The clock and the record are kept and the deadlines move by the time pondered, so every
    /// decision after the hit is a fresh search's over the same times. True when it absorbs one.
    pub(super) fn absorb_ponder_hit(&mut self) -> bool {
        if !self.clock.pondering || !self.ponder_hit.is_some_and(|f| f.load(Ordering::Relaxed)) {
            return false;
        }
        self.clock.pondering = false;
        self.clock.hit_at = self.elapsed_ms();
        self.clock.budget = self.clock.budget_on_hit;
        true
    }

    /// The decision skipped while pondering, taken at the last completed iteration. With none, the
    /// iteration running is the first and goes on.
    fn would_have_started(&self) -> bool {
        self.clock.budget.is_none_or(|b| {
            time::starts_another(
                &self.limits,
                b,
                &self.run.iterations,
                self.stable_iterations(),
                self.drop_scale(),
                self.clock.hit_at,
            )
        })
    }

    /// Under `infinite` or an unhit ponder, both of which mean do not answer until told. A hit is a
    /// telling too: a ponder that ran out of iterations before it would otherwise never move.
    pub(super) fn wait_if_open_ended(&mut self) {
        while (self.limits.infinite || self.clock.pondering) && !self.stop_requested() {
            self.absorb_ponder_hit();
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use cadence_core::position::Board;
    use cadence_core::{Colour, Move, START_FEN, generate_legal, parse_uci};

    use super::{CLOCK_INTERVAL, Limits, Search};
    use crate::position::Position;
    use crate::score::Score;
    use crate::time::{self, Budget};
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
        alone.run.nodes = 100;
        assert!(alone.out_of_time(), "one thread stops at the limit");

        let mut helper = member(&stop, &tt, &slots, 1);
        assert!(helper.out_of_time(), "a helper stops");

        let mut primary = member(&stop, &tt, &slots, 0);
        primary.run.nodes = 1;
        assert!(!primary.out_of_time(), "the primary owes its first line");
        primary.run.completed_depth = 1;
        assert!(primary.out_of_time(), "and stops once it has one");

        let mut clocked = member(&stop, &tt, &slots, 0);
        clocked.clock.budget = Some(Budget { soft: 0, hard: 0 });
        clocked.run.nodes = CLOCK_INTERVAL;
        assert!(clocked.out_of_time(), "a hard deadline still ends it");

        stop.store(true, Ordering::Relaxed);
        let mut stopped = member(&stop, &tt, &slots, 0);
        assert!(stopped.out_of_time(), "so does stop");
    }

    /// Five completed iterations on one move, the last 100 centipawns under the four before it.
    const FALLING: [Score; 5] = [30, 30, 30, 30, -70];

    /// The same run with nothing lost.
    const STEADY: [Score; 5] = [30; 5];

    /// The king's one move takes the queen giving check.
    const ONE_MOVE: &str = "k7/8/8/8/8/8/1q6/K7 w - - 0 1";

    /// Sudden death, whose hard budget is three times the soft one, so a stretch has room under it.
    fn clock() -> Limits {
        Limits::parse("wtime 6000 btime 6000".split_whitespace())
    }

    fn budget() -> Budget {
        time::budget(&clock(), Colour::White).expect("a clock gives a budget")
    }

    /// What the five-iteration run is allowed before any stretch.
    fn steady_soft(b: Budget) -> u64 {
        time::soft_for_run(&clock(), b, FALLING.len())
    }

    /// As if the completed iterations scored `scores` on one move and ended at `ladder`, under
    /// `b`.
    fn record(search: &mut Search<'_>, scores: &[Score], ladder: &[u64], b: Budget) {
        search.run.roots = scores.iter().map(|&s| (Move::NULL, s)).collect();
        search.run.iterations = ladder.to_vec();
        search.clock.budget = Some(b);
    }

    /// Searched to depth two with `only` the root's one move where given, then given a falling
    /// score at the steady soft budget.
    fn searched_then_fallen<'a>(
        stop: &'a AtomicBool,
        tt: &'a Table,
        fen: &str,
        only: Option<&str>,
    ) -> Search<'a> {
        let mut search = Search::new(stop, tt);
        search.set_limits(Limits::depth(2));
        let board = Board::from_fen(fen).expect("a position");
        if let Some(uci) = only {
            let mv = parse_uci(&generate_legal(&board), uci).expect("a legal move");
            search.set_root_filter(&[mv]);
        }
        search.run(&mut Position::new(board), &mut std::io::sink());
        let b = budget();
        record(&mut search, &FALLING, &[steady_soft(b)], b);
        search
    }

    /// The steady run has spent its soft budget and stops; the same times after a falling score go
    /// on, the next iteration predicted inside the hard budget.
    #[test]
    fn a_falling_score_starts_an_iteration_a_steady_one_does_not() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = Search::new(&stop, &tt);
        search.set_limits(clock());
        let b = budget();
        let soft = steady_soft(b);
        let ladder = [soft / 2, soft * 7 / 10, soft];
        assert!(
            time::another_iteration_fits(&ladder, b),
            "{ladder:?} under {b:?}"
        );

        record(&mut search, &STEADY, &ladder, b);
        assert_eq!(
            search.drop_scale(),
            1000,
            "a steady score stretches nothing"
        );
        assert!(
            !search.would_have_started(),
            "a steady score at {soft} ms of {b:?}"
        );

        record(&mut search, &FALLING, &ladder, b);
        assert!(search.drop_scale() > 1000, "a fall of 100 stretches");
        assert!(
            search.would_have_started(),
            "a falling score at {soft} ms of {b:?}"
        );
    }

    /// However far the score has fallen, an iteration predicted to pass the hard budget is not
    /// started, even inside the soft budget.
    #[test]
    fn the_prediction_stops_a_falling_score() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = Search::new(&stop, &tt);
        search.set_limits(clock());
        let b = budget();
        let elapsed = steady_soft(b) / 2;
        let ladder = [elapsed / 100, elapsed / 10, elapsed];
        assert!(
            !time::another_iteration_fits(&ladder, b),
            "{ladder:?} under {b:?}"
        );

        record(&mut search, &[30, 30, 30, 30, -500], &ladder, b);
        assert_eq!(search.drop_scale(), time::DROP_SCALE_MAX);
        assert!(
            !search.would_have_started(),
            "the largest stretch started {ladder:?} under {b:?}"
        );
    }

    /// The scores of the last search are gone when the next begins, so it starts with no drop.
    #[test]
    fn a_reused_search_starts_with_no_drop() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = Search::new(&stop, &tt);
        search.set_limits(Limits::depth(1));
        let b = budget();
        record(&mut search, &FALLING, &[steady_soft(b)], b);
        assert!(search.drop_scale() > 1000, "the drop a last search left");

        let board = Board::from_fen(START_FEN).expect("the start position");
        search.run(&mut Position::new(board), &mut std::io::sink());
        assert_eq!(
            search.iteration_roots().len(),
            1,
            "{:?}",
            search.iteration_roots()
        );
        assert_eq!(
            search.drop_scale(),
            1000,
            "the last search's drop carried over"
        );
    }

    /// The scores pondered survive the hit on purpose, and the decision taken at it stretches the
    /// budget by them.
    #[test]
    fn a_hit_decides_on_the_scores_pondered() {
        let stop = AtomicBool::new(false);
        let flag = AtomicBool::new(true);
        let tt = Table::new(1).expect("a one mebibyte table");
        let ponder = Limits::parse("ponder wtime 6000 btime 6000".split_whitespace());
        let b = time::hit_budget(&ponder, Colour::White).expect("a ponder with a clock");
        let mut search = Search::new(&stop, &tt);
        search.set_limits(ponder);
        search.set_ponder_hit(&flag);
        search.clock.pondering = true;
        search.clock.budget_on_hit = Some(b);
        search.run.roots = FALLING.iter().map(|&s| (Move::NULL, s)).collect();

        assert!(search.absorb_ponder_hit(), "the raised flag is a hit");
        assert_eq!(search.clock.budget, Some(b), "the hit brought its budget");
        assert_eq!(
            search.iteration_roots().len(),
            FALLING.len(),
            "the hit kept the scores"
        );
        // Spent to the steady soft budget after the time pondered, where a steady score stops.
        let ladder = [steady_soft(b) + search.clock.hit_at];
        search.run.iterations = ladder.to_vec();
        assert!(
            search.would_have_started(),
            "the pondered fall at {ladder:?}"
        );

        search.run.roots = STEADY.iter().map(|&s| (Move::NULL, s)).collect();
        assert!(
            !search.would_have_started(),
            "a steady ponder at {ladder:?}"
        );
    }

    /// Each worker of a group decides on its own scores, so a helper's steady run neither stops the
    /// primary's falling one nor is stretched by it.
    #[test]
    fn the_primary_decides_on_its_own_scores() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let slots: Vec<AtomicU64> = (0..2).map(|_| AtomicU64::new(0)).collect();
        let b = budget();
        let ladder = [steady_soft(b)];
        let mut primary = member(&stop, &tt, &slots, 0);
        let mut helper = member(&stop, &tt, &slots, 1);
        record(&mut primary, &FALLING, &ladder, b);
        record(&mut helper, &STEADY, &ladder, b);
        slots[1].store(1_000_000, Ordering::Relaxed);

        assert!(
            primary.would_have_started(),
            "the helper's run stopped the primary"
        );
        assert!(
            !helper.would_have_started(),
            "the primary's fall stretched the helper"
        );
    }

    /// With one legal move there is no other move to find, so a falling score earns no stretch.
    #[test]
    fn one_legal_move_is_not_stretched() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let board = Board::from_fen(ONE_MOVE).expect("the position");
        assert_eq!(generate_legal(&board).len(), 1, "{ONE_MOVE} has one move");
        let search = searched_then_fallen(&stop, &tt, ONE_MOVE, None);
        assert_eq!(
            search.drop_scale(),
            1000,
            "one legal move stretched by a fall"
        );
        assert!(!search.would_have_started(), "one legal move went on");
    }

    /// `searchmoves` with one move leaves the root one move to search, so it is not stretched
    /// either; with every move the same fall is.
    #[test]
    fn one_searchmoves_move_is_not_stretched() {
        let stop = AtomicBool::new(false);
        let tt = Table::new(1).expect("a one mebibyte table");
        let every = searched_then_fallen(&stop, &tt, START_FEN, None);
        assert!(every.drop_scale() > 1000, "every move and a fall of 100");
        assert!(
            every.would_have_started(),
            "every move went on after a fall"
        );

        let one = searched_then_fallen(&stop, &tt, START_FEN, Some("e2e4"));
        assert_eq!(
            one.drop_scale(),
            1000,
            "one searchmoves move stretched by a fall"
        );
        assert!(!one.would_have_started(), "one searchmoves move went on");
    }
}
