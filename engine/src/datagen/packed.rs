// SPDX-License-Identifier: GPL-3.0-or-later

//! The packed writer: chunks of games under a run seed, closed whole, paused or stopped by files in
//! its directory. A kill leaves a `.tmp` of whole records that `repair` closes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cadence_core::rng::Rng;
use cadence_dataset::chunk::{
    Chunk, GameRecord, HEADER_LEN, Header, KIND_STANDARD, KIND_UNIFORM, NO_SCORE, SCORE_LIMIT,
    Trailer, chunk_name, encode_move, whole_records,
};
use cadence_dataset::crc32::crc32;
use cadence_dataset::durable::{self, Disk, Steps};
use cadence_dataset::sha256::{Sha256, hex, sha256};

use super::game::{self, Ending, Outcome};
use super::opening::{self, Probe};
use super::{
    AHEAD, HASH_MB, MAX_THREADS, NODES, game_seed, number, panic_text, spawn_search, split,
};
use crate::tt::Table;
use crate::version;

/// The chunk index's seed is the run seed's game seed at that index.
#[must_use]
pub fn chunk_seed(run_seed: u64, chunk_index: u32) -> u64 {
    game_seed(run_seed, u64::from(chunk_index))
}

/// The first draw of a game's stream, made whatever the share so the rest of the stream does not
/// depend on it.
pub fn start_kind(rng: &mut Rng, share: u8) -> u8 {
    if rng.below(100) < usize::from(share) {
        KIND_STANDARD
    } else {
        KIND_UNIFORM
    }
}

/// A function of the chunk seed, the index and the share alone, whatever `tt` held before.
#[must_use]
pub fn play(chunk_seed: u64, index: u32, share: u8, tt: &Table) -> GameRecord {
    play_with(chunk_seed, index, share, tt, &|board, tt| {
        game::probe(board, opening::SCREEN_NODES, tt)
    })
}

/// `probe` screens a start, so a test can refuse some.
///
/// # Panics
///
/// If a score lies outside the record's range, which no finished search gives.
pub fn play_with(
    chunk_seed: u64,
    index: u32,
    share: u8,
    tt: &Table,
    probe: &Probe<'_>,
) -> GameRecord {
    let seed = game_seed(chunk_seed, u64::from(index));
    let mut rng = Rng::new(seed);
    let kind = start_kind(&mut rng, share);
    let (mut start, refused) = opening::next_of_kind(&mut rng, tt, kind == KIND_STANDARD, probe);
    let played = game::play(&mut start.board, NODES, tt);
    let label = |score: Option<i32>| {
        score.map_or(NO_SCORE, |s| {
            i16::try_from(s)
                .ok()
                .filter(|s| s.abs() <= SCORE_LIMIT)
                .unwrap_or_else(|| panic!("a score of {s} outside the record's range"))
        })
    };
    GameRecord {
        index,
        seed,
        kind,
        white: u16::try_from(start.white).expect("an array number"),
        black: u16::try_from(start.black).expect("an array number"),
        plies: start.plies.iter().map(|&m| encode_move(m)).collect(),
        moves: played
            .moves
            .iter()
            .map(|&(m, s)| (encode_move(m), label(s)))
            .collect(),
        outcome: match played.outcome {
            Outcome::White => 0,
            Outcome::Black => 1,
            Outcome::Draw => 2,
        },
        ending: match played.ending {
            Ending::Mate => 0,
            Ending::Stalemate => 1,
            Ending::Repetition => 2,
            Ending::FiftyMoves => 3,
            Ending::InsufficientMaterial => 4,
        },
        refused: u16::try_from(refused).unwrap_or(u16::MAX),
    }
}

/// What a run writes and how.
#[derive(Clone, Debug)]
pub struct Settings {
    pub dir: PathBuf,
    pub run_seed: u64,
    pub threads: usize,
    pub chunk_games: u32,
    pub standard_share: u8,
    pub host: String,
    /// Games handed out past the next to be written, at least the thread count.
    pub ahead: u32,
    /// How often the control files are read and how often `STATUS` is rewritten.
    pub poll: Duration,
    pub status_every: Duration,
}

/// What the binary was built from, passed in so a test's modified tree is not refused.
#[derive(Clone, Debug)]
pub struct Build {
    pub commit_full: String,
    pub modified: bool,
    pub version: String,
    pub target: String,
    pub features: String,
    pub cfg_features: String,
}

impl Build {
    #[must_use]
    pub fn this() -> Build {
        Build {
            commit_full: version::COMMIT_FULL.to_string(),
            modified: version::MODIFIED,
            version: version::VERSION.to_string(),
            target: version::TARGET.to_string(),
            features: version::TARGET_FEATURES.to_string(),
            cfg_features: version::CFG_FEATURES.to_string(),
        }
    }
}

/// A run's settings from its command line, the host from `hostname` unless `--host` names it.
///
/// # Errors
///
/// A flag missing, unknown or malformed.
pub fn settings_from_args(args: &[String]) -> Result<Settings, String> {
    let (flags, rest) = split(args)?;
    if let Some(extra) = rest.first() {
        return Err(format!("unexpected argument {extra}"));
    }
    let (mut dir, mut run_seed, mut threads, mut chunk_games, mut share, mut host) =
        (None, None, 1, 1_000, 50, None);
    for (name, value) in &flags {
        match name.as_str() {
            "--out" => dir = Some(PathBuf::from(value)),
            "--run-seed" => run_seed = Some(parse_seed(value)?),
            "--threads" => {
                threads = usize::try_from(number(name, value)?).map_err(|_| "bad --threads")?;
            }
            "--chunk-games" => {
                chunk_games =
                    u32::try_from(number(name, value)?).map_err(|_| "bad --chunk-games")?;
            }
            "--standard-share" => {
                share = u8::try_from(number(name, value)?).map_err(|_| "bad --standard-share")?;
            }
            "--host" => host = Some(value.clone()),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let dir = dir.ok_or("--out is required")?;
    let run_seed = run_seed.ok_or("--run-seed is required: a hex seed, or fresh to draw one")?;
    let host = match host {
        Some(host) => host,
        None => hostname()?,
    };
    let ahead = u32::try_from(AHEAD.max(threads as u64)).unwrap_or(u32::MAX);
    Ok(Settings {
        dir,
        run_seed,
        threads,
        chunk_games,
        standard_share: share,
        host,
        ahead,
        poll: Duration::from_secs(1),
        status_every: Duration::from_secs(60),
    })
}

/// Hex, or `fresh` for one drawn from the per-process hasher keys, the time and the process.
///
/// # Errors
///
/// Neither.
pub fn parse_seed(value: &str) -> Result<u64, String> {
    if value == "fresh" {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        return Ok(std::hash::BuildHasher::hash_one(
            &std::hash::RandomState::new(),
            (nanos, std::process::id()),
        ));
    }
    u64::from_str_radix(value, 16).map_err(|_| format!("bad --run-seed `{value}`: hex, or fresh"))
}

/// The standard library has no call for it, so the command is asked.
fn hostname() -> Result<String, String> {
    let out = std::process::Command::new("hostname")
        .output()
        .map_err(|e| format!("hostname: {e}; name the host with --host"))?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || name.is_empty() {
        return Err("hostname named no host; name it with --host".to_string());
    }
    Ok(name)
}

/// Why this build may not write a dataset, or `None`.
#[must_use]
pub fn build_refusal(commit_full: &str, modified: bool) -> Option<&'static str> {
    if commit_full.len() != 40 || !commit_full.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(
            "this binary was built with no commit known; datagen run writes only from a git checkout",
        );
    }
    if modified {
        return Some(
            "this binary was built from a modified tree; datagen run writes only from a clean checkout",
        );
    }
    None
}

/// What plays one game, so a test can play a cheap one or make one panic.
pub type Player<'a> = dyn Fn(u64, u32, u8, &Table) -> GameRecord + Sync + 'a;

/// Chunks written and games in them, by start kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub chunks: u32,
    pub games: [u64; 2],
}

/// Until `STOP`: a chunk is closed at its planned count or short on a control file, and the run
/// sleeps through a pause and goes on in the next chunk.
///
/// # Errors
///
/// A refused build, a directory holding a `.tmp` or this run seed's files, a file that cannot be
/// written, or a game that panicked, whose chunk is left as a `.tmp` for `repair`.
pub fn run(settings: &Settings, build: &Build, player: &Player<'_>) -> Result<Written, String> {
    if let Some(why) = build_refusal(&build.commit_full, build.modified) {
        return Err(why.to_string());
    }
    check_settings(settings)?;
    check_dir(&settings.dir, settings.run_seed)?;
    let mut status = Status::new(settings);
    status.write(&settings.dir, "running")?;
    let mut written = Written::default();
    for chunk_index in 0.. {
        if control(&settings.dir)? == Control::Stop {
            break;
        }
        if control(&settings.dir)? == Control::Pause {
            log(&settings.dir, "paused")?;
            status.write(&settings.dir, "paused")?;
            while control(&settings.dir)? == Control::Pause {
                thread::sleep(settings.poll);
            }
            if control(&settings.dir)? == Control::Stop {
                break;
            }
            log(&settings.dir, "resumed")?;
        }
        let games = write_chunk(settings, build, chunk_index, player, &mut status)?;
        written.chunks += 1;
        for (all, these) in written.games.iter_mut().zip(games) {
            *all += these;
        }
        status.write(&settings.dir, "running")?;
    }
    status.write(&settings.dir, "stopped")?;
    Ok(written)
}

fn check_settings(settings: &Settings) -> Result<(), String> {
    if settings.standard_share > 100 {
        return Err(format!(
            "--standard-share {} is not a percentage",
            settings.standard_share
        ));
    }
    if settings.chunk_games == 0 {
        return Err("--chunk-games must be at least 1".to_string());
    }
    if settings.host.is_empty() || settings.host.len() > 32 || !settings.host.is_ascii() {
        return Err(format!(
            "the host `{}` is not ASCII of 1 to 32 bytes",
            settings.host
        ));
    }
    Ok(())
}

/// No `.tmp` waits for `repair`, and no file already carries this run seed.
fn check_dir(dir: &Path, run_seed: u64) -> Result<(), String> {
    let prefix = format!("{run_seed:016x}-");
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".tmp") {
            return Err(format!(
                "{} holds {name}; run cadence datagen repair {name} first",
                dir.display()
            ));
        }
        if name.starts_with(&prefix) {
            return Err(format!(
                "run seed {run_seed:016x} already has chunks in {}; a second run with it repeats their games",
                dir.display()
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Run,
    Pause,
    Stop,
}

/// `STOP` first, then any `PAUSE*`, each writer of a pause owning its own file.
fn control(dir: &Path) -> Result<Control, String> {
    if dir.join("STOP").exists() {
        return Ok(Control::Stop);
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if name.to_string_lossy().starts_with("PAUSE") {
            return Ok(Control::Pause);
        }
    }
    Ok(Control::Run)
}

/// What a worker sends the writer.
enum Message {
    Record(GameRecord),
    NoTable,
    Panicked(u32),
}

/// Hands out game indices in order, none past the planned count, none more than `ahead` past the
/// next to be written, and none once stopped.
struct Dispenser {
    ahead: u32,
    state: Mutex<Dispensed>,
    moved: Condvar,
}

struct Dispensed {
    next: u32,
    limit: u32,
    written: u32,
    stopped: bool,
}

impl Dispenser {
    fn take(&self) -> Option<u32> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.stopped || state.next >= state.limit {
                return None;
            }
            if state.next < state.written.saturating_add(self.ahead) {
                state.next += 1;
                return Some(state.next - 1);
            }
            state = self
                .moved
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn advance(&self, written: u32) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .written = written;
        self.moved.notify_all();
    }

    /// How many were handed out, every one of which will arrive.
    fn stop(&self) -> u32 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.stopped = true;
        self.moved.notify_all();
        state.next
    }
}

/// Held while a game is played, so a panic in it tells the writer which record will never come.
struct Playing<'a> {
    index: u32,
    tx: &'a mpsc::Sender<Message>,
}

impl Drop for Playing<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            let _ = self.tx.send(Message::Panicked(self.index));
        }
    }
}

/// One chunk, from its `.tmp` to its rename; the games written by kind.
fn write_chunk(
    settings: &Settings,
    build: &Build,
    chunk_index: u32,
    player: &Player<'_>,
    status: &mut Status,
) -> Result<[u64; 2], String> {
    let seed = chunk_seed(settings.run_seed, chunk_index);
    let name = chunk_name(settings.run_seed, chunk_index);
    status.chunk = chunk_index;
    let final_path = settings.dir.join(&name);
    let tmp_path = settings.dir.join(format!("{name}.tmp"));
    let header = header(settings, build, chunk_index)?;
    let head = header.encode()?;
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|e| format!("{}: {e}", tmp_path.display()))?;
    let mut sink = Sink {
        file,
        hasher: Sha256::new(),
        path: tmp_path.clone(),
    };
    sink.put(&head)?;
    let dispenser = Dispenser {
        ahead: settings.ahead.max(1),
        state: Mutex::new(Dispensed {
            next: 0,
            limit: settings.chunk_games,
            written: 0,
            stopped: false,
        }),
        moved: Condvar::new(),
    };
    let (tx, rx) = mpsc::channel::<Message>();
    let share = settings.standard_share;
    let mut kinds = [0_u64; 2];
    let mut plies = 0_u64;
    let threads = settings.threads.clamp(1, MAX_THREADS);
    let outcome = thread::scope(|scope| {
        let mut handles = Vec::new();
        let mut result: Result<u32, String> = Ok(0);
        for _ in 0..threads {
            let tx = tx.clone();
            let dispenser = &dispenser;
            match spawn_search(scope, move || {
                let Some(tt) = Table::new(HASH_MB) else {
                    let _ = tx.send(Message::NoTable);
                    return;
                };
                while let Some(index) = dispenser.take() {
                    let playing = Playing { index, tx: &tx };
                    let record = player(seed, index, share, &tt);
                    drop(playing);
                    if tx.send(Message::Record(record)).is_err() {
                        return;
                    }
                }
            }) {
                Ok(handle) => handles.push(handle),
                Err(e) => {
                    result = Err(e.to_string());
                    break;
                }
            }
        }
        drop(tx);
        if result.is_ok() {
            result = write_in_order(
                settings, &dispenser, &rx, &mut sink, &mut kinds, &mut plies, status,
            );
        }
        dispenser.stop();
        drop(rx);
        let mut panic = None;
        for handle in handles {
            if let Err(payload) = handle.join() {
                panic.get_or_insert_with(|| panic_text(payload.as_ref()));
            }
        }
        match (result, panic) {
            (Err(e), Some(text)) => Err(format!("{e}: {text}")),
            (Ok(_), Some(text)) => Err(format!("a worker panicked: {text}")),
            (result, None) => result,
        }
    });
    let games = outcome.map_err(|e| format!("{e}; {} is kept for repair", tmp_path.display()))?;
    let trailer = Trailer {
        games,
        plies,
        header_crc: crc32(&head),
    };
    sink.put(&trailer.encode())?;
    let notes = close(
        &mut Disk,
        &settings.dir,
        sink.file,
        &tmp_path,
        &final_path,
        sink.hasher.finish(),
    )?;
    log(&settings.dir, &format!("closed {name} games {games}"))?;
    unconfirmed(&settings.dir, notes)?;
    Ok(kinds)
}

/// Each closed file whose durability is unconfirmed, logged and said on stderr.
fn unconfirmed(dir: &Path, notes: Vec<String>) -> Result<(), String> {
    for note in notes {
        log(dir, &note)?;
        eprintln!("cadence datagen: {note}");
    }
    Ok(())
}

/// The open `.tmp`, hashed as it is written.
struct Sink {
    file: File,
    hasher: Sha256,
    path: PathBuf,
}

impl Sink {
    fn put(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.hasher.update(bytes);
        self.file
            .write_all(bytes)
            .map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

/// The records in index order as they arrive; on a control file the handing out stops and every
/// game already handed out is written, so the chunk closes short and whole.
fn write_in_order(
    settings: &Settings,
    dispenser: &Dispenser,
    rx: &mpsc::Receiver<Message>,
    sink: &mut Sink,
    kinds: &mut [u64; 2],
    plies: &mut u64,
    status: &mut Status,
) -> Result<u32, String> {
    let mut pending = BTreeMap::new();
    let mut written = 0_u32;
    let mut due = settings.chunk_games;
    let mut last_poll = Instant::now();
    while written < due {
        let message = rx.recv_timeout(settings.poll);
        if last_poll.elapsed() >= settings.poll {
            last_poll = Instant::now();
            if control(&settings.dir)? != Control::Run && due == settings.chunk_games {
                due = dispenser.stop();
            }
            status.maybe_write(&settings.dir)?;
        }
        match message {
            Ok(Message::Record(record)) => {
                pending.insert(record.index, record);
            }
            Ok(Message::NoTable) => {
                return Err("a worker's table could not be allocated".to_string());
            }
            Ok(Message::Panicked(index)) => return Err(format!("game {index} panicked")),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        while let Some(record) = pending.remove(&written) {
            sink.put(&record.encode()?)?;
            *plies += record.moves.len() as u64;
            kinds[usize::from(record.kind)] += 1;
            status.games[usize::from(record.kind)] += 1;
            written += 1;
        }
        dispenser.advance(written);
    }
    if written < due {
        return Err(format!("the workers stopped at game {written} of {due}"));
    }
    Ok(written)
}

/// Synced, its sidecar published, then renamed into place, each rename followed by a sync of the
/// directory.
fn close(
    steps: &mut dyn Steps,
    dir: &Path,
    file: File,
    tmp: &Path,
    path: &Path,
    digest: [u8; 32],
) -> Result<Vec<String>, String> {
    let failed = |e: io::Error| format!("{}: {e}", tmp.display());
    file.sync_all().map_err(failed)?;
    drop(file);
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let sidecar = dir.join(format!("{name}.sha256"));
    let partial = dir.join(format!("{name}.sha256.partial"));
    let side_failed = |e: io::Error| format!("{}: {e}", sidecar.display());
    let mut side = File::create(&partial).map_err(side_failed)?;
    side.write_all(format!("{}  {name}\n", hex(&digest)).as_bytes())
        .map_err(side_failed)?;
    side.sync_all().map_err(side_failed)?;
    drop(side);
    durable::publish(steps, &partial, &sidecar).map_err(side_failed)?;
    durable::publish(steps, tmp, path).map_err(failed)?;
    Ok(Vec::new())
}

fn header(settings: &Settings, build: &Build, chunk_index: u32) -> Result<Header, String> {
    let mut commit = [0_u8; 20];
    for (byte, pair) in commit
        .iter_mut()
        .zip(build.commit_full.as_bytes().chunks(2))
    {
        let text = std::str::from_utf8(pair).map_err(|e| e.to_string())?;
        *byte = u8::from_str_radix(text, 16).map_err(|e| e.to_string())?;
    }
    Ok(Header {
        commit,
        version: build.version.clone(),
        target: build.target.clone(),
        features: build.features.clone(),
        nodes: u32::try_from(NODES).map_err(|e| e.to_string())?,
        screen_nodes: u32::try_from(opening::SCREEN_NODES).map_err(|e| e.to_string())?,
        hash_mb: u16::try_from(HASH_MB).map_err(|e| e.to_string())?,
        random_plies: u8::try_from(opening::RANDOM_PLIES).map_err(|e| e.to_string())?,
        window: i16::try_from(opening::WINDOW).map_err(|e| e.to_string())?,
        standard_share: settings.standard_share,
        run_seed: settings.run_seed,
        chunk_index,
        games_planned: settings.chunk_games,
        host: settings.host.clone(),
        start_time: unix_now(),
        net_sha256: [0; 32],
        modified: u8::from(build.modified),
        cfg_sha256: sha256(build.cfg_features.as_bytes()),
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `YYYY-MM-DDThh:mm:ssZ` from Unix seconds, by the civil calendar's day count.
#[must_use]
pub fn utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rest = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// One timestamped line per chunk closed and per pause and resume.
fn log(dir: &Path, line: &str) -> Result<(), String> {
    let path = dir.join("datagen.log");
    let mut file = File::options()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    writeln!(file, "{} {line}", utc(unix_now())).map_err(|e| format!("{}: {e}", path.display()))
}

/// The `STATUS` line, rewritten whole so a reader never sees half of one.
struct Status {
    run_seed: u64,
    start: Instant,
    last: Instant,
    every: Duration,
    games: [u64; 2],
    chunk: u32,
}

impl Status {
    fn new(settings: &Settings) -> Status {
        Status {
            run_seed: settings.run_seed,
            start: Instant::now(),
            last: Instant::now(),
            every: settings.status_every,
            games: [0; 2],
            chunk: 0,
        }
    }

    fn maybe_write(&mut self, dir: &Path) -> Result<(), String> {
        if self.last.elapsed() >= self.every {
            self.write(dir, "running")?;
        }
        Ok(())
    }

    #[expect(clippy::cast_precision_loss, reason = "a game count is far below 2^52")]
    fn write(&mut self, dir: &Path, state: &str) -> Result<(), String> {
        self.last = Instant::now();
        let secs = self.start.elapsed().as_secs_f64().max(1e-9);
        let total = self.games[0] + self.games[1];
        let mut line = String::new();
        let _ = write!(
            line,
            "run-seed {:016x} chunk {} games {total} uniform {} standard {} games-per-second {:.2} {} {}",
            self.run_seed,
            self.chunk,
            self.games[0],
            self.games[1],
            total as f64 / secs,
            state,
            utc(unix_now())
        );
        let tmp = dir.join("STATUS.partial");
        fs::write(&tmp, format!("{line}\n")).map_err(|e| format!("{}: {e}", tmp.display()))?;
        fs::rename(&tmp, dir.join("STATUS")).map_err(|e| format!("{}: {e}", tmp.display()))
    }
}

/// A `.tmp` cut to the records it holds whole, closed as its chunk, and what was kept and dropped.
///
/// # Errors
///
/// A path not ending `.tmp`, a file whose header is not a chunk's, or a file that cannot be
/// rewritten.
pub fn repair(tmp: &Path) -> Result<String, String> {
    let name = tmp.to_string_lossy();
    let Some(final_name) = name.strip_suffix(".tmp") else {
        return Err(format!("{name}: not a .tmp"));
    };
    let path = PathBuf::from(final_name);
    let bytes = fs::read(tmp).map_err(|e| format!("{name}: {e}"))?;
    if bytes.len() < HEADER_LEN {
        return Err(format!("{name}: shorter than a chunk's header"));
    }
    let head: [u8; HEADER_LEN] = bytes[..HEADER_LEN].try_into().map_err(|_| "no header")?;
    Header::decode(&head).map_err(|e| format!("{name}: {e}"))?;
    let (records, used) = whole_records(&bytes[HEADER_LEN..]);
    let dropped = bytes.len() - HEADER_LEN - used;
    let mut whole = bytes[..HEADER_LEN + used].to_vec();
    let trailer = Trailer {
        games: u32::try_from(records.len()).map_err(|e| e.to_string())?,
        plies: records.iter().map(|r| r.moves.len() as u64).sum(),
        header_crc: crc32(&head),
    };
    whole.extend_from_slice(&trailer.encode());
    Chunk::read(&whole).map_err(|e| format!("{name}: the repaired chunk does not read: {e}"))?;
    let mut file = File::options()
        .write(true)
        .truncate(true)
        .open(tmp)
        .map_err(|e| format!("{name}: {e}"))?;
    file.write_all(&whole).map_err(|e| format!("{name}: {e}"))?;
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let notes = close(&mut Disk, &dir, file, tmp, &path, sha256(&whole))?;
    let mut report = format!(
        "repaired {} kept {} games dropped {dropped} bytes",
        path.display(),
        records.len()
    );
    for note in notes {
        report.push('\n');
        report.push_str(&note);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use cadence_dataset::durable::{Logged, Step};
    use cadence_dataset::sha256::{hex, sha256};

    use super::close;

    /// A sync that fails after the sidecar's rename or the chunk's leaves both in place, and reports
    /// the one whose durability is unconfirmed.
    #[test]
    fn a_failed_sync_after_either_rename_is_reported_and_leaves_the_chunk_closed() {
        for (n, unsynced) in [(0, "x.cdg.sha256"), (1, "x.cdg")] {
            let dir =
                std::env::temp_dir().join(format!("cadence-close-{n}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a scratch directory");
            let (tmp, path) = (dir.join("x.cdg.tmp"), dir.join("x.cdg"));
            std::fs::write(&tmp, b"chunk").expect("writes");
            let file = File::options().write(true).open(&tmp).expect("opens");
            let closed = close(
                &mut Logged::failing_sync(n),
                &dir,
                file,
                &tmp,
                &path,
                sha256(b"chunk"),
            );
            let mut left: Vec<String> = std::fs::read_dir(&dir)
                .expect("lists")
                .map(|e| {
                    e.expect("an entry")
                        .file_name()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            left.sort_unstable();
            let _ = std::fs::remove_dir_all(&dir);
            let notes = closed.expect("closed, with one sync failed");
            assert_eq!(left, ["x.cdg", "x.cdg.sha256"], "sync {n} failing");
            assert_eq!(notes.len(), 1, "{notes:?}");
            assert!(
                notes[0].contains(&format!("{unsynced}: published, durability unconfirmed")),
                "{notes:?}"
            );
        }
    }

    /// The sidecar and then the chunk are each published by a rename, and the directory is synced
    /// after each, so a crash cannot keep a name without its entry or lose one already reported.
    #[test]
    fn a_closed_chunk_and_its_sidecar_each_have_their_directory_synced() {
        let dir = std::env::temp_dir().join(format!("cadence-close-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let (tmp, path) = (dir.join("x.cdg.tmp"), dir.join("x.cdg"));
        std::fs::write(&tmp, b"chunk").expect("writes");
        let file = File::options().write(true).open(&tmp).expect("opens");
        let digest = sha256(b"chunk");
        let mut logged = Logged::default();
        let closed = close(&mut logged, &dir, file, &tmp, &path, digest);
        let sidecar = std::fs::read_to_string(dir.join("x.cdg.sha256"));
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .expect("lists")
            .map(|e| {
                e.expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        left.sort_unstable();
        let _ = std::fs::remove_dir_all(&dir);
        closed.expect("closes");
        assert_eq!(
            sidecar.expect("a sidecar"),
            format!("{}  x.cdg\n", hex(&digest))
        );
        assert_eq!(left, ["x.cdg", "x.cdg.sha256"]);
        assert_eq!(
            logged.steps,
            [
                Step::Rename(dir.join("x.cdg.sha256")),
                Step::Sync(dir.clone()),
                Step::Rename(path),
                Step::Sync(dir)
            ]
        );
    }
}
