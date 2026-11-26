// SPDX-License-Identifier: GPL-3.0-or-later

//! The `bestmove` a `go` owes, given once: by the search thread on its way out, or by the session
//! when that thread is held up past where it should answer, as a blocked tablebase read holds one.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cadence_core::{Move, MoveList, to_uci};

use super::say::say;

/// Far past any search's own stop, which reads its flag at every node, so only a thread held up
/// outside the search waits this long.
pub(super) const STOP_WAIT: Duration = Duration::from_secs(1);

/// Past the hard deadline the search keeps itself, so only a thread held up is answered for.
pub(super) const DEADLINE_GRACE: Duration = Duration::from_millis(100);

pub(super) struct Answer {
    given: AtomicBool,
    /// When the line was printed, which is when the GUI's clock stopped for this move.
    given_at: Mutex<Option<Instant>>,
    /// The best move of the search's last completed iteration, `Move::NULL` before the first.
    pub(super) standing: Mutex<Move>,
    legal: MoveList,
    chess960: bool,
    /// The first `searchmoves` move or legal move, for a thread held up before any iteration.
    fallback: String,
    finished: Mutex<bool>,
    done: Condvar,
}

impl Answer {
    pub(super) fn new(legal: MoveList, chess960: bool, fallback: String) -> Answer {
        Answer {
            given: AtomicBool::new(false),
            given_at: Mutex::new(None),
            standing: Mutex::new(Move::NULL),
            legal,
            chess960,
            fallback,
            finished: Mutex::new(false),
            done: Condvar::new(),
        }
    }

    pub(super) fn given(&self) -> bool {
        self.given.load(Ordering::SeqCst)
    }

    pub(super) fn given_at(&self) -> Option<Instant> {
        *lock(&self.given_at)
    }

    /// The search thread's own line, unless the session has already answered.
    pub(super) fn give(&self, line: &str) {
        if !self.given.swap(true, Ordering::SeqCst) {
            say(format_args!("{line}"));
            *lock(&self.given_at) = Some(Instant::now());
        }
    }

    /// For a thread held up: the last completed iteration's move, with no ponder move.
    pub(super) fn give_standing(&self, why: &str) {
        if self.given.swap(true, Ordering::SeqCst) {
            return;
        }
        let standing = *lock(&self.standing);
        let spelled = if standing == Move::NULL {
            self.fallback.clone()
        } else {
            to_uci(standing, &self.legal, self.chess960)
        };
        say(format_args!(
            "info string {why}, so it is answered from its last completed iteration"
        ));
        say(format_args!("bestmove {spelled}"));
        *lock(&self.given_at) = Some(Instant::now());
    }

    /// True where the search thread finished within `wait`.
    pub(super) fn wait(&self, wait: Duration) -> bool {
        let finished = lock(&self.finished);
        let (finished, _) = self
            .done
            .wait_timeout_while(finished, wait, |finished| !*finished)
            .unwrap_or_else(PoisonError::into_inner);
        *finished
    }

    fn finish(&self) {
        *lock(&self.finished) = true;
        self.done.notify_all();
    }
}

/// Held by the search thread, so the session hears it finish however it ends.
pub(super) struct Finishing<'a>(pub(super) &'a Answer);

impl Drop for Finishing<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// The primary's `info` lines, dropped once the answer is given so that nothing follows it.
pub(super) struct Gated<'a> {
    pub(super) answer: &'a Answer,
    pub(super) out: io::Stdout,
}

impl Write for Gated<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.answer.given() {
            return Ok(buf.len());
        }
        self.out.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    /// Whole, as `Stdout` writes a line under one lock.
    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> io::Result<()> {
        if self.answer.given() {
            return Ok(());
        }
        self.out.write_fmt(args)
    }
}

/// Answers for the search and stops it if it has not answered by `deadline`.
pub(super) fn watch(answer: &Arc<Answer>, stop: &Arc<AtomicBool>, deadline: Instant) {
    let (answer, stop) = (Arc::clone(answer), Arc::clone(stop));
    let started = std::thread::Builder::new()
        .name("deadline".to_string())
        .spawn(move || {
            if !answer.wait(deadline.saturating_duration_since(Instant::now())) {
                stop.store(true, Ordering::Relaxed);
                answer.give_standing("the search is held up past its deadline");
            }
        });
    if let Err(e) = started {
        say(format_args!(
            "info string go: could not start the deadline watch: {e}"
        ));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
