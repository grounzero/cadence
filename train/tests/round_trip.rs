// SPDX-License-Identifier: GPL-3.0-or-later

//! What `cadence-tools convert` writes, read back through the trainer's own reader and input
//! mapping: every record is a position of the source games, with the mover's score and result.

mod common;

use std::collections::HashMap;
use std::path::Path;

use bullet_lib::game::inputs::{Chess768, SparseInputType};
use bullet_lib::value::loader::{DirectSequentialDataLoader, LoadableDataType};
use bullet_trainer::reader::DataReader;
use bulletformat::ChessBoard;
use cadence_core::chess960::dfrc_fen;
use cadence_core::features::feature_index;
use cadence_core::position::Board;
use cadence_core::{Colour, generate_legal};
use cadence_dataset::chunk::{GameRecord, NO_SCORE, decode_move};
use cadence_dataset::trainer::RECORD_LEN;
use common::{convert_unit, dataset, games, scratch};

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
    let dir = scratch("round-trip");
    let chunks = [
        games(11, 40, |i| u8::from(i % 2 == 0)),
        games(12, 40, |_| 0),
    ];
    let manifest = dataset(&dir.join("data"), 5, &chunks);
    let unit = convert_unit(&manifest, &dir.join("out"));
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
