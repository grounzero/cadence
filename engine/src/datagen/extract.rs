// SPDX-License-Identifier: GPL-3.0-or-later

//! Split into training and holdout by game number. A position is kept if its ply is inside the
//! window, it is not in check, its move is not noisy, and its search finished depth 1 with no mate.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};

use cadence_core::{FenStyle, PieceType};
use cadence_dataset::sha256::{Sha256, hex, sha256};

use super::game::{Ending, Outcome, ending};
use super::opening::{RANDOM_PLIES, SCREEN_NODES, WINDOW};
use super::record::Record;
use super::{HASH_MB, NODES};
use crate::eval::{self, PHASE_MAX};
use crate::score;
use crate::tt::Table;
use crate::version::VERSION;

/// Plies of the game proper skipped before the window opens. The random plies are not in a
/// record's moves, so they are not among these.
pub const FIRST_PLY: usize = 8;

pub const LAST_PLY: usize = 240;

/// In pawns; the ends hold everything past them.
pub const BALANCE_CAP: i32 = 9;

/// The last band holds everything past it.
pub const PLY_BAND: usize = 10;

pub const PLY_BANDS: usize = 30;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub games: u64,
    pub refused: u64,
    pub visited: u64,
    pub outside: u64,
    pub in_check: u64,
    pub noisy: u64,
    pub no_score: u64,
    pub mate: u64,
    pub train: u64,
    pub holdout: u64,
    /// In the order `Ending::word` lists them.
    pub endings: [u64; 5],
    /// White wins, Black wins, draws.
    pub outcomes: [u64; 3],
    pub phase: Vec<u64>,
    /// Offset by [`BALANCE_CAP`].
    pub balance: Vec<u64>,
    /// The random plies not counted.
    pub ply: Vec<u64>,
    pub longest: usize,
    /// Of the input's bytes as read.
    pub input_sha256: [u8; 32],
    /// Every version a provenance line names.
    pub versions: BTreeSet<String>,
}

/// Every `every`-th game by number goes to `holdout`. Comment lines go to both, so the data sets
/// carry the run's provenance, and the input's SHA-256 closes both.
///
/// # Errors
///
/// A record that does not parse or whose moves do not end as it says, an output that cannot be
/// written, a record before any provenance line, a run not holding every game it names by the next
/// provenance line or the end, a game out of turn in its run, a provenance line that is not this
/// build's, a seed or a game read twice, or a game 0 of this build's that does not replay.
pub fn extract(
    input: impl Read,
    train: &mut dyn Write,
    holdout: &mut dyn Write,
    every: u64,
) -> Result<Stats, String> {
    let mut stats = Stats {
        phase: vec![0; PHASE_MAX as usize + 1],
        balance: vec![0; 2 * BALANCE_CAP as usize + 1],
        ply: vec![0; PLY_BANDS],
        ..Stats::default()
    };
    let mut hashed = Hashed {
        inner: input,
        hasher: Sha256::new(),
    };
    let mut seen = Seen::default();
    let mut table = None;
    let mut named = None;
    let mut run: Option<Run> = None;
    for (i, line) in BufReader::new(&mut hashed).lines().enumerate() {
        let at = |e: String| format!("line {}: {e}", i + 1);
        let line = line.map_err(|e| at(e.to_string()))?;
        if line.starts_with('#') {
            if let Some(header) = Header::parse(&line).map_err(at)? {
                if let Some(done) = &run {
                    done.finished().map_err(at)?;
                }
                seen.seed(header.seed, i + 1).map_err(at)?;
                stats.versions.insert(header.version.clone());
                // Summed too, a second check that runs joined end to end hold every game.
                named = Some(named.unwrap_or(0) + header.games);
                run = Some(Run {
                    line: i + 1,
                    header,
                    due: 0,
                });
            }
            writeln!(train, "{line}").map_err(|e| e.to_string())?;
            writeln!(holdout, "{line}").map_err(|e| e.to_string())?;
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let Some(current) = &mut run else {
            return Err(at("a record before any provenance line".to_string()));
        };
        let record = Record::parse(&line).map_err(at)?;
        replay(&record).map_err(at)?;
        current.admit(record.number).map_err(at)?;
        seen.game(&record, i + 1).map_err(at)?;
        if record.number == 0 && current.header.version == VERSION {
            claim(&current.header, &record, &mut table).map_err(at)?;
        }
        let held = every > 0 && record.number % every == 0;
        if held {
            stats.holdout += positions(&record, &mut stats, holdout).map_err(|e| e.to_string())?;
        } else {
            stats.train += positions(&record, &mut stats, train).map_err(|e| e.to_string())?;
        }
    }
    if let Some(last) = &run {
        last.finished()
            .map_err(|e| format!("the end of the input: {e}"))?;
    }
    if let Some(n) = named
        && n != stats.games
    {
        return Err(format!(
            "the provenance names {n} games and the file holds {}",
            stats.games
        ));
    }
    stats.input_sha256 = hashed.hasher.finish();
    let closing = format!(
        "# cadence extract input-sha256 {}",
        hex(&stats.input_sha256)
    );
    writeln!(train, "{closing}").map_err(|e| e.to_string())?;
    writeln!(holdout, "{closing}").map_err(|e| e.to_string())?;
    Ok(stats)
}

/// Hashes what it passes on, so the digest is of the bytes as read.
struct Hashed<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> Read for Hashed<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

/// The seeds and the games read so far, each with the line it was read at.
#[derive(Default)]
struct Seen {
    seeds: BTreeMap<u64, usize>,
    games: BTreeMap<[u8; 16], usize>,
}

impl Seen {
    fn seed(&mut self, seed: u64, line: usize) -> Result<(), String> {
        if let Some(first) = self.seeds.get(&seed) {
            return Err(format!(
                "seed {seed} is the seed of the run at line {first}, so its games repeat"
            ));
        }
        self.seeds.insert(seed, line);
        Ok(())
    }

    fn game(&mut self, record: &Record, line: usize) -> Result<(), String> {
        let key = content_key(record);
        if let Some(first) = self.games.get(&key) {
            return Err(format!("the same game as line {first}"));
        }
        self.games.insert(key, line);
        Ok(())
    }
}

/// The game itself, its arrays, random plies and moves, so a copy is found whatever its number,
/// labels or run.
fn content_key(record: &Record) -> [u8; 16] {
    let mut text = format!("{} {}", record.white, record.black);
    for m in &record.plies {
        text.push(' ');
        text.push_str(&m.to_uci_chess960());
    }
    text.push_str(" |");
    for (m, _) in &record.moves {
        text.push(' ');
        text.push_str(&m.to_uci_chess960());
    }
    let digest = sha256(text.as_bytes());
    let mut key = [0; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

/// Game 0 played again by this build from the seed its run names must be the record, which no
/// constant check can show of an edited seed.
fn claim(header: &Header, record: &Record, table: &mut Option<Table>) -> Result<(), String> {
    if table.is_none() {
        *table = Table::new(HASH_MB);
    }
    let tt = table.as_ref().ok_or("no table to replay game 0 in")?;
    if super::play(header.seed, 0, tt) == *record {
        Ok(())
    } else {
        Err(format!("game 0 does not replay from seed {}", header.seed))
    }
}

/// The result labels every position the game gives, so the moves must end there and nothing may
/// follow the end.
fn replay(record: &Record) -> Result<(), String> {
    let mut board = record.start();
    let plies = record.plies.iter();
    for &m in plies.chain(record.moves.iter().map(|(m, _)| m)) {
        if let Some((end, _)) = ending(&board) {
            return Err(format!("a move after the game ended by {}", end.word()));
        }
        board.play(m);
    }
    match ending(&board) {
        Some(found) if found == (record.ending, record.outcome) => Ok(()),
        Some((end, won)) => Err(format!("the moves end {} {}", won.text(), end.word())),
        None => Err("the moves do not end the game".to_string()),
    }
}

/// What a provenance line names, where it stands, and the game due next under it.
struct Run {
    line: usize,
    header: Header,
    due: u64,
}

impl Run {
    /// From 0 in order, and no more than the line names.
    fn admit(&mut self, number: u64) -> Result<(), String> {
        let (games, due) = (self.header.games, self.due);
        if due >= games {
            return Err(format!("game {number} past the {games} its run names"));
        }
        if number != due {
            return Err(format!("game {number} where game {due} is due"));
        }
        self.due += 1;
        Ok(())
    }

    /// Every game the line names, read by the next provenance line and at the end.
    fn finished(&self) -> Result<(), String> {
        if self.due == self.header.games {
            return Ok(());
        }
        Err(format!(
            "the run at line {} names {} games and holds {}",
            self.line, self.header.games, self.due
        ))
    }
}

/// The words a provenance line opens with, as `provenance` writes them.
const PROVENANCE: [&str; 3] = ["#", "cadence", "datagen"];

/// The fields after `self-play`, in the order `provenance` writes them.
const FIELDS: [&str; 7] = [
    "seed", "games", "nodes", "hash", "plies", "window", "screen",
];

/// A run's provenance line.
struct Header {
    version: String,
    seed: u64,
    games: u64,
}

impl Header {
    /// `None` for any other comment. A line opening as provenance must hold every field once in
    /// `provenance`'s order, each setting this build's, or a header cut short or edited passes.
    fn parse(line: &str) -> Result<Option<Header>, String> {
        let mut words = line.split(' ');
        if !PROVENANCE.iter().all(|&w| words.next() == Some(w)) {
            return Ok(None);
        }
        let refused = || format!("a provenance line that does not parse: {line}");
        let version = words.next().filter(|v| !v.is_empty()).ok_or_else(refused)?;
        if words.next() != Some("self-play") {
            return Err(refused());
        }
        let mut values = [0_u64; FIELDS.len()];
        for (name, value) in FIELDS.iter().zip(&mut values) {
            if words.next() != Some(*name) {
                return Err(refused());
            }
            *value = words
                .next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(refused)?;
        }
        if words.next().is_some() {
            return Err(refused());
        }
        let [seed, games, nodes, hash, plies, window, screen] = values;
        for (name, named, built) in [
            ("nodes", nodes, NODES),
            ("hash", hash, HASH_MB as u64),
            ("plies", plies, RANDOM_PLIES as u64),
            ("window", window, u64::from(WINDOW.unsigned_abs())),
            ("screen", screen, SCREEN_NODES),
        ] {
            if named != built {
                return Err(format!(
                    "the provenance names {name} {named} and this build's is {built}"
                ));
            }
        }
        Ok(Some(Header {
            version: version.to_string(),
            seed,
            games,
        }))
    }
}

fn positions(record: &Record, stats: &mut Stats, out: &mut dyn Write) -> std::io::Result<u64> {
    stats.games += 1;
    stats.refused += u64::from(record.refused);
    stats.endings[ending_index(record.ending)] += 1;
    stats.outcomes[match record.outcome {
        Outcome::White => 0,
        Outcome::Black => 1,
        Outcome::Draw => 2,
    }] += 1;
    stats.longest = stats.longest.max(record.moves.len());
    let result = record.outcome.text();
    let mut board = record.start();
    for &m in &record.plies {
        board.play(m);
    }
    let mut kept = 0;
    for (ply, &(m, score)) in record.moves.iter().enumerate() {
        stats.visited += 1;
        if !(FIRST_PLY..=LAST_PLY).contains(&ply) {
            stats.outside += 1;
        } else if board.in_check() {
            stats.in_check += 1;
        } else if m.is_noisy() {
            stats.noisy += 1;
        } else if score.is_none() {
            stats.no_score += 1;
        } else if score.is_some_and(score::is_mate) {
            stats.mate += 1;
        } else {
            writeln!(out, "{} | {result}", board.to_fen(FenStyle::Shredder))?;
            kept += 1;
            stats.phase[eval::phase(&board) as usize] += 1;
            stats.balance
                [(balance(&board).clamp(-BALANCE_CAP, BALANCE_CAP) + BALANCE_CAP) as usize] += 1;
            stats.ply[(ply / PLY_BAND).min(PLY_BANDS - 1)] += 1;
        }
        board.play(m);
    }
    Ok(kept)
}

fn balance(board: &cadence_core::position::Board) -> i32 {
    let values = [
        (PieceType::Pawn, 1),
        (PieceType::Knight, 3),
        (PieceType::Bishop, 3),
        (PieceType::Rook, 5),
        (PieceType::Queen, 9),
    ];
    let white = board.by_colour(cadence_core::Colour::White);
    values
        .iter()
        .map(|&(pt, v)| {
            let all = board.by_type(pt);
            let ours = (all & white).count();
            let theirs = all.count() - ours;
            v * (i32::try_from(ours).unwrap_or(0) - i32::try_from(theirs).unwrap_or(0))
        })
        .sum()
}

fn ending_index(ending: Ending) -> usize {
    match ending {
        Ending::Mate => 0,
        Ending::Stalemate => 1,
        Ending::Repetition => 2,
        Ending::FiftyMoves => 3,
        Ending::InsufficientMaterial => 4,
    }
}

impl Stats {
    /// # Errors
    ///
    /// If `out` cannot be written.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a position count is far below 2^52"
    )]
    pub fn report(&self, out: &mut dyn Write) -> std::io::Result<()> {
        let kept = self.train + self.holdout;
        let per_game = |n: u64| n as f64 / self.games.max(1) as f64;
        writeln!(out, "input-sha256 {}", hex(&self.input_sha256))?;
        let versions: Vec<&str> = self.versions.iter().map(String::as_str).collect();
        writeln!(out, "versions {}", versions.join(" "))?;
        writeln!(out, "games {} refused-starts {}", self.games, self.refused)?;
        writeln!(
            out,
            "endings mate {} stalemate {} repetition {} fifty {} material {}",
            self.endings[0], self.endings[1], self.endings[2], self.endings[3], self.endings[4]
        )?;
        writeln!(
            out,
            "outcomes white {} black {} draw {}",
            self.outcomes[0], self.outcomes[1], self.outcomes[2]
        )?;
        writeln!(
            out,
            "plies mean {:.1} longest {}",
            per_game(self.visited),
            self.longest
        )?;
        writeln!(
            out,
            "visited {} outside-plies-{FIRST_PLY}-{LAST_PLY} {} in-check {} noisy {} no-score {} \
             mate {}",
            self.visited, self.outside, self.in_check, self.noisy, self.no_score, self.mate
        )?;
        writeln!(
            out,
            "kept {kept} train {} holdout {} per-game {:.1} yield {:.3}",
            self.train,
            self.holdout,
            per_game(kept),
            kept as f64 / self.visited.max(1) as f64
        )?;
        let share = |n: u64| 100.0 * n as f64 / kept.max(1) as f64;
        writeln!(out, "phase (0 bare, {PHASE_MAX} full)")?;
        for (p, &n) in self.phase.iter().enumerate() {
            writeln!(out, "  {p:>3} {n:>10} {:>6.2}%", share(n))?;
        }
        writeln!(out, "balance (White's lead in pawns)")?;
        for (i, &n) in self.balance.iter().enumerate() {
            let lead = i32::try_from(i).unwrap_or(i32::MAX) - BALANCE_CAP;
            let edge = if lead.abs() == BALANCE_CAP { "+" } else { " " };
            writeln!(out, "  {lead:>+3}{edge} {n:>10} {:>6.2}%", share(n))?;
        }
        writeln!(out, "ply of the game proper")?;
        for (i, &n) in self.ply.iter().enumerate() {
            let from = i * PLY_BAND;
            let band = if i + 1 == PLY_BANDS {
                format!("{from}+")
            } else {
                format!("{from}-{}", from + PLY_BAND - 1)
            };
            writeln!(out, "  {band:>7} {n:>10} {:>6.2}%", share(n))?;
        }
        Ok(())
    }
}
