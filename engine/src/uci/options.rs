// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;

use cadence_core::MAX_MOVES;

use super::say::say;
use super::{MAX_MOVE_OVERHEAD_MS, MAX_THREADS, Session};
use crate::level;
use crate::syzygy::{MAX_PROBE_LIMIT, Tables};
use crate::tt::{self, Table};
use crate::tune::{self, Param};

impl Session {
    /// `name` and `value` delimit names and values, which may contain spaces. A path is kept as
    /// sent between its first and last character that is not blank, its own spacing included.
    pub(super) fn set_option(&mut self, line: &str) {
        let mut name = Vec::new();
        let mut value = Vec::new();
        let mut path = "";
        let mut into_value = false;
        let mut seen_name = false;
        // Where the last word ended: words are found in order, so a word's text first occurs at it.
        let mut end = 0;
        for tok in line.split_whitespace() {
            let at = line[end..].find(tok).map_or(end, |found| end + found);
            end = at + tok.len();
            match tok {
                "name" if !seen_name => seen_name = true,
                "value" if seen_name && !into_value => {
                    into_value = true;
                    path = line[end..].trim();
                }
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
        } else if name.eq_ignore_ascii_case("Move Overhead") {
            self.set_move_overhead(&value);
        } else if name.eq_ignore_ascii_case("Clear Hash") {
            // `ucinewgame`'s table half, under the name a GUI's button sends.
            self.stop_search();
            self.tt.clear();
        } else if name.eq_ignore_ascii_case("SyzygyPath") {
            self.set_syzygy_path(path);
        } else if name.eq_ignore_ascii_case("SyzygyProbeLimit") {
            self.set_probe_limit(&value);
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

    /// Empty, or `<empty>`, turns probing off. A search already running keeps the set it was
    /// given.
    fn set_syzygy_path(&mut self, value: &str) {
        let value = value.trim();
        self.syzygy_path = if value.eq_ignore_ascii_case("<empty>") {
            String::new()
        } else {
            value.to_string()
        };
        self.load_tables();
    }

    /// Clamped, not refused, for [`Session::set_hash`]'s reason; zero turns probing off.
    fn set_probe_limit(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption SyzygyProbeLimit: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        self.probe_limit = asked.min(MAX_PROBE_LIMIT);
        self.load_tables();
    }

    /// No file is opened here: a table is read when a probe first needs it.
    fn load_tables(&mut self) {
        if self.syzygy_path.is_empty() {
            self.tables = None;
            return;
        }
        let (tables, problems) = Tables::load(&self.syzygy_path, self.probe_limit);
        for problem in problems {
            say(format_args!("info string SyzygyPath: {problem}"));
        }
        say(format_args!(
            "info string SyzygyPath: {} table files, probing up to {} pieces",
            tables.files(),
            tables.cardinality()
        ));
        self.tables = (tables.cardinality() > 0).then(|| Arc::new(tables));
    }

    /// Clamped, not refused, for [`Session::set_hash`]'s reason.
    fn set_move_overhead(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<u64>() else {
            say(format_args!(
                "info string setoption Move Overhead: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        self.move_overhead = asked.min(MAX_MOVE_OVERHEAD_MS);
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
        // The running search holds the old table, so it is stopped before the new one is zeroed:
        // `bestmove` comes at once, and two tables are never resident.
        self.stop_search();
        match Table::new(mb) {
            Some(table) => self.tt = Arc::new(table),
            None => say(format_args!(
                "info string setoption Hash: {mb} MB could not be allocated, keeping {} MB",
                self.tt.bytes() >> 20
            )),
        }
    }
}
