// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;

use cadence_core::MAX_MOVES;

use super::say::say;
use super::{MAX_THREADS, Session};
use crate::level;
use crate::tt::{self, Table};
use crate::tune::{self, Param};

impl Session {
    /// `name` and `value` delimit names and values, which may contain spaces.
    pub(super) fn set_option<'a>(&mut self, tokens: impl Iterator<Item = &'a str>) {
        let mut name = Vec::new();
        let mut value = Vec::new();
        let mut into_value = false;
        let mut seen_name = false;
        for tok in tokens {
            match tok {
                "name" if !seen_name => seen_name = true,
                "value" if seen_name && !into_value => into_value = true,
                _ if into_value => value.push(tok),
                _ if seen_name => name.push(tok),
                _ => {}
            }
        }
        let name = name.join(" ");
        let value = value.join(" ");
        if name.eq_ignore_ascii_case("UCI_Chess960") {
            if value.eq_ignore_ascii_case("true") {
                self.chess960 = true;
            } else if value.eq_ignore_ascii_case("false") {
                self.chess960 = false;
            }
        } else if name.eq_ignore_ascii_case("Hash") {
            self.set_hash(&value);
        } else if name.eq_ignore_ascii_case("MultiPV") {
            self.set_multipv(&value);
        } else if name.eq_ignore_ascii_case("UCI_LimitStrength") {
            self.set_limit_strength(&value);
        } else if name.eq_ignore_ascii_case("UCI_Elo") {
            self.set_elo(&value);
        } else if name.eq_ignore_ascii_case("Ponder") {
            if value.eq_ignore_ascii_case("true") {
                self.ponder = true;
            } else if value.eq_ignore_ascii_case("false") {
                self.ponder = false;
            }
        } else if name.eq_ignore_ascii_case("Threads") {
            self.set_threads(&value);
        } else if let Some(param) = tune::find(&name) {
            self.set_tunable(param, &value);
        }
        // A GUI sends whatever it was told to.
    }

    /// Clamped, not refused, for [`Session::set_hash`]'s reason. A standing level holds it at one:
    /// a level's move is reproducible only where the search is.
    fn set_threads(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption Threads: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        let asked = asked.clamp(1, MAX_THREADS);
        if asked > 1 && self.limit_strength {
            say(format_args!(
                "info string setoption Threads: a level is reproducible only on one thread, \
                 so Threads stays at 1 while UCI_LimitStrength is on"
            ));
            return;
        }
        self.threads = asked;
    }

    /// A value that is not a number of the parameter's kind is ignored and the old value kept; it
    /// never reaches the search or ends the session.
    fn set_tunable(&mut self, param: &Param, value: &str) {
        match param.parse(value) {
            Some(stored) => param.set(&mut self.tunables, stored),
            None => say(format_args!(
                "info string setoption {}: `{value}` is not a number, keeping {}",
                param.name,
                param.spell(self.tunables.get(param.tunable))
            )),
        }
    }

    /// Clamped, not refused, for [`Session::set_hash`]'s reason.
    fn set_multipv(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption MultiPV: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        let asked = asked.clamp(1, MAX_MOVES);
        if asked > 1 && self.limit_strength {
            say(format_args!(
                "info string setoption MultiPV: a level owns the line count, so MultiPV stays \
                 at 1 while UCI_LimitStrength is on"
            ));
            return;
        }
        self.multipv = asked;
    }

    /// Refuses to engage beside `MultiPV` or `Threads` above one: the level owns the line count,
    /// and its move is reproducible only on one thread.
    fn set_limit_strength(&mut self, value: &str) {
        if value.eq_ignore_ascii_case("true") {
            if self.multipv > 1 {
                say(format_args!(
                    "info string setoption UCI_LimitStrength: UCI_Elo needs MultiPV at 1, so \
                     the level is not engaged"
                ));
                return;
            }
            if self.threads > 1 {
                say(format_args!(
                    "info string setoption UCI_LimitStrength: UCI_Elo needs Threads at 1, so \
                     the level is not engaged"
                ));
                return;
            }
            self.limit_strength = true;
        } else if value.eq_ignore_ascii_case("false") {
            self.limit_strength = false;
        }
    }

    /// Clamped into the ladder's range, and inert while `UCI_LimitStrength` is false.
    fn set_elo(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<u32>() else {
            say(format_args!(
                "info string setoption UCI_Elo: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        self.elo = asked.clamp(level::MIN_ELO, level::MAX_ELO);
    }

    /// The only place the two options are read together.
    pub(super) fn level(&self) -> Option<level::Policy> {
        self.limit_strength.then(|| level::policy(self.elo))
    }

    /// Out-of-range values are clamped rather than refused: a GUI that sends one is not going to
    /// send another.
    fn set_hash(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption Hash: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        let mb = asked.clamp(tt::MIN_HASH_MB, tt::MAX_HASH_MB);
        match Table::new(mb) {
            Some(table) => self.tt = Arc::new(table),
            None => say(format_args!(
                "info string setoption Hash: {mb} MB could not be allocated, keeping {} MB",
                self.tt.bytes() >> 20
            )),
        }
    }
}
