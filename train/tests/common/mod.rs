// SPDX-License-Identifier: GPL-3.0-or-later

//! Packed datasets built from random legal games, and their conversion by the real
//! `cadence-tools`, shared by the tests that need the converter's own output.

#![allow(
    dead_code,
    reason = "each test file uses a different part of this module"
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use cadence_core::chess960::dfrc_fen;
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_dataset::chunk::{Chunk, GameRecord, Header, NO_SCORE, chunk_name, encode_move};
use cadence_dataset::constants::{MATE_BOUND, RANDOM_PLIES};
use cadence_dataset::sha256::{hex, sha256};

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadence-convert-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A legal game: its arrays by kind, random plies, then up to `length` random moves with scores
/// drawn small, and now and then none, a mate or an exact zero.
pub fn game(rng: &mut Rng, index: u32, kind: u8, length: usize) -> GameRecord {
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

pub fn header(run_seed: u64, chunk_index: u32) -> Header {
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
        node_cap: 0,
    }
}

/// Chunks with their sidecars and a manifest listing them, as `datagen verify --manifest` writes it.
pub fn dataset(dir: &Path, run_seed: u64, chunks: &[Vec<GameRecord>]) -> PathBuf {
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
        "# cadence dataset manifest\ncommit\t{}\nversion\t0.0.0-test\nnodes\t5000\nscreen-nodes\t5000\n\
         hash\t16\nrandom-plies\t{RANDOM_PLIES}\nwindow\t200\nstandard-share\t50\n\
         chunk-games\t1000\nrun-seeds\t{run_seed:016x}\n\
         chunk\tsha256\tgames\tplies\tuniform\tstandard\n{rows}",
        hex(&[1; 20])
    );
    std::fs::write(&manifest, text).expect("writes");
    manifest
}

pub fn games(seed: u64, count: u32, kind: impl Fn(u32) -> u8) -> Vec<GameRecord> {
    let mut rng = Rng::new(seed);
    (0..count).map(|i| game(&mut rng, i, kind(i), 60)).collect()
}

/// The `cadence-tools` binary `CADENCE_TOOLS` names, which a workspace build leaves in `target/debug`.
pub fn tools() -> PathBuf {
    std::env::var_os("CADENCE_TOOLS")
        .map(PathBuf::from)
        .expect("CADENCE_TOOLS names the cadence-tools binary these tests run")
}

/// The one unit `cadence-tools convert` publishes under `out` for the dataset at `manifest`.
pub fn convert_unit(manifest: &Path, out: &Path) -> PathBuf {
    let run = Command::new(tools())
        .arg("convert")
        .arg("--manifest")
        .arg(manifest)
        .arg("--out")
        .arg(out)
        .args(["--shuffle-seed", "1"])
        .output()
        .expect("cadence-tools runs");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read_dir(out)
        .expect("an output directory")
        .next()
        .expect("one unit")
        .expect("an entry")
        .path()
}
