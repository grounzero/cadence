// SPDX-License-Identifier: GPL-3.0-or-later

//! Every game is seeded by its number, so a run's file is the same whatever the thread count and
//! whichever thread finished first.

pub mod extract;
pub mod game;
pub mod opening;
pub mod record;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, LockResult, Mutex, PoisonError, mpsc};
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

/// The most workers a run starts.
pub const MAX_THREADS: usize = 256;

/// The most games past the next one to be written that a worker may start, which bounds the records
/// held for reordering; four times `MAX_THREADS`, so a run nothing delays never waits on it.
pub const AHEAD: u64 = 1_024;

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
    let line_for = |number: u64, tt: &Table| play(seed, number, tt).line();
    generate_with(seed, games, threads, AHEAD, out, progress, &line_for)
}

/// `line_for` makes a game's record line, so a test can hold a game back.
fn generate_with(
    seed: u64,
    games: u64,
    threads: usize,
    ahead: u64,
    out: &mut dyn Write,
    progress: &mut dyn FnMut(u64),
    line_for: &(dyn Fn(u64, &Table) -> String + Sync),
) -> io::Result<()> {
    writeln!(out, "{}", provenance(seed, games))?;
    let next = AtomicU64::new(0);
    let window = Window::new(ahead);
    let (tx, rx) = mpsc::channel::<Option<(u64, String)>>();
    thread::scope(|scope| {
        let _closing = Closing(&window);
        for _ in 0..workers(threads, games) {
            let tx = tx.clone();
            let next = &next;
            let window = &window;
            let spawned = spawn_search(scope, move || {
                let Some(tt) = Table::new(HASH_MB) else {
                    let _ = tx.send(None);
                    return;
                };
                loop {
                    let number = next.fetch_add(1, Ordering::Relaxed);
                    if number >= games || !window.wait_for(number) {
                        return;
                    }
                    let line = line_for(number, &tt);
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
            window.advance(wanted);
        }
        out.flush()
    })
}

/// Every thread that searches, on the stack a `go` gives its search.
///
/// # Errors
///
/// If the thread cannot be started.
pub fn spawn_search<'scope, T: Send + 'scope>(
    scope: &'scope thread::Scope<'scope, '_>,
    work: impl FnOnce() -> T + Send + 'scope,
) -> io::Result<thread::ScopedJoinHandle<'scope, T>> {
    thread::Builder::new()
        .stack_size(SEARCH_STACK_BYTES)
        .spawn_scoped(scope, work)
}

/// How far the workers may run ahead of the next record to be written.
struct Window {
    ahead: u64,
    state: Mutex<WindowState>,
    moved: Condvar,
}

struct WindowState {
    written: u64,
    closed: bool,
}

impl Window {
    /// At least one, or the next game to be written would wait on itself.
    fn new(ahead: u64) -> Window {
        Window {
            ahead: ahead.max(1),
            state: Mutex::new(WindowState {
                written: 0,
                closed: false,
            }),
            moved: Condvar::new(),
        }
    }

    /// The next record to be written is always inside, so its game never waits and the window
    /// moves. False once the writer has stopped, when no game is worth starting.
    fn wait_for(&self, number: u64) -> bool {
        let mut state = unpoisoned(self.state.lock());
        while !state.closed && number >= state.written.saturating_add(self.ahead) {
            state = unpoisoned(self.moved.wait(state));
        }
        !state.closed
    }

    fn advance(&self, written: u64) {
        unpoisoned(self.state.lock()).written = written;
        self.moved.notify_all();
    }

    fn close(&self) {
        unpoisoned(self.state.lock()).closed = true;
        self.moved.notify_all();
    }
}

/// The lock guards two plain fields, which no panic can leave half-written.
fn unpoisoned<T>(result: LockResult<T>) -> T {
    result.unwrap_or_else(PoisonError::into_inner)
}

/// On every way out of the writer, so no worker waits on a window that will not move again.
struct Closing<'a>(&'a Window);

impl Drop for Closing<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// Each worker holds a stack and a table before it asks for a game, so none start for no games.
fn workers(threads: usize, games: u64) -> usize {
    threads
        .clamp(1, MAX_THREADS)
        .min(usize::try_from(games).unwrap_or(usize::MAX))
}

const USAGE: &str = "usage: cadence datagen play --games N --seed S [--threads N] [--out PATH]\n       \
     cadence datagen extract <games> --train PATH --holdout PATH [--every N]\n\
     --threads is cut to the number of games and to 256";

/// Only a command line that does not parse is answered with the usage.
enum Failure {
    Usage(String),
    Work(String),
}

#[must_use]
pub fn run(args: &[String]) -> ExitCode {
    let result = match args.first().map(String::as_str) {
        Some("play") => play_from_args(&args[1..]),
        Some("extract") => extract_from_args(&args[1..]),
        _ => Err(Failure::Usage("name play or extract".to_string())),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(e)) => {
            eprintln!("cadence datagen: {e}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Work(e)) => {
            eprintln!("cadence datagen: {e}");
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

struct PlayArgs {
    games: u64,
    seed: u64,
    threads: usize,
    path: Option<String>,
}

fn play_args(args: &[String]) -> Result<PlayArgs, String> {
    let (flags, rest) = split(args)?;
    if let Some(extra) = rest.first() {
        return Err(format!("unexpected argument {extra}"));
    }
    let (mut games, mut seed, mut threads, mut path) = (None, None, 1, None);
    for (name, value) in &flags {
        match name.as_str() {
            "--games" => games = Some(number(name, value)?),
            "--seed" => seed = Some(number(name, value)?),
            "--threads" => {
                threads = usize::try_from(number(name, value)?).map_err(|_| "bad --threads")?;
            }
            "--out" => path = Some(value.clone()),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(PlayArgs {
        games: games.ok_or("--games is required")?,
        seed: seed
            .ok_or("--seed is required: a run without one repeats every other run's games")?,
        threads,
        path,
    })
}

fn play_from_args(args: &[String]) -> Result<(), Failure> {
    let args = play_args(args).map_err(Failure::Usage)?;
    play_to(&args).map_err(Failure::Work)
}

fn play_to(args: &PlayArgs) -> Result<(), String> {
    let PlayArgs {
        games,
        seed,
        threads,
        ref path,
    } = *args;
    if threads > MAX_THREADS {
        eprintln!("cadence datagen: --threads {threads} is cut to {MAX_THREADS}");
    }
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

fn extract_from_args(args: &[String]) -> Result<(), Failure> {
    let (input, train, holdout, every) = extract_args(args).map_err(Failure::Usage)?;
    extract_to(
        Path::new(&input),
        Path::new(&train),
        Path::new(&holdout),
        every,
        &mut io::stdout().lock(),
    )
    .map_err(Failure::Work)
}

fn extract_args(args: &[String]) -> Result<(String, String, String, u64), String> {
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
    Ok((input.clone(), train, holdout, every))
}

/// The report is written before the outputs are published, so a failure exits with nothing
/// published; `report` is the destination, so a test can make it fail.
fn extract_to(
    input: &Path,
    train_path: &Path,
    holdout_path: &Path,
    every: u64,
    report: &mut dyn Write,
) -> Result<(), String> {
    let (train_temp, holdout_temp) = (stage_name(train_path)?, stage_name(holdout_path)?);
    distinct(&[
        input,
        train_path,
        holdout_path,
        train_temp.as_path(),
        holdout_temp.as_path(),
    ])?;
    let file = File::open(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let mut train = Staged::create(train_path, train_temp)?;
    let mut holdout = Staged::create(holdout_path, holdout_temp)?;
    let stats = extract::extract(BufReader::new(file), &mut train, &mut holdout, every)?;
    let mut text = Vec::new();
    stats.report(&mut text).map_err(|e| e.to_string())?;
    if report
        .write_all(&text)
        .and_then(|()| report.flush())
        .is_err()
    {
        return Err("the report could not be written; nothing was published".to_string());
    }
    publish(vec![train, holdout])
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
    beside(path, "partial")
}

fn beside(path: &Path, kind: &str) -> Result<PathBuf, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| format!("{}: not a file name", path.display()))?;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut temp = name.to_os_string();
    temp.push(format!(".{}.{n}.{kind}", std::process::id()));
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

    /// A file already at the destination is moved aside first, and every step is recorded so a
    /// later failure can undo it.
    fn publish(&mut self, rename: &mut Rename<'_>, undo: &mut Vec<Undo>) -> Result<(), String> {
        let failed = |e: io::Error| format!("{}: {e}", self.path.display());
        if self.path.symlink_metadata().is_err() {
            rename(&self.temp, &self.path).map_err(failed)?;
            self.published = true;
            undo.push(Undo::Remove(self.path.clone()));
            return Ok(());
        }
        let aside = beside(&self.path, "previous")?;
        if aside.symlink_metadata().is_ok() {
            return Err(format!("{}: already there", aside.display()));
        }
        rename(&self.path, &aside).map_err(failed)?;
        undo.push(Undo::Restore {
            aside,
            path: self.path.clone(),
        });
        rename(&self.temp, &self.path).map_err(failed)?;
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

/// What puts one step of a publish back.
enum Undo {
    /// The destination's earlier file, moved aside, returns over whatever replaced it.
    Restore { aside: PathBuf, path: PathBuf },
    /// The destination had no file, so the one published there is removed.
    Remove(PathBuf),
}

/// `rename_file` outside the tests, which put a failure where they choose.
type Rename<'a> = dyn FnMut(&Path, &Path) -> io::Result<()> + 'a;

fn rename_file(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

fn publish(stages: Vec<Staged>) -> Result<(), String> {
    publish_with(stages, &mut rename_file)
}

/// Every stage is written out and every destination checked before any is renamed, and a rename
/// that fails undoes those before it, so the set is published whole or not at all.
fn publish_with(mut stages: Vec<Staged>, rename: &mut Rename<'_>) -> Result<(), String> {
    for stage in &mut stages {
        stage
            .close()
            .map_err(|e| format!("{e}; nothing was published"))?;
    }
    if let Some(stage) = stages.iter().find(|stage| stage.path.is_dir()) {
        return Err(format!(
            "{}: a directory, not a file; nothing was published",
            stage.path.display()
        ));
    }
    let mut undo = Vec::new();
    for stage in &mut stages {
        if let Err(e) = stage.publish(rename, &mut undo) {
            return Err(roll_back(undo, rename, e));
        }
    }
    for step in undo {
        if let Undo::Restore { aside, .. } = step {
            let _ = std::fs::remove_file(aside);
        }
    }
    Ok(())
}

/// Last step first, each tried whatever became of the one before, and any that fails named after
/// `error`; only a roll back with none failing says nothing was published.
fn roll_back(undo: Vec<Undo>, rename: &mut Rename<'_>, mut error: String) -> String {
    let mut whole = true;
    for step in undo.into_iter().rev() {
        match step {
            Undo::Restore { aside, path } => {
                if let Err(e) = rename(&aside, &path) {
                    let kept = format!("; {} kept as {}: {e}", path.display(), aside.display());
                    error.push_str(&kept);
                    whole = false;
                }
            }
            Undo::Remove(path) => {
                if let Err(e) = std::fs::remove_file(&path) {
                    let _ = write!(error, "; {} not removed: {e}", path.display());
                    whole = false;
                }
            }
        }
    }
    if whole {
        error.push_str("; nothing was published");
    }
    error
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::path::Path;
    use std::sync::Mutex;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        HASH_MB, MAX_THREADS, Staged, USAGE, extract_to, game_seed, generate, generate_with, play,
        provenance, publish_with, spawn_search, stage_name, workers,
    };
    use crate::tt::Table;

    /// A game that panics ends the run with an error naming it, at one thread and at three, rather
    /// than leaving the writer waiting for its record.
    #[test]
    fn a_panicking_game_ends_the_run_naming_it() {
        for threads in [1, 3] {
            let (tx, rx) = std::sync::mpsc::channel();
            thread::spawn(move || {
                let line_for = |number: u64, _: &Table| {
                    assert!(number != 3, "game 3 cannot be played");
                    format!("game {number}")
                };
                let mut out = Vec::new();
                let run = generate_with(1, 8, threads, 4, &mut out, &mut |_| {}, &line_for);
                let _ = tx.send(run.map_err(|e| e.to_string()));
            });
            let ended = rx.recv_timeout(Duration::from_secs(30));
            let run = ended.unwrap_or_else(|e| panic!("{threads} threads: no end: {e}"));
            let error = run.expect_err("a run with a panicking game succeeded");
            assert!(
                error.contains("game 3 panicked"),
                "{threads} threads: {error}"
            );
            assert!(
                error.contains("cannot be played"),
                "{threads} threads: {error}"
            );
        }
        let line_for = |number: u64, _: &Table| format!("game {number}");
        let mut out = Vec::new();
        generate_with(1, 8, 3, 4, &mut out, &mut |_| {}, &line_for).expect("the control");
    }

    /// The helper gives a search the stack a `go` gives it, and the game is the same game.
    #[test]
    fn a_game_on_a_search_thread_is_the_game_played_here() {
        let here = play(3, 1, &Table::new(HASH_MB).expect("a table"));
        let there = thread::scope(|scope| {
            let spawned =
                spawn_search(scope, || play(3, 1, &Table::new(HASH_MB).expect("a table")));
            spawned.expect("spawns").join().expect("joins")
        });
        assert_eq!(there, here);
    }

    /// A change to the derivation changes every dataset, so it fails here first.
    #[test]
    fn a_game_seed_is_pinned_to_its_derivation() {
        assert_eq!(game_seed(0, 0), 0xA706_DD2F_4D19_7E6F);
        assert_eq!(game_seed(5, 3), 0x67E0_7EA6_E630_C1F5);
    }

    /// The report is written before anything is published, so a report that fails leaves neither
    /// output nor a stage.
    #[test]
    fn a_report_that_cannot_be_written_publishes_nothing() {
        let dir = std::env::temp_dir().join(format!("cadence-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let games = dir.join("games.txt");
        let mut text = Vec::new();
        generate(9, 2, 1, &mut text, &mut |_| {}).expect("a run into memory");
        std::fs::write(&games, text).expect("writes");
        let (train, held) = (dir.join("train.txt"), dir.join("held.txt"));
        let refused = extract_to(&games, &train, &held, 10, &mut Closed);
        let left = listed(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            refused.is_err(),
            "a report nobody could read was taken as written"
        );
        assert_eq!(left, ["games.txt"]);
    }

    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }
    }

    #[test]
    fn workers_number_at_most_the_games_and_the_maximum() {
        assert_eq!(workers(100_000, 0), 0, "no games, no workers");
        assert_eq!(workers(100_000, 1), 1);
        assert_eq!(workers(100_000, 1_000_000), MAX_THREADS);
        assert_eq!(workers(0, 10), 1);
        assert_eq!(workers(4, 10), 4);
        assert!(USAGE.ends_with(&format!("and to {MAX_THREADS}")), "{USAGE}");
    }

    /// Game 0 is held until the others have started every game the window allows, and a while
    /// longer, so a worker that ran past the window shows in what had started.
    #[test]
    fn holding_game_zero_back_keeps_every_worker_inside_the_window() {
        const AHEAD_HERE: u64 = 8;
        const GAMES: u64 = 64;
        let started = Mutex::new(Vec::new());
        let held = Mutex::new(Vec::new());
        let line_for = |number: u64, _: &Table| {
            started.lock().expect("a lock").push(number);
            if number == 0 {
                let deadline = Instant::now() + Duration::from_secs(30);
                while (started.lock().expect("a lock").len() as u64) < AHEAD_HERE
                    && Instant::now() < deadline
                {
                    thread::sleep(Duration::from_millis(1));
                }
                thread::sleep(Duration::from_millis(200));
                let snapshot = started.lock().expect("a lock").clone();
                *held.lock().expect("a lock") = snapshot;
            }
            format!("game {number}")
        };
        let mut out = Vec::new();
        let run = generate_with(1, GAMES, 3, AHEAD_HERE, &mut out, &mut |_| {}, &line_for);
        run.expect("a run into memory");

        let mut held = held.into_inner().expect("a lock");
        held.sort_unstable();
        assert_eq!(
            held,
            (0..AHEAD_HERE).collect::<Vec<_>>(),
            "started while held"
        );
        let lines: Vec<&str> = std::str::from_utf8(&out).expect("utf-8").lines().collect();
        let names: Vec<String> = (0..GAMES).map(|n| format!("game {n}")).collect();
        assert_eq!(lines[1..], names, "written in number order");
    }

    /// The pair is published whole or not at all: a rename that fails, whichever it is, leaves both
    /// destinations as they were and nothing beside them.
    #[test]
    fn a_failed_rename_leaves_the_original_pair_untouched() {
        let dir = std::env::temp_dir();
        let dir = dir.join(format!("cadence-pair-{}", std::process::id()));
        let train = dir.join("train.txt");
        let holdout = dir.join("holdout.txt");
        let mut failed = 0;
        for fail_at in 0.. {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a scratch directory");
            std::fs::write(&train, "train 1").expect("writes");
            std::fs::write(&holdout, "holdout 1").expect("writes");
            let stages = vec![staged(&train, "train 2"), staged(&holdout, "holdout 2")];
            let mut calls = 0;
            let mut rename = |from: &Path, to: &Path| {
                calls += 1;
                if calls == fail_at + 1 {
                    return Err(io::Error::other("injected"));
                }
                std::fs::rename(from, to)
            };
            let published = publish_with(stages, &mut rename).is_ok();
            assert_eq!(
                listed(&dir),
                ["holdout.txt", "train.txt"],
                "call {fail_at} failing"
            );
            let pair = [read(&train), read(&holdout)];
            if published {
                assert_eq!(pair, ["train 2", "holdout 2"]);
                break;
            }
            failed += 1;
            assert_eq!(pair, ["train 1", "holdout 1"], "call {fail_at} failing");
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert!(failed >= 2, "the second rename was never made to fail");
    }

    fn staged(path: &Path, text: &str) -> Staged {
        let name = stage_name(path).expect("a stage name");
        let mut stage = Staged::create(path, name).expect("a stage");
        stage.write_all(text.as_bytes()).expect("writes");
        stage
    }

    fn listed(dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(dir).expect("lists") {
            let name = entry.expect("an entry").file_name();
            names.push(name.to_string_lossy().into_owned());
        }
        names.sort();
        names
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("reads")
    }

    /// A writer that fails releases every worker the window holds, so the run ends.
    #[test]
    fn a_failed_write_ends_a_run_whose_workers_are_held() {
        const GAMES: u64 = 64;
        let mut room = vec![0_u8; provenance(1, GAMES).len() + "\ngame 0\n".len()];
        let mut out: &mut [u8] = &mut room;
        let line_for = |number: u64, _: &Table| format!("game {number}");
        let run = generate_with(1, GAMES, 3, 2, &mut out, &mut |_| {}, &line_for);
        assert!(run.is_err(), "the second record did not fit");
    }
}
