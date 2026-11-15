// SPDX-License-Identifier: GPL-3.0-or-later

//! What `cadence-tools convert` writes, read back through the trainer's own reader and input
//! mapping: every record is a position of the source games, with the mover's score and result.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use bullet_lib::game::inputs::{Chess768, SparseInputType};
use bullet_lib::value::loader::{DirectSequentialDataLoader, LoadableDataType};
use bullet_trainer::reader::DataReader;
use bulletformat::ChessBoard;
use cadence_core::chess960::dfrc_fen;
use cadence_core::features::feature_index;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{Colour, generate_legal};
use cadence_dataset::chunk::{
    Chunk, GameRecord, Header, NO_SCORE, chunk_name, decode_move, encode_move,
};
use cadence_dataset::constants::{MATE_BOUND, RANDOM_PLIES};
use cadence_dataset::sha256::{hex, sha256};
use cadence_dataset::trainer::RECORD_LEN;

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
        "# cadence dataset manifest\ncommit\t{}\nversion\t0.0.0-test\nnodes\t5000\nscreen-nodes\t5000\n\
         hash\t16\nrandom-plies\t{RANDOM_PLIES}\nwindow\t200\nstandard-share\t50\n\
         run-seeds\t{run_seed:016x}\nchunk\tsha256\tgames\tplies\tuniform\tstandard\n{rows}",
        hex(&[1; 20])
    );
    std::fs::write(&manifest, text).expect("writes");
    manifest
}

fn games(seed: u64, count: u32, kind: impl Fn(u32) -> u8) -> Vec<GameRecord> {
    let mut rng = Rng::new(seed);
    (0..count).map(|i| game(&mut rng, i, kind(i), 60)).collect()
}

/// The two inputs as the trainer maps them, sorted, then the mover's score and result.
type Seen = (Vec<(usize, usize)>, i16, u8);

/// Every scored position of the games, as the converter is meant to write it, keyed as the trainer
/// reads it and kept with the side that was to move.
fn meant(chunks: &[Vec<GameRecord>]) -> HashMap<Seen, Vec<Colour>> {
    let mut out: HashMap<Seen, Vec<Colour>> = HashMap::new();
    for record in chunks.iter().flatten() {
        let fen = dfrc_fen(u32::from(record.white), u32::from(record.black));
        let mut board = Board::from_fen(&fen).expect("a start");
        let next = |board: &Board, bits| decode_move(&generate_legal(board), bits).expect("legal");
        for &bits in &record.plies {
            let m = next(&board, bits);
            board.play(m);
        }
        for &(bits, score) in &record.moves {
            let m = next(&board, bits);
            if score != NO_SCORE {
                let mover = board.side_to_move();
                let mut inputs: Vec<(usize, usize)> = board
                    .occupied()
                    .into_iter()
                    .map(|sq| {
                        let piece = board.piece_at(sq).expect("an occupied square");
                        (
                            feature_index(mover, piece, sq),
                            feature_index(mover.flip(), piece, sq),
                        )
                    })
                    .collect();
                inputs.sort_unstable();
                // White's win, Black's win or a draw, then read as the mover's loss, draw or win.
                let result = match record.outcome {
                    2 => 1,
                    o if (o == 0) == (mover == Colour::White) => 2,
                    _ => 0,
                };
                out.entry((inputs, score, result)).or_default().push(mover);
            }
            board.play(m);
        }
    }
    out
}

/// One pass over `path` through the trainer's reader, which cycles until told to stop.
fn read(path: &Path) -> Vec<ChessBoard> {
    let len = std::fs::metadata(path).expect("written").len();
    assert_eq!(len % RECORD_LEN as u64, 0, "{}", path.display());
    let want = usize::try_from(len).expect("a small file") / RECORD_LEN;
    assert!(want > 0, "{} holds no record", path.display());
    let loader = DirectSequentialDataLoader::new(&[path.to_str().expect("a UTF-8 path")]);
    let mut boards = Vec::with_capacity(want);
    loader.read_chunks(0, |chunk: &[ChessBoard]| {
        boards.extend_from_slice(chunk);
        boards.len() >= want
    });
    boards.truncate(want);
    boards
}

/// A converter that flipped the board for Black to move and left its score or result unflipped
/// writes records no position matches, which is what this catches and the converter's tests cannot.
#[test]
fn the_trainer_reads_each_record_as_the_converter_meant_it() {
    let tools = std::env::var_os("CADENCE_TOOLS")
        .map(PathBuf::from)
        .expect("CADENCE_TOOLS names the cadence-tools binary this test runs");
    let dir = scratch("round-trip");
    let chunks = [
        games(11, 40, |i| u8::from(i % 2 == 0)),
        games(12, 40, |_| 0),
    ];
    let manifest = dataset(&dir.join("data"), 5, &chunks);
    let out = dir.join("out");
    let run = Command::new(&tools)
        .arg("convert")
        .arg("--manifest")
        .arg(&manifest)
        .arg("--out")
        .arg(&out)
        .args(["--shuffle-seed", "1"])
        .output()
        .expect("cadence-tools runs");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let unit = std::fs::read_dir(&out)
        .expect("an output directory")
        .next()
        .expect("one unit")
        .expect("an entry")
        .path();
    let mut meant = meant(&chunks);
    let (mut seen, mut black) = (0, 0);
    for split in ["train.bin", "holdout.bin"] {
        for board in read(&unit.join(split)) {
            let mut inputs = Vec::new();
            Chess768.map_features(&board, |stm, ntm| inputs.push((stm, ntm)));
            inputs.sort_unstable();
            let key = (
                inputs,
                LoadableDataType::score(&board),
                LoadableDataType::result(&board) as u8,
            );
            let mover = meant.get_mut(&key).and_then(Vec::pop).unwrap_or_else(|| {
                panic!("record {seen} of {split} is no position the games hold")
            });
            black += usize::from(mover == Colour::Black);
            seen += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(seen > 500, "{seen} records read");
    assert!(
        black > seen / 4 && black < seen * 3 / 4,
        "{black} of {seen} with Black to move"
    );
}
