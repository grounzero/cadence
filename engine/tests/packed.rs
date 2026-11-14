// SPDX-License-Identifier: GPL-3.0-or-later

//! The packed writer: what a run refuses, how a chunk closes, pauses and stops, what a kill leaves
//! for `repair`, and that a chunk's records do not depend on the thread count.

#![cfg(feature = "packed-datagen")]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cadence_core::rng::Rng;
use cadence_dataset::chunk::{
    Chunk, GameRecord, HEADER_LEN, KIND_STANDARD, KIND_UNIFORM, TRAILER_LEN, chunk_name,
};
use cadence_dataset::sha256::{hex, sha256};
use cadence_engine::datagen::game_seed;
use cadence_engine::datagen::opening::{RANDOM_PLIES, SCREEN_NODES};
use cadence_engine::datagen::packed::{
    self, Build, Settings, build_refusal, chunk_seed, parse_seed, repair, start_kind, utc,
};
use cadence_engine::datagen::{HASH_MB, game};
use cadence_engine::tt::Table;
use cadence_engine::version::VERSION;

const SEED: u64 = 0x5eed;

/// A fake game's length once a control file is down, so the writer's poll lands inside the chunk.
const GAME: Duration = Duration::from_millis(2);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadence-packed-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

fn settings(dir: &Path, chunk_games: u32, threads: usize) -> Settings {
    Settings {
        dir: dir.to_path_buf(),
        run_seed: SEED,
        threads,
        chunk_games,
        standard_share: 50,
        host: "test".to_string(),
        ahead: 1_024,
        poll: Duration::from_millis(10),
        status_every: Duration::from_millis(10),
    }
}

/// A clean, known commit, so a test's own working tree is never refused.
fn build() -> Build {
    Build {
        commit_full: "0123456789abcdef0123456789abcdef01234567".to_string(),
        modified: false,
        version: VERSION.to_string(),
        target: "test-target-triple".to_string(),
        features: "+test".to_string(),
        cfg_features: "test".to_string(),
    }
}

/// A record the format accepts, played by nothing.
fn fake(index: u32, share: u8) -> GameRecord {
    GameRecord {
        index,
        seed: u64::from(index),
        kind: if share == 100 {
            KIND_STANDARD
        } else {
            KIND_UNIFORM
        },
        white: 518,
        black: 518,
        plies: vec![0; RANDOM_PLIES],
        moves: vec![(0, 0); 3],
        outcome: 2,
        ending: 3,
        refused: 0,
    }
}

fn listed(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("lists")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn read_chunk(dir: &Path, index: u32) -> Chunk {
    let path = dir.join(chunk_name(SEED, index));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Chunk::read(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn a_run_writes_only_from_a_known_clean_commit() {
    let full = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(build_refusal(full, false), None);
    assert!(
        build_refusal(full, true)
            .expect("refused")
            .contains("modified tree")
    );
    assert!(
        build_refusal("unknown", false)
            .expect("refused")
            .contains("no commit known")
    );
    assert!(build_refusal("unknown", true).is_some());
}

/// The kind is the first draw of the game's stream, so the share decides it and nothing else.
#[test]
fn the_start_kind_follows_the_share() {
    let kinds = |share: u8, games: u64| -> Vec<u8> {
        (0..games)
            .map(|i| start_kind(&mut Rng::new(game_seed(chunk_seed(SEED, 0), i)), share))
            .collect()
    };
    assert!(kinds(0, 200).iter().all(|&k| k == KIND_UNIFORM));
    assert!(kinds(100, 200).iter().all(|&k| k == KIND_STANDARD));
    let standard: usize = kinds(50, 400)
        .iter()
        .map(|&k| usize::from(k == KIND_STANDARD))
        .sum();
    println!("share 50: {standard} standard starts in 400");
    assert!((160..=240).contains(&standard), "{standard} of 400");
}

/// A standard start is the orthodox array for both sides, and a start the screen refuses is drawn
/// again under the same kind and counted.
#[test]
fn a_refused_start_is_drawn_again_under_its_kind() {
    let tt = Table::new(HASH_MB).expect("a table");
    let calls = Mutex::new(0);
    let probe = |board: &mut _, tt: &Table| {
        let mut calls = calls.lock().expect("a lock");
        *calls += 1;
        if *calls <= 3 {
            None
        } else {
            game::probe(board, SCREEN_NODES, tt)
        }
    };
    let record = packed::play_with(chunk_seed(SEED, 0), 0, 100, &tt, &probe);
    assert_eq!(record.kind, KIND_STANDARD);
    assert_eq!((record.white, record.black), (518, 518));
    assert_eq!(record.refused, 3);
}

/// Workers finish in any order, and the chunk's records are the same at one thread and at three.
#[test]
fn a_chunk_holds_the_same_records_at_any_thread_count() {
    let records = |threads: usize| {
        let dir = scratch(&format!("threads-{threads}"));
        let stop_after = |seed: u64, index: u32, share: u8, tt: &Table| {
            if index == 2 {
                std::fs::write(dir.join("STOP"), "").expect("writes");
            }
            packed::play(seed, index, share, tt)
        };
        let written = packed::run(&settings(&dir, 3, threads), &build(), &stop_after);
        assert_eq!(written.expect("a run").chunks, 1);
        let chunk = read_chunk(&dir, 0);
        let _ = std::fs::remove_dir_all(&dir);
        chunk.records
    };
    let one = records(1);
    assert_eq!(one.len(), 3);
    assert_eq!(records(3), one);
}

/// `STOP` stops the handing out, the games handed out finish, and the chunk closes short with its
/// trailer counting what it holds and its sidecar hashing it.
#[test]
fn stop_closes_a_short_chunk_whole() {
    let dir = scratch("stop");
    let player = |_: u64, index: u32, share: u8, _: &Table| {
        if index == 5 {
            std::fs::write(dir.join("STOP"), "").expect("writes");
        }
        if index >= 5 {
            std::thread::sleep(GAME);
        }
        fake(index, share)
    };
    let written = packed::run(&settings(&dir, 1_000, 2), &build(), &player).expect("a run");
    let chunk = read_chunk(&dir, 0);
    let name = chunk_name(SEED, 0);
    let sidecar = std::fs::read_to_string(dir.join(format!("{name}.sha256"))).expect("a sidecar");
    let bytes = std::fs::read(dir.join(&name)).expect("the chunk");
    let left = listed(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(written.chunks, 1);
    let games = chunk.records.len();
    assert!((6..1_000).contains(&games), "{games} games");
    assert_eq!(chunk.trailer.games as usize, games);
    assert_eq!(sidecar, format!("{}  {name}\n", hex(&sha256(&bytes))));
    assert!(
        !left
            .iter()
            .any(|n| Path::new(n).extension().is_some_and(|e| e == "tmp")),
        "{left:?}"
    );
    assert!(left.contains(&"STATUS".to_string()) && left.contains(&"datagen.log".to_string()));
}

/// A pause closes the chunk short, and the run goes on in the next chunk under the same run seed
/// once the pause is lifted.
#[test]
fn a_pause_closes_the_chunk_and_the_run_goes_on_in_the_next() {
    let dir = scratch("pause");
    let player = |seed: u64, index: u32, share: u8, _: &Table| {
        if seed == chunk_seed(SEED, 0) && index == 3 {
            std::fs::write(dir.join("PAUSE"), "").expect("writes");
        }
        if seed == chunk_seed(SEED, 1) && index == 2 {
            std::fs::write(dir.join("STOP"), "").expect("writes");
        }
        if index >= 2 {
            std::thread::sleep(GAME);
        }
        fake(index, share)
    };
    let written = std::thread::scope(|scope| {
        let running = scope.spawn(|| packed::run(&settings(&dir, 1_000, 1), &build(), &player));
        let deadline = Instant::now() + Duration::from_secs(30);
        let paused = || {
            std::fs::read_to_string(dir.join("datagen.log")).is_ok_and(|l| l.contains(" paused"))
        };
        while !paused() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_file(dir.join("PAUSE")).expect("the pause is lifted");
        running.join().expect("the run returns")
    });
    let (first, second) = (read_chunk(&dir, 0), read_chunk(&dir, 1));
    let log = std::fs::read_to_string(dir.join("datagen.log")).expect("a log");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(written.expect("a run").chunks, 2);
    assert!(
        first.records.len() < 1_000,
        "the pause did not close the chunk short"
    );
    assert_eq!(
        (second.header.run_seed, second.header.chunk_index),
        (SEED, 1)
    );
    assert!(log.contains(" paused") && log.contains(" resumed"), "{log}");
}

/// A `.tmp` waits for `repair`, and a run seed already present would repeat its games.
#[test]
fn a_run_is_refused_beside_a_tmp_or_its_own_seed() {
    for (name, file, want) in [
        ("tmp", "x.cdg.tmp".to_string(), "repair x.cdg.tmp first"),
        ("seed", chunk_name(SEED, 0), "already has chunks"),
    ] {
        let dir = scratch(&format!("refused-{name}"));
        std::fs::write(dir.join(&file), "").expect("writes");
        let refused = packed::run(&settings(&dir, 10, 1), &build(), &|_, i, s, _| fake(i, s));
        let left = listed(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        let error = refused.expect_err("a run was started");
        assert!(error.contains(want), "{error}");
        assert_eq!(left, [file], "the directory changed");
    }
}

/// Cut at every byte of its records, a `.tmp` repairs to exactly the records whole before the cut.
#[test]
fn repair_keeps_exactly_the_whole_records_at_every_cut() {
    let dir = scratch("repair-source");
    let player = |_: u64, index: u32, share: u8, _: &Table| fake(index, share);
    packed::run(&settings(&dir, 3, 1), &build(), &|s, i, sh, t| {
        if i == 2 {
            std::fs::write(dir.join("STOP"), "").expect("writes");
        }
        player(s, i, sh, t)
    })
    .expect("a run");
    let whole = std::fs::read(dir.join(chunk_name(SEED, 0))).expect("the chunk");
    let _ = std::fs::remove_dir_all(&dir);
    let record = fake(0, 50).encode().expect("encodes").len();
    let body_end = whole.len() - TRAILER_LEN;
    for cut in HEADER_LEN..=body_end {
        let dir = scratch("repair");
        let tmp = dir.join(format!("{}.tmp", chunk_name(SEED, 0)));
        std::fs::write(&tmp, &whole[..cut]).expect("writes");
        let report = repair(&tmp).unwrap_or_else(|e| panic!("cut at {cut}: {e}"));
        let chunk = read_chunk(&dir, 0);
        let _ = std::fs::remove_dir_all(&dir);
        let want = (cut - HEADER_LEN) / record;
        assert_eq!(chunk.records.len(), want, "cut at {cut}: {report}");
    }
}

/// `repair` from the command line closes a `.tmp` and says what it did not examine, the skipped
/// directory sync among it where a directory cannot be synced.
#[test]
fn repair_says_what_it_did_not_examine() {
    let dir = scratch("repair-cli");
    packed::run(&settings(&dir, 3, 1), &build(), &|s, i, sh, t| {
        if i == 2 {
            std::fs::write(dir.join("STOP"), "").expect("writes");
        }
        let _ = (s, t);
        fake(i, sh)
    })
    .expect("a run");
    let chunk = dir.join(chunk_name(SEED, 0));
    let tmp = dir.join(format!("{}.tmp", chunk_name(SEED, 0)));
    std::fs::rename(&chunk, &tmp).expect("renames");
    let out = Command::new(support::binary())
        .args(["datagen", "repair"])
        .arg(&tmp)
        .output()
        .expect("runs");
    let repaired = chunk.exists();
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && repaired, "{out:?}");
    assert!(
        stdout.contains("  not examined: the games themselves"),
        "{stdout}"
    );
    assert_eq!(
        stdout.contains("  not examined: each published name's durability"),
        cfg!(not(unix)),
        "{stdout}"
    );
}

/// A game that panics ends the run naming it and leaves its chunk's `.tmp`, holding every record
/// before it, for `repair`.
#[test]
fn a_panicking_game_leaves_its_chunk_for_repair() {
    let dir = scratch("panic");
    let player = |_: u64, index: u32, share: u8, _: &Table| {
        assert!(index != 3, "game 3 cannot be played");
        fake(index, share)
    };
    let error = packed::run(&settings(&dir, 10, 2), &build(), &player).expect_err("a run");
    let tmp = dir.join(format!("{}.tmp", chunk_name(SEED, 0)));
    let report = repair(&tmp);
    let chunk = read_chunk(&dir, 0);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(error.contains("game 3 panicked"), "{error}");
    assert!(error.contains("kept for repair"), "{error}");
    report.expect("repaired");
    assert_eq!(chunk.records.len(), 3);
}

/// Game 0 is held until the others have started every game the window allows, and a while longer,
/// so a game handed out past the window shows in what had started.
#[test]
fn no_game_is_handed_out_further_ahead_than_the_window() {
    const AHEAD: u32 = 8;
    let dir = scratch("window");
    let started = Mutex::new(Vec::new());
    let held = Mutex::new(Vec::new());
    let player = |_: u64, index: u32, share: u8, _: &Table| {
        started.lock().expect("a lock").push(index);
        if index == 0 {
            let deadline = Instant::now() + Duration::from_secs(30);
            while started.lock().expect("a lock").len() < AHEAD as usize
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            std::thread::sleep(Duration::from_millis(200));
            *held.lock().expect("a lock") = started.lock().expect("a lock").clone();
        }
        if index == 63 {
            std::fs::write(dir.join("STOP"), "").expect("writes");
        }
        fake(index, share)
    };
    let mut windowed = settings(&dir, 64, 3);
    windowed.ahead = AHEAD;
    packed::run(&windowed, &build(), &player).expect("a run");
    let chunk = read_chunk(&dir, 0);
    let _ = std::fs::remove_dir_all(&dir);
    let mut held = held.into_inner().expect("a lock");
    held.sort_unstable();
    assert_eq!(
        held,
        (0..AHEAD).collect::<Vec<_>>(),
        "started while game 0 was held"
    );
    assert_eq!(chunk.records.len(), 64);
}

/// `STATUS` counts what was written, by kind, and the header carries the share asked for.
#[test]
fn the_status_counts_what_was_written_and_the_header_carries_the_share() {
    let dir = scratch("status");
    let player = |seed: u64, index: u32, _: u8, _: &Table| {
        if seed == chunk_seed(SEED, 1) && index == 3 {
            std::fs::write(dir.join("STOP"), "").expect("writes");
        }
        fake(index, if index.is_multiple_of(2) { 100 } else { 0 })
    };
    let mut shared = settings(&dir, 4, 1);
    shared.standard_share = 37;
    let written = packed::run(&shared, &build(), &player).expect("a run");
    let status = std::fs::read_to_string(dir.join("STATUS")).expect("a status line");
    let chunks = [read_chunk(&dir, 0), read_chunk(&dir, 1)];
    let _ = std::fs::remove_dir_all(&dir);
    let in_chunks: usize = chunks.iter().map(|c| c.records.len()).sum();
    let total = written.games[0] + written.games[1];
    assert_eq!(total as usize, in_chunks);
    let want = format!(
        "games {total} uniform {} standard {}",
        written.games[0], written.games[1]
    );
    assert!(status.contains(&want), "{status}");
    assert!(chunks.iter().all(|c| c.header.standard_share == 37));
}

#[test]
fn a_run_seed_is_hex_or_drawn_fresh() {
    assert_eq!(parse_seed("ff"), Ok(255));
    assert!(parse_seed("zz").is_err());
    assert_ne!(parse_seed("fresh"), parse_seed("fresh"));
}

#[test]
fn utc_names_the_civil_date_and_time() {
    assert_eq!(utc(0), "1970-01-01T00:00:00Z");
    assert_eq!(utc(1_794_600_000), "2026-11-13T20:00:00Z");
    assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
}

/// A run with no seed would repeat another's games, so the command line is refused and nothing is
/// written.
#[test]
fn the_command_line_needs_a_run_seed() {
    let dir = scratch("cli");
    let out = Command::new(support::binary())
        .args(["datagen", "run", "--out"])
        .arg(dir.join("run"))
        .args(["--host", "test"])
        .output()
        .expect("runs");
    let left = listed(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--run-seed is required"), "{err}");
    assert_eq!(left, [] as [&str; 0], "written without a seed");
}
