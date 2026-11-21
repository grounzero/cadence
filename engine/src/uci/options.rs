// SPDX-License-Identifier: GPL-3.0-or-later

use std::sync::Arc;

use cadence_core::MAX_MOVES;
use cadence_dataset::sha256::sha256;
use cadence_nnue::AnyNetwork;

use super::say::say;
use super::{MAX_MOVE_OVERHEAD_MS, MAX_THREADS, NetOptions, Session};
use crate::level;
use crate::syzygy::{MAX_PROBE_LIMIT, Tables};
use crate::tt::{self, Table};
use crate::tune::{self, Param};

impl NetOptions {
    /// Scoring with the net in `bytes` by default, the net the build ships; empty bytes ship none.
    pub(super) fn shipping(bytes: &[u8]) -> NetOptions {
        let shipped = AnyNetwork::shipped(bytes).map(|loaded| {
            loaded
                .map(|net| (net, sha256(bytes)))
                .map_err(|e| e.to_string())
        });
        let mut options = NetOptions {
            shipped,
            ..NetOptions::default()
        };
        options.on = options.ships();
        options.restore_shipped();
        options
    }

    /// Whether the build ships a net its loader took.
    pub(super) fn ships(&self) -> bool {
        matches!(self.shipped, Some(Ok(_)))
    }

    /// The shipped net in place of any file's, or no net where the build ships none.
    fn restore_shipped(&mut self) {
        (self.net, self.key) = match &self.shipped {
            Some(Ok((net, key))) => (Some(net.clone()), Some(*key)),
            _ => (None, None),
        };
    }
}

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
        } else if name.eq_ignore_ascii_case("Use NNUE") {
            let before = self.evaluator();
            if value.eq_ignore_ascii_case("true") {
                self.nnue.on = true;
            } else if value.eq_ignore_ascii_case("false") {
                self.nnue.on = false;
            }
            self.evaluator_changed(before);
        } else if name.eq_ignore_ascii_case("EvalFile") {
            let before = self.evaluator();
            self.set_eval_file(path);
            self.evaluator_changed(before);
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

    /// Empty, or `<empty>`, returns to the net the build ships, or to none where it ships none; a
    /// file the loader refuses does the same, and says why.
    fn set_eval_file(&mut self, value: &str) {
        let value = value.trim();
        self.nnue.file = if value.eq_ignore_ascii_case("<empty>") {
            String::new()
        } else {
            value.to_string()
        };
        self.nnue.restore_shipped();
        if self.nnue.file.is_empty() {
            return;
        }
        let loaded = std::fs::read(&self.nnue.file)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                AnyNetwork::from_bytes(&bytes)
                    .map(|net| (net, sha256(&bytes)))
                    .map_err(|e| e.to_string())
            });
        match loaded {
            Ok((net, key)) => {
                say(format_args!(
                    "info string EvalFile: {} loaded, provenance {}, trainer {}",
                    self.nnue.file,
                    net.provenance(),
                    net.trainer()
                ));
                self.nnue.net = Some(net);
                self.nnue.key = Some(key);
            }
            Err(e) => say(format_args!(
                "info string EvalFile: {}: {e}",
                self.nnue.file
            )),
        }
    }

    /// What scores a search: `None` for the handcrafted evaluation, else the net's file digest.
    fn evaluator(&self) -> Option<[u8; 32]> {
        self.nnue
            .key
            .filter(|_| self.nnue.on && self.nnue.net.is_some())
    }

    /// The table holds scores, so another evaluator starts from an empty one, as `ucinewgame` does.
    fn evaluator_changed(&mut self, before: Option<[u8; 32]>) {
        if self.evaluator() != before {
            self.stop_search();
            self.tt.clear();
        }
    }

    /// The net a `go` scores with: only when `Use NNUE` is on and a net loaded.
    pub(super) fn active_net(&self) -> Option<AnyNetwork> {
        if self.nnue.on && self.nnue.net.is_none() {
            say(format_args!(
                "info string Use NNUE is on and no net is loaded; the handcrafted evaluation scores this search"
            ));
        }
        self.nnue.net.clone().filter(|_| self.nnue.on)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cadence_dataset::sha256::sha256;
    use cadence_nnue::{Network, Provenance};

    use super::Session;

    /// A session whose build ships a seeded net of `provenance`, and that net's digest.
    fn shipping(provenance: Provenance) -> (Session, [u8; 32]) {
        let bytes = Network::seeded(7, provenance).to_bytes();
        (Session::shipping(&bytes), sha256(&bytes))
    }

    fn net_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "cadence-shipped-{name}-{}.cdnn",
            std::process::id()
        ));
        std::fs::write(&path, bytes).expect("write the net file");
        path
    }

    /// A shipped net scores every search from the start, with no option set.
    #[test]
    fn a_shipped_net_scores_by_default() {
        let (session, key) = shipping(Provenance::SelfPlay);
        assert!(session.nnue.ships());
        assert_eq!(session.evaluator(), Some(key));
    }

    /// `EvalFile` replaces the shipped net, and emptying it returns to the shipped net.
    #[test]
    fn eval_file_overrides_the_shipped_net_until_it_is_emptied() {
        let (mut session, key) = shipping(Provenance::SelfPlay);
        let other = Network::seeded(8, Provenance::SelfPlay).to_bytes();
        let file = net_file("override", &other);
        session.set_option(&format!("setoption name EvalFile value {}", file.display()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(session.evaluator(), Some(sha256(&other)));
        session.set_option("setoption name EvalFile value <empty>");
        assert_eq!(session.evaluator(), Some(key));
    }

    /// A file the loader refuses leaves the shipped net scoring, not none.
    #[test]
    fn a_refused_file_leaves_the_shipped_net() {
        let (mut session, key) = shipping(Provenance::SelfPlay);
        let file = net_file("refused", b"not a net");
        session.set_option(&format!("setoption name EvalFile value {}", file.display()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(session.evaluator(), Some(key));
    }

    /// `Use NNUE` turns the shipped net off and on again as it does a loaded one.
    #[test]
    fn use_nnue_switches_the_shipped_net_off_and_on() {
        let (mut session, key) = shipping(Provenance::SelfPlay);
        session.set_option("setoption name Use NNUE value false");
        assert_eq!(session.evaluator(), None);
        session.set_option("setoption name Use NNUE value true");
        assert_eq!(session.evaluator(), Some(key));
    }

    /// A build that ships no net starts on the handcrafted evaluation with the switch off.
    #[test]
    fn a_build_that_ships_no_net_starts_handcrafted() {
        let session = Session::shipping(&[]);
        assert!(!session.nnue.on);
        assert!(session.nnue.shipped.is_none());
        assert_eq!(session.evaluator(), None);
    }

    /// A canary net or an external one is refused as the shipped net, and scores nothing.
    #[test]
    fn a_shipped_net_that_is_not_self_play_is_refused() {
        let validation = Provenance::External("lichess-big3-resolved".to_string());
        for provenance in [Provenance::Canary, validation] {
            let (session, _) = shipping(provenance);
            assert!(!session.nnue.on);
            assert!(matches!(session.nnue.shipped, Some(Err(_))));
            assert_eq!(session.evaluator(), None);
        }
    }
}
