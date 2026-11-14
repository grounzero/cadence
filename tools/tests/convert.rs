// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-tools convert` on datasets built here from random legal games: what it refuses before
//! reading a record, what each filter keeps, how it splits and shuffles, and what it publishes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use cadence_core::chess960::dfrc_fen;
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_dataset::chunk::{Chunk, GameRecord, Header, NO_SCORE, chunk_name, encode_move};
use cadence_dataset::constants::{FIRST_PLY, LAST_PLY, MATE_BOUND, RANDOM_PLIES};
use cadence_dataset::sha256::{hex, sha256};
use cadence_dataset::trainer::RECORD_LEN;

/// One fault planted in a dataset directory.
type Fault = fn(&Path);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadence-convert-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A legal game: its arrays by kind, random plies, then up to `length` random moves with scores
/// drawn small, and now and then none, a mate or an exact zero.
fn game(rng: &mut Rng, index: u32, kind: u8, length: usize) -> GameRecord {
    let (white, black) = if kind == 1 {
        (518, 518)
    } else {
        (rng.below(960) as u16, rng.below(960) as u16)
    };
    let mut board =
        Board::from_fen(&dfrc_fen(u32::from(white), u32::from(black))).expect("a start");
    let mut plies = Vec::new();
    for _ in 0..RANDOM_PLIES {
        let legal = generate_legal(&board);
        let m = legal.as_slice()[rng.below(legal.len())];
        plies.push(encode_move(m));
        board.play(m);
    }
    let mut moves = Vec::new();
    for _ in 0..length {
        let legal = generate_legal(&board);
        if legal.is_empty() {
            break;
        }
        let m = legal.as_slice()[rng.below(legal.len())];
        let score = match rng.below(20) {
            0 => NO_SCORE,
            1 => MATE_BOUND + 10,
            2 => 0,
            _ => i16::try_from(rng.below(600)).expect("below 600") - 300,
        };
        moves.push((encode_move(m), score));
        board.play(m);
    }
    GameRecord {
        index,
        seed: rng.next_u64(),
        kind,
        white,
        black,
        plies,
        moves,
        outcome: (index % 3) as u8,
        ending: 3,
        refused: 0,
    }
}

fn header(run_seed: u64, chunk_index: u32) -> Header {
    Header {
        commit: [1; 20],
        version: "0.0.0-test".to_string(),
        target: "test".to_string(),
        features: String::new(),
        nodes: 5_000,
        screen_nodes: 5_000,
        hash_mb: 16,
        random_plies: RANDOM_PLIES as u8,
        window: 200,
        standard_share: 50,
        run_seed,
        chunk_index,
        games_planned: 1_000,
        host: "test".to_string(),
        start_time: 0,
        net_sha256: [0; 32],
        modified: 0,
        cfg_sha256: [0; 32],
    }
}

/// Chunks with their sidecars and a manifest listing them, as `datagen verify --manifest` writes it.
fn dataset(dir: &Path, run_seed: u64, chunks: &[Vec<GameRecord>]) -> PathBuf {
    std::fs::create_dir_all(dir).expect("a dataset directory");
    let mut rows = String::new();
    for (i, records) in chunks.iter().enumerate() {
        let index = u32::try_from(i).expect("an index");
        let name = chunk_name(run_seed, index);
        let bytes = Chunk::bytes(&header(run_seed, index), records).expect("a chunk");
        let digest = hex(&sha256(&bytes));
        std::fs::write(dir.join(&name), &bytes).expect("writes");
        std::fs::write(
            dir.join(format!("{name}.sha256")),
            format!("{digest}  {name}\n"),
        )
        .expect("writes");
        let _ = writeln!(rows, "{name}\t{digest}\t{}\t0\t0\t0", records.len());
    }
    let manifest = dir.join("MANIFEST.tsv");
    let text = format!(
        "# cadence dataset manifest\ncommit\t{}\nchunk\tsha256\tgames\tplies\tuniform\tstandard\n{rows}",
        hex(&[1; 20])
    );
    std::fs::write(&manifest, text).expect("writes");
    manifest
}

fn games(seed: u64, count: u32, kind: impl Fn(u32) -> u8) -> Vec<GameRecord> {
    let mut rng = Rng::new(seed);
    (0..count).map(|i| game(&mut rng, i, kind(i), 60)).collect()
}

/// Whether it succeeded, its stdout and stderr, and the units published under `out`.
fn convert(manifests: &[&Path], out: &Path, extra: &[&str]) -> (bool, String, Vec<PathBuf>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cadence-tools"));
    command.arg("convert");
    for m in manifests {
        command.arg("--manifest").arg(m);
    }
    let output = command
        .arg("--out")
        .arg(out)
        .args(extra)
        .output()
        .expect("runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let units = std::fs::read_dir(out).map_or_else(
        |_| Vec::new(),
        |dir| dir.map(|e| e.expect("an entry").path()).collect(),
    );
    (output.status.success(), text, units)
}

fn records(path: &Path) -> Vec<Vec<u8>> {
    let bytes = std::fs::read(path).expect("reads");
    assert_eq!(bytes.len() % RECORD_LEN, 0, "a whole number of records");
    bytes.chunks(RECORD_LEN).map(<[u8]>::to_vec).collect()
}

/// The same seed gives the same bytes and the same unit; another seed the same positions in
/// another order.
#[test]
fn the_shuffle_seed_decides_the_order_and_nothing_else() {
    let dir = scratch("seed");
    let manifest = dataset(
        &dir.join("data"),
        7,
        &[games(1, 12, |_| 0), games(2, 12, |i| u8::from(i % 3 == 0))],
    );
    let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
    let (ok_a, text, units_a) = convert(&[&manifest], &a, &["--shuffle-seed", "1"]);
    let (ok_b, _, units_b) = convert(&[&manifest], &b, &["--shuffle-seed", "1"]);
    let (ok_c, _, units_c) = convert(&[&manifest], &c, &["--shuffle-seed", "2"]);
    assert!(ok_a && ok_b && ok_c, "{text}");
    let (unit_a, unit_b, unit_c) = (&units_a[0], &units_b[0], &units_c[0]);
    assert_eq!(unit_a.file_name(), unit_b.file_name());
    let train_a = records(&unit_a.join("train.bin"));
    assert_eq!(train_a, records(&unit_b.join("train.bin")));
    let mut sorted_a = train_a.clone();
    let mut sorted_c = records(&unit_c.join("train.bin"));
    assert_ne!(train_a, sorted_c, "another seed gave the same order");
    sorted_a.sort();
    sorted_c.sort();
    assert_eq!(sorted_a, sorted_c);
    let manifest_text = std::fs::read_to_string(unit_a.join("MANIFEST.tsv")).expect("a manifest");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        manifest_text.contains("shuffle-seed\t0000000000000001"),
        "{manifest_text}"
    );
    assert!(
        text.contains("  not examined: the trainer's own reading"),
        "{text}"
    );
    assert_eq!(
        text.contains("  not examined: each published name's durability"),
        cfg!(not(unix)),
        "{text}"
    );
}

/// What each filter drops is counted in the order the converter applies them, and what is left is
/// what the outputs hold; the holdout is every tenth game by index within its chunk.
#[test]
fn every_position_is_counted_by_the_filter_that_dropped_it() {
    let dir = scratch("filters");
    let chunk = games(3, 20, |_| 0);
    let manifest = dataset(&dir.join("data"), 9, std::slice::from_ref(&chunk));
    let (ok, text, units) = convert(&[&manifest], &dir.join("out"), &["--shuffle-seed", "5"]);
    assert!(ok, "{text}");
    let (train, holdout) = (
        records(&units[0].join("train.bin")),
        records(&units[0].join("holdout.bin")),
    );
    let _ = std::fs::remove_dir_all(&dir);
    let mut want = [0_usize; 7];
    let (mut kept_train, mut kept_holdout) = (0, 0);
    for record in &chunk {
        let mut board =
            Board::from_fen(&dfrc_fen(u32::from(record.white), u32::from(record.black)))
                .expect("a start");
        for &bits in &record.plies {
            board.play(
                cadence_dataset::chunk::decode_move(&generate_legal(&board), bits).expect("legal"),
            );
        }
        for (ply, &(bits, score)) in record.moves.iter().enumerate() {
            let m =
                cadence_dataset::chunk::decode_move(&generate_legal(&board), bits).expect("legal");
            let slot = if !(FIRST_PLY..=LAST_PLY).contains(&ply) {
                0
            } else if board.in_check() {
                1
            } else if m.is_noisy() {
                2
            } else if score == NO_SCORE {
                3
            } else if score.unsigned_abs() >= MATE_BOUND.unsigned_abs() {
                4
            } else if record.index % 10 == 0 {
                kept_holdout += 1;
                5
            } else {
                kept_train += 1;
                6
            };
            want[slot] += 1;
            board.play(m);
        }
    }
    let line = format!(
        "uniform outside-plies-{FIRST_PLY}-{LAST_PLY} {} in-check {} noisy {} no-score {} mate {}",
        want[0], want[1], want[2], want[3], want[4]
    );
    assert!(text.contains(&line), "{line}\n{text}");
    assert_eq!((train.len(), holdout.len()), (kept_train, kept_holdout));
    assert!(
        want[3] > 0 && want[4] > 0,
        "no planted no-score or mate reached the window"
    );
}

/// Before any record is read, each listing fault is refused naming the file, and nothing is
/// published.
#[test]
fn a_dataset_that_disagrees_with_its_manifest_is_refused() {
    let faults: [(&str, &str, Fault); 5] = [
        ("unlisted", "not in its manifest", |d| {
            std::fs::write(d.join("extra.cdg"), b"").expect("writes");
        }),
        ("missing", "is listed and missing", |d| {
            std::fs::remove_file(d.join(chunk_name(11, 0))).expect("removes");
        }),
        ("manifest", "and the manifest says", |d| {
            let m = d.join("MANIFEST.tsv");
            let text = std::fs::read_to_string(&m).expect("reads");
            let row = text.lines().last().expect("a row").to_string();
            let mut fields: Vec<&str> = row.split('\t').collect();
            let flipped = fields[1].replacen(
                &fields[1][..1],
                if &fields[1][..1] == "0" { "1" } else { "0" },
                1,
            );
            fields[1] = &flipped;
            std::fs::write(&m, text.replace(&row, &fields.join("\t"))).expect("writes");
        }),
        ("no-sidecar", ".sha256 is missing", |d| {
            std::fs::remove_file(d.join(format!("{}.sha256", chunk_name(11, 0)))).expect("removes");
        }),
        ("sidecar", "its sidecar says", |d| {
            let side = d.join(format!("{}.sha256", chunk_name(11, 0)));
            std::fs::write(&side, format!("{}  x\n", "0".repeat(64))).expect("writes");
        }),
    ];
    for (name, want, fault) in faults {
        let dir = scratch(&format!("refused-{name}"));
        let data = dir.join("data");
        let manifest = dataset(&data, 11, &[games(4, 5, |_| 0)]);
        fault(&data);
        let out = dir.join("out");
        let (ok, text, units) = convert(&[&manifest], &out, &["--shuffle-seed", "1"]);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!ok, "{name}: converted");
        assert!(text.contains(want), "{name}: {text}");
        assert_eq!(units, [] as [PathBuf; 0], "{name}: published");
    }
}

/// A chunk listed in two datasets, or a game copied between them, is the same data twice.
#[test]
fn the_same_chunk_or_game_twice_is_refused() {
    let dir = scratch("twice");
    let chunk = games(5, 6, |_| 0);
    let first = dataset(&dir.join("one"), 21, std::slice::from_ref(&chunk));
    let same_chunk = dataset(&dir.join("two"), 21, &[games(6, 6, |_| 0)]);
    let mut copied = games(7, 6, |_| 0);
    copied[2] = chunk[2].clone();
    let same_game = dataset(&dir.join("three"), 22, &[copied]);
    let (ok_chunk, chunk_text, _) = convert(
        &[&first, &same_chunk],
        &dir.join("a"),
        &["--shuffle-seed", "1"],
    );
    let (ok_game, game_text, _) = convert(
        &[&first, &same_game],
        &dir.join("b"),
        &["--shuffle-seed", "1"],
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !ok_chunk && chunk_text.contains("carry run seed"),
        "{chunk_text}"
    );
    assert!(
        !ok_game && game_text.contains("is the same game as"),
        "{game_text}"
    );
}

/// The share asked is met exactly by dropping games of the more common kind.
#[test]
fn the_standard_share_is_met_exactly() {
    let dir = scratch("share");
    let manifest = dataset(
        &dir.join("data"),
        31,
        &[games(8, 40, |i| u8::from(i % 4 == 0))],
    );
    let (ok, text, _) = convert(
        &[&manifest],
        &dir.join("out"),
        &["--shuffle-seed", "1", "--standard-share", "50"],
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{text}");
    assert!(
        text.contains("uniform games 10 ") && text.contains("standard games 10 "),
        "{text}"
    );
}

/// A holdout game that shares a position with a training game is counted.
#[test]
fn a_holdout_position_also_in_training_is_counted() {
    let dir = scratch("overlap");
    let mut chunk = games(9, 2, |_| 0);
    let mut twin = chunk[0].clone();
    twin.index = 1;
    twin.seed = chunk[1].seed;
    twin.moves.truncate(FIRST_PLY + 3);
    let mut rng = Rng::new(99);
    let mut board =
        Board::from_fen(&dfrc_fen(u32::from(twin.white), u32::from(twin.black))).expect("a start");
    for &bits in twin.plies.iter().chain(twin.moves.iter().map(|(m, _)| m)) {
        board.play(
            cadence_dataset::chunk::decode_move(&generate_legal(&board), bits).expect("legal"),
        );
    }
    for _ in 0..10 {
        let legal = generate_legal(&board);
        if legal.is_empty() {
            break;
        }
        let m = legal.as_slice()[rng.below(legal.len())];
        twin.moves.push((encode_move(m), 5));
        board.play(m);
    }
    chunk[1] = twin;
    let manifest = dataset(&dir.join("data"), 41, &[chunk]);
    let (ok, text, _) = convert(&[&manifest], &dir.join("out"), &["--shuffle-seed", "1"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{text}");
    let line = text
        .lines()
        .find(|l| l.starts_with("holdout positions whose key"))
        .expect("a count");
    let count: u64 = line
        .rsplit(' ')
        .next()
        .and_then(|n| n.parse().ok())
        .expect("a number");
    assert!(count > 0, "{text}");
}

/// A game longer than the search's stack is read whole, its plies past the window counted.
#[test]
fn a_game_longer_than_the_search_stack_is_read_whole() {
    let dir = scratch("long");
    let mut rng = Rng::new(31);
    let long = (0..64)
        .map(|_| game(&mut rng, 0, 0, 300))
        .find(|g| g.moves.len() == 300)
        .expect("a random game that lasts 300 moves");
    let manifest = dataset(&dir.join("data"), 31, &[vec![long]]);
    let (ok, text, _) = convert(&[&manifest], &dir.join("out"), &["--shuffle-seed", "3"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{text}");
    assert!(text.contains("uniform games 1 visited 300 "), "{text}");
    let outside = 300 - (LAST_PLY + 1 - FIRST_PLY);
    assert!(
        text.contains(&format!(
            "uniform outside-plies-{FIRST_PLY}-{LAST_PLY} {outside} "
        )),
        "{text}"
    );
}
