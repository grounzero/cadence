// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::Read;
use std::sync::Arc;

use cadence_core::MAX_MOVES;
use cadence_dataset::sha256::sha256;
use cadence_nnue::AnyNetwork;
use cadence_nnue::net::{HEADER_LEN, LoadError};

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
            self.clear_learned();
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

    /// Clamped, not refused, for [`Session::set_hash`]'s reason. Kept under a level, whose search
    /// runs on one thread whatever this says.
    fn set_threads(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption Threads: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        self.threads = asked.clamp(1, MAX_THREADS);
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

    /// Clamped, not refused, for [`Session::set_hash`]'s reason. Kept under a level, which reports
    /// its own lines whatever this says.
    fn set_multipv(&mut self, value: &str) {
        let Ok(asked) = value.trim().parse::<usize>() else {
            say(format_args!(
                "info string setoption MultiPV: `{value}` is not a number, ignoring it"
            ));
            return;
        };
        self.multipv = asked.clamp(1, MAX_MOVES);
    }

    /// Engages beside any `MultiPV` and `Threads`, which a level's search overrides at `go` and
    /// which apply again once it is off.
    fn set_limit_strength(&mut self, value: &str) {
        if value.eq_ignore_ascii_case("true") {
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
        // Only a regular file is opened: a named pipe holds the open until something writes to it.
        let loaded = std::fs::metadata(&self.nnue.file)
            .map_err(|e| e.to_string())
            .and_then(|meta| {
                if meta.is_file() {
                    Ok(())
                } else {
                    Err("not a regular file".to_string())
                }
            })
            .and_then(|()| {
                std::fs::File::open(&self.nnue.file)
                    .and_then(|file| Ok((file.metadata()?.len(), file)))
                    .map_err(|e| e.to_string())
            })
            .and_then(|(size, file)| read_net(file, size));
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

    /// The table holds scores and static evaluations, so another evaluator starts from an empty
    /// one, as `ucinewgame` does.
    fn evaluator_changed(&mut self, before: Option<[u8; 32]>) {
        if self.evaluator() != before {
            self.stop_search();
            self.tt.clear();
            self.clear_learned();
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
            Some(table) => {
                self.tt = Arc::new(table);
                self.clear_learned();
            }
            None => say(format_args!(
                "info string setoption Hash: {mb} MB could not be allocated, keeping {} MB",
                self.tt.bytes() >> 20
            )),
        }
    }
}

/// The net in `file`, whose size is `size` bytes, and the SHA-256 of its bytes. The header is
/// read and checked first, so a foreign file or one of the wrong size is refused unread.
fn read_net(file: impl Read, size: u64) -> Result<(AnyNetwork, [u8; 32]), String> {
    let mut file = file.take(HEADER_LEN as u64);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let want = AnyNetwork::file_len(&bytes).map_err(|e| e.to_string())?;
    let got = usize::try_from(size).unwrap_or(usize::MAX);
    if got != want {
        return Err(LoadError::Length { want, got }.to_string());
    }
    let rest = want - bytes.len();
    file.set_limit(rest as u64);
    bytes.reserve_exact(rest);
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    AnyNetwork::from_bytes(&bytes)
        .map(|net| (net, sha256(&bytes)))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};
    use std::path::PathBuf;

    use cadence_dataset::sha256::sha256;
    use cadence_nnue::{Network, Provenance};

    use super::{HEADER_LEN, Session, read_net};

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

    /// Serves its bytes and fails the test if asked for more, so a refusal is shown to read no
    /// further than the header.
    struct HeaderOnly(Cursor<Vec<u8>>);

    impl Read for HeaderOnly {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.read(buf)?;
            assert!(n > 0 || buf.is_empty(), "read past the header");
            Ok(n)
        }
    }

    /// A file that is not a net is named as one from its first bytes, however large it is.
    #[test]
    fn a_foreign_file_is_named_from_its_header() {
        let foreign = HeaderOnly(Cursor::new(vec![b'x'; HEADER_LEN]));
        let refused = read_net(foreign, 1 << 30).err();
        assert_eq!(refused.as_deref(), Some("not a Cadence net file"));
    }

    /// A net file of another size than its header names is refused before the rest is read.
    #[test]
    fn a_file_of_the_wrong_size_is_refused_before_it_is_read() {
        let bytes = Network::seeded(7, Provenance::SelfPlay).to_bytes();
        let header = HeaderOnly(Cursor::new(bytes[..HEADER_LEN].to_vec()));
        let refused = read_net(header, 1 << 40).err().unwrap_or_default();
        assert!(
            refused.starts_with("1099511627776 bytes where a net is "),
            "{refused}"
        );
        let whole = u64::try_from(bytes.len()).expect("a net's length");
        let (_, key) = read_net(Cursor::new(bytes.clone()), whole).expect("the whole net loads");
        assert_eq!(key, sha256(&bytes));
    }
}
