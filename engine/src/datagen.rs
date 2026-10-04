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
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use cadence_core::rng::{Rng, splitmix64};

use crate::tt::Table;
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
/// If `out` cannot be written or a table cannot be allocated.
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
            scope.spawn(move || {
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
    let mut out: Box<dyn Write> = match path {
        Some(p) => Box::new(BufWriter::new(
            File::create(&p).map_err(|e| format!("{p}: {e}"))?,
        )),
        None => Box::new(BufWriter::new(io::stdout().lock())),
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
    generate(seed, games, threads, &mut out, &mut progress).map_err(|e| e.to_string())
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
    let create = |p: Option<String>, name: &str| -> Result<BufWriter<File>, String> {
        let p = p.ok_or_else(|| format!("{name} is required"))?;
        Ok(BufWriter::new(
            File::create(&p).map_err(|e| format!("{p}: {e}"))?,
        ))
    };
    let mut train = create(train, "--train")?;
    let mut holdout = create(holdout, "--holdout")?;
    let input = BufReader::new(File::open(input).map_err(|e| format!("{input}: {e}"))?);
    let stats = extract::extract(input, &mut train, &mut holdout, every)?;
    train.flush().map_err(|e| e.to_string())?;
    holdout.flush().map_err(|e| e.to_string())?;
    stats
        .report(&mut io::stdout().lock())
        .map_err(|e| e.to_string())
}
