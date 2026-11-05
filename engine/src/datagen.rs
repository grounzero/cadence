// SPDX-License-Identifier: GPL-3.0-or-later

//! Every game is seeded by its number, so a run's file is the same whatever the thread count and
//! whichever thread finished first.

pub mod extract;
pub mod game;
pub mod opening;
pub mod record;

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use cadence_core::rng::{Rng, splitmix64};

use crate::tt::Table;
use crate::uci::SEARCH_STACK_BYTES;
use crate::version::VERSION;
use record::Record;

/// Compiled in, like `bench`'s depth, so a record's provenance line names every setting that shaped
/// it.
pub const NODES: u64 = 5_000;

/// Cleared at the start of every game.
pub const HASH_MB: usize = 16;

pub const HOLDOUT_EVERY: u64 = 10;

/// Hashed twice, so runs with different seeds share a game only by a 64-bit coincidence.
#[must_use]
pub fn game_seed(seed: u64, number: u64) -> u64 {
    splitmix64(splitmix64(seed).0 ^ number).0
}

/// A function of `seed` and `number` alone, whatever `tt` held before.
#[must_use]
pub fn play(seed: u64, number: u64, tt: &Table) -> Record {
    let mut rng = Rng::new(game_seed(seed, number));
    let (mut opening, refused) = opening::next(&mut rng, tt);
    let played = game::play(&mut opening.board, NODES, tt);
    Record::new(number, &opening, played, refused)
}

/// Everything that shaped the games.
#[must_use]
pub fn provenance(seed: u64, games: u64) -> String {
    format!(
        "# cadence datagen {VERSION} self-play seed {seed} games {games} nodes {NODES} \
         hash {HASH_MB} plies {} window {} screen {}",
        opening::RANDOM_PLIES,
        opening::WINDOW,
        opening::SCREEN_NODES
    )
}

/// Records are written in number order; `progress` is called from the writing thread.
///
/// # Errors
///
/// If `out` cannot be written, a worker cannot be started or its table cannot be allocated.
pub fn generate(
    seed: u64,
    games: u64,
    threads: usize,
    out: &mut dyn Write,
    progress: &mut dyn FnMut(u64),
) -> io::Result<()> {
    writeln!(out, "{}", provenance(seed, games))?;
    let next = AtomicU64::new(0);
    let (tx, rx) = mpsc::channel::<Option<(u64, String)>>();
    thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            let tx = tx.clone();
            let next = &next;
            // The stack a `go` gives its search, since these games are the same search.
            let spawned = thread::Builder::new()
                .stack_size(SEARCH_STACK_BYTES)
                .spawn_scoped(scope, move || {
                    let Some(tt) = Table::new(HASH_MB) else {
                        let _ = tx.send(None);
                        return;
                    };
                    loop {
                        let number = next.fetch_add(1, Ordering::Relaxed);
                        if number >= games {
                            return;
                        }
                        let line = play(seed, number, &tt).line();
                        if tx.send(Some((number, line))).is_err() {
                            return;
                        }
                    }
                });
            if let Err(e) = spawned {
                // The workers already started take no new game, so the scope ends soon.
                next.store(games, Ordering::Relaxed);
                return Err(e);
            }
        }
        drop(tx);
        // Records arrive in whatever order the workers finish, and leave in number order.
        let mut pending = BTreeMap::new();
        let mut wanted = 0;
        for message in rx {
            let Some((number, line)) = message else {
                return Err(io::Error::other("a worker's table could not be allocated"));
            };
            pending.insert(number, line);
            while let Some(line) = pending.remove(&wanted) {
                writeln!(out, "{line}")?;
                wanted += 1;
                progress(wanted);
            }
        }
        out.flush()
    })
}

const USAGE: &str = "usage: cadence datagen play --games N [--seed S] [--threads N] [--out PATH]\n       \
     cadence datagen extract <games> --train PATH --holdout PATH [--every N]";

#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    let result = match args.first().map(String::as_str) {
        Some("play") => play_from_args(&args[1..]),
        Some("extract") => extract_from_args(&args[1..]),
        _ => Err("name play or extract".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cadence datagen: {e}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

type Split = (Vec<(String, String)>, Vec<String>);

fn split(args: &[String]) -> Result<Split, String> {
    let (mut flags, mut rest) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg.starts_with("--") {
            let value = it.next().ok_or_else(|| format!("{arg} takes a value"))?;
            flags.push((arg.clone(), value.clone()));
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((flags, rest))
}

fn number(name: &str, value: &str) -> Result<u64, String> {
    value.parse().map_err(|_| format!("bad {name} `{value}`"))
}

fn play_from_args(args: &[String]) -> Result<(), String> {
    let (flags, rest) = split(args)?;
    if let Some(extra) = rest.first() {
        return Err(format!("unexpected argument {extra}"));
    }
    let (mut games, mut seed, mut threads, mut path) = (None, 0, 1, None);
    for (name, value) in &flags {
        match name.as_str() {
            "--games" => games = Some(number(name, value)?),
            "--seed" => seed = number(name, value)?,
            "--threads" => {
                threads = usize::try_from(number(name, value)?).map_err(|_| "bad --threads")?;
            }
            "--out" => path = Some(value.clone()),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let games = games.ok_or("--games is required")?;
    let mut staged = match path.as_deref().map(Path::new) {
        Some(out) => Some(Staged::create(out, stage_name(out)?)?),
        None => None,
    };
    let mut stdout = BufWriter::new(io::stdout().lock());
    let out: &mut dyn Write = match &mut staged {
        Some(file) => file,
        None => &mut stdout,
    };
    let start = Instant::now();
    let mut progress = |written: u64| {
        if written.is_multiple_of(100) || written == games {
            let secs = start.elapsed().as_secs_f64();
            #[expect(clippy::cast_precision_loss, reason = "a game count is far below 2^52")]
            let rate = written as f64 / secs.max(1e-9);
            eprintln!("games {written}/{games} seconds {secs:.1} games-per-second {rate:.2}");
        }
    };
    generate(seed, games, threads, out, &mut progress).map_err(|e| e.to_string())?;
    staged.map_or(Ok(()), |stage| publish(vec![stage]))
}

fn extract_from_args(args: &[String]) -> Result<(), String> {
    let (flags, rest) = split(args)?;
    let [input] = &rest[..] else {
        return Err("name one record file".to_string());
    };
    let (mut train, mut holdout, mut every) = (None, None, HOLDOUT_EVERY);
    for (name, value) in &flags {
        match name.as_str() {
            "--train" => train = Some(value.clone()),
            "--holdout" => holdout = Some(value.clone()),
            "--every" => every = number(name, value)?,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let train = train.ok_or("--train is required")?;
    let holdout = holdout.ok_or("--holdout is required")?;
    let (train_path, holdout_path) = (Path::new(&train), Path::new(&holdout));
    let (train_temp, holdout_temp) = (stage_name(train_path)?, stage_name(holdout_path)?);
    distinct(&[
        Path::new(input),
        train_path,
        holdout_path,
        train_temp.as_path(),
        holdout_temp.as_path(),
    ])?;
    let input = BufReader::new(File::open(input).map_err(|e| format!("{input}: {e}"))?);
    let mut train = Staged::create(train_path, train_temp)?;
    let mut holdout = Staged::create(holdout_path, holdout_temp)?;
    let stats = extract::extract(input, &mut train, &mut holdout, every)?;
    publish(vec![train, holdout])?;
    stats
        .report(&mut io::stdout().lock())
        .map_err(|e| e.to_string())
}

/// Refused before anything is created: an output or a stage that is the input, or another output
/// or stage, would be written or published over it.
fn distinct(paths: &[&Path]) -> Result<(), String> {
    let resolved = paths
        .iter()
        .map(|&p| resolve(p))
        .collect::<Result<Vec<_>, _>>()?;
    for (i, a) in resolved.iter().enumerate() {
        if let Some(j) = resolved[i + 1..].iter().position(|b| same_file(a, b)) {
            return Err(format!(
                "{} and {} name one file",
                paths[i].display(),
                paths[i + 1 + j].display()
            ));
        }
    }
    Ok(())
}

/// Where `path` is or would be: itself resolved where it exists, else its directory resolved and
/// its name added, so two spellings of one file resolve alike.
fn resolve(path: &Path) -> Result<PathBuf, String> {
    let failed = |e: io::Error| format!("{}: {e}", path.display());
    if path.exists() {
        return std::fs::canonicalize(path).map_err(failed);
    }
    let name = path
        .file_name()
        .ok_or_else(|| format!("{}: not a file name", path.display()))?;
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    Ok(std::fs::canonicalize(dir).map_err(failed)?.join(name))
}

/// Spellings differing only in case are one file where the file system ignores case, as macOS and
/// Windows do by default.
fn same_file(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
    } else {
        a == b
    }
}

/// Beside `path`, named by this process and a count, so no two stages share a name and an earlier
/// run's leftover is refused by the creation rather than written over.
fn stage_name(path: &Path) -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| format!("{}: not a file name", path.display()))?;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut temp = name.to_os_string();
    temp.push(format!(".{}.{n}.partial", std::process::id()));
    Ok(path.with_file_name(temp))
}

/// A file written under a temporary name beside its own and renamed to it only once complete, so
/// a stage that fails leaves nothing a later one could read as whole.
struct Staged {
    path: PathBuf,
    temp: PathBuf,
    file: Option<BufWriter<File>>,
    /// Until it is, dropping the stage removes its file, written out or not.
    published: bool,
}

impl Staged {
    /// Created new, so a stage can never be a file already there.
    fn create(path: &Path, temp: PathBuf) -> Result<Staged, String> {
        let file = File::options()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| format!("{}: {e}", temp.display()))?;
        Ok(Staged {
            path: path.to_path_buf(),
            temp,
            file: Some(BufWriter::new(file)),
            published: false,
        })
    }

    /// Written out and closed, which some platforms require of a file before it is renamed.
    fn close(&mut self) -> Result<(), String> {
        if let Some(file) = self.file.take() {
            file.into_inner()
                .map_err(|e| format!("{}: {}", self.temp.display(), e.error()))?;
        }
        Ok(())
    }

    fn rename(mut self) -> Result<(), String> {
        std::fs::rename(&self.temp, &self.path)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.published = true;
        Ok(())
    }

    fn open(&mut self) -> io::Result<&mut BufWriter<File>> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("written after it was finished"))
    }
}

impl Write for Staged {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.open()?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.open()?.flush()
    }
}

impl Drop for Staged {
    /// The file is closed before it is removed, which some platforms require.
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.published {
            let _ = std::fs::remove_file(&self.temp);
        }
    }
}

/// Every stage is written out and every destination checked before any is renamed, so one that
/// cannot be published leaves the others' destinations as they were.
fn publish(mut stages: Vec<Staged>) -> Result<(), String> {
    for stage in &mut stages {
        stage.close()?;
    }
    if let Some(stage) = stages.iter().find(|stage| stage.path.is_dir()) {
        return Err(format!("{}: a directory, not a file", stage.path.display()));
    }
    for stage in stages {
        stage.rename()?;
    }
    Ok(())
}
