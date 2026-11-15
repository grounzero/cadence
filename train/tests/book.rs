// SPDX-License-Identifier: GPL-3.0-or-later

//! A tuning set's lines, each a FEN and White's result in brackets, converted and read back through
//! the trainer's own reader and input mapping: every record a line's position, scored zero, with the
//! side to move's result.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use bullet_lib::game::inputs::{Chess768, SparseInputType};
use bullet_lib::value::loader::{DirectSequentialDataLoader, LoadableDataType};
use bullet_trainer::reader::DataReader;
use bulletformat::ChessBoard;
use cadence_core::chess960::dfrc_fen;
use cadence_core::features::feature_index;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{Colour, FenStyle, generate_legal};
use cadence_dataset::trainer::{DRAW, LOSS, RECORD_LEN, WIN};
use cadence_train::book::{convert, parse_line};

const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Positions from random walks, standard and DFRC, each with a random result for White, as lines.
fn book(seed: u64, count: usize) -> (String, Vec<(Board, u8)>) {
    let mut rng = Rng::new(seed);
    let mut text = String::new();
    let mut lines = Vec::new();
    while lines.len() < count {
        let fen = if rng.below(2) == 0 {
            START.to_string()
        } else {
            dfrc_fen(rng.below(960) as u32, rng.below(960) as u32)
        };
        let mut board = Board::from_fen(&fen).expect("a start");
        for _ in 0..rng.below(80) {
            let legal = generate_legal(&board);
            let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                break;
            };
            board.play(m);
        }
        let (white, spelled) = [(WIN, "1.0"), (DRAW, "0.5"), (LOSS, "0.0")][rng.below(3)];
        let mut fen = board.to_fen(FenStyle::Shredder);
        if rng.below(4) == 0 {
            fen = fen.split(' ').take(4).collect::<Vec<_>>().join(" ");
        }
        let _ = writeln!(text, "{fen} [{spelled}]");
        lines.push((board, white));
    }
    (text, lines)
}

/// One pass over `path` through the trainer's reader, which cycles until told to stop.
fn read(path: &Path) -> Vec<ChessBoard> {
    let len = std::fs::metadata(path).expect("written").len();
    let want = usize::try_from(len).expect("a small file") / RECORD_LEN;
    let loader = DirectSequentialDataLoader::new(&[path.to_str().expect("a UTF-8 path")]);
    let mut boards = Vec::with_capacity(want);
    loader.read_chunks(0, |chunk: &[ChessBoard]| {
        boards.extend_from_slice(chunk);
        boards.len() >= want
    });
    boards.truncate(want);
    boards
}

#[test]
fn a_line_is_a_fen_and_whites_result() {
    let (board, white) = parse_line(&format!("{START} [1.0]")).expect("a line");
    assert_eq!((board.side_to_move(), white), (Colour::White, WIN));
    let short = "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - [0.0]";
    let (board, white) = parse_line(short).expect("four fields");
    assert_eq!((board.side_to_move(), white), (Colour::Black, LOSS));
    assert_eq!(
        parse_line(&format!("{START} [0.5]")).expect("a draw").1,
        DRAW
    );
}

#[test]
fn anything_else_is_refused_by_line() {
    for line in [
        START.to_string(),
        format!("{START} [1]"),
        format!("{START} [1.0] c9"),
        format!("{START} [1.0"),
        "8/8/8/8 w - - [0.5]".to_string(),
    ] {
        assert!(parse_line(&line).is_err(), "{line} parsed");
    }
    let text = format!("{START} [1.0]\n\n{START} [2.0]\n");
    let refused = convert(&text, 1).expect_err("refused");
    assert!(refused.starts_with("line 3: "), "{refused}");
}

/// Keys each position as the trainer's inputs map it, beside the side to move's result.
type Seen = (Vec<(usize, usize)>, i16, u8);

#[test]
fn the_trainer_reads_each_line_with_the_movers_result() {
    let (text, lines) = book(7, 3_000);
    let (records, counts) = convert(&text, 0x2026_1115).expect("converts");
    assert_eq!(counts.positions, 3_000);
    let path: PathBuf =
        std::env::temp_dir().join(format!("cadence-book-{}.bin", std::process::id()));
    std::fs::write(
        &path,
        records.iter().flatten().copied().collect::<Vec<u8>>(),
    )
    .expect("writes");
    let mut meant: HashMap<Seen, Vec<Colour>> = HashMap::new();
    for (board, white) in &lines {
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
        let result = if mover == Colour::White {
            *white
        } else {
            WIN - white
        };
        meant.entry((inputs, 0, result)).or_default().push(mover);
    }
    let boards = read(&path);
    let _ = std::fs::remove_file(&path);
    assert_eq!(boards.len(), 3_000);
    let mut black = 0;
    for (i, board) in boards.iter().enumerate() {
        let mut inputs = Vec::new();
        Chess768.map_features(board, |stm, ntm| inputs.push((stm, ntm)));
        inputs.sort_unstable();
        let key = (
            inputs,
            LoadableDataType::score(board),
            LoadableDataType::result(board) as u8,
        );
        let mover = meant
            .get_mut(&key)
            .and_then(Vec::pop)
            .unwrap_or_else(|| panic!("record {i} is no line of the book as written"));
        black += usize::from(mover == Colour::Black);
    }
    assert!(
        black > 900 && black < 2_100,
        "{black} of 3000 with Black to move"
    );
    assert_eq!(counts.white_to_move, 3_000 - black as u64);
}

#[test]
fn the_seed_decides_the_order_and_nothing_else() {
    let (text, _) = book(8, 500);
    let (a, _) = convert(&text, 1).expect("converts");
    let (b, _) = convert(&text, 1).expect("converts");
    let (c, _) = convert(&text, 2).expect("converts");
    assert_eq!(a, b);
    assert_ne!(a, c);
    let (mut a, mut c) = (a, c);
    a.sort_unstable();
    c.sort_unstable();
    assert_eq!(a, c);
}
