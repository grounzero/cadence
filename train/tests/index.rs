// SPDX-License-Identifier: GPL-3.0-or-later

//! The engine's feature index against the trainer's, for every perspective, colour, piece and
//! square, compared as integers: a permutation trains a net that loads and plays badly.

use bullet_lib::game::inputs::{Chess768, ChessBucketsMirrored, SparseInputType};
use bulletformat::{BulletFormat, ChessBoard};
use cadence_core::features::{NUM_INPUTS, feature_index, mirrored_feature_index};
use cadence_core::movegen::generate_legal;
use cadence_core::position::Board;
use cadence_core::{Colour, Piece, PieceType, Square};
use cadence_dataset::trainer::{DRAW, encode};

/// The one piece, stored as the trainer stores a position: from the side to move's view.
fn record(piece: Piece, sq: Square, mover: Colour) -> ChessBoard {
    let mut bitboards = [0_u64; 8];
    let bit = 1_u64 << sq.index();
    bitboards[piece.colour().index()] |= bit;
    bitboards[2 + piece.piece_type().index()] |= bit;
    ChessBoard::from_raw(bitboards, mover.index(), 0, 0.5).expect("a one-piece record")
}

/// Each record gives the trainer's index from the mover's view and the other side's, which are
/// the engine's two perspectives.
#[test]
fn every_feature_index_is_the_trainers() {
    assert_eq!(Chess768.num_inputs(), NUM_INPUTS);
    let mut compared = 0;
    for mover in [Colour::White, Colour::Black] {
        for colour in [Colour::White, Colour::Black] {
            for kind in PieceType::ALL {
                for index in 0..64 {
                    let (piece, sq) = (Piece::new(colour, kind), Square::new(index));
                    let mut trainer = Vec::new();
                    Chess768.map_features(&record(piece, sq, mover), |stm, ntm| {
                        trainer.push((stm, ntm));
                    });
                    let engine = (
                        feature_index(mover, piece, sq),
                        feature_index(mover.flip(), piece, sq),
                    );
                    assert_eq!(trainer, [engine], "{piece:?} on {sq:?}, {mover:?} to move");
                    compared += 2;
                }
            }
        }
    }
    assert_eq!(compared, 2 * 2 * 6 * 2 * 64);
}

/// Both kings and one more piece, stored as the trainer stores a position.
fn with_kings(pieces: [(Piece, Square); 3], mover: Colour) -> ChessBoard {
    let mut bitboards = [0_u64; 8];
    for (piece, sq) in pieces {
        let bit = 1_u64 << sq.index();
        bitboards[piece.colour().index()] |= bit;
        bitboards[2 + piece.piece_type().index()] |= bit;
    }
    ChessBoard::from_raw(bitboards, mover.index(), 0, 0.5).expect("a three-piece record")
}

/// Every pair of king squares with every other piece on every other square, so each perspective
/// is read with its own king on each half while the other's stands on each. The trainer lists a
/// record's features in its own order, so the two lists are sorted before they are compared.
#[test]
fn every_mirrored_feature_index_is_the_trainers() {
    let inputs = ChessBucketsMirrored::default();
    assert_eq!(inputs.num_inputs(), NUM_INPUTS);
    let mut compared = 0_u64;
    for mover in [Colour::White, Colour::Black] {
        for white in 0..64 {
            for black in (0..64).filter(|&black| black != white) {
                let kings = [Square::new(white), Square::new(black)];
                let own = |persp: Colour| kings[persp.index()];
                for colour in [Colour::White, Colour::Black] {
                    for kind in PieceType::ALL {
                        if kind == PieceType::King {
                            continue;
                        }
                        for index in (0..64).filter(|&i| i != white && i != black) {
                            let pieces = [
                                (Piece::WKing, kings[0]),
                                (Piece::BKing, kings[1]),
                                (Piece::new(colour, kind), Square::new(index)),
                            ];
                            let mut trainer = Vec::new();
                            inputs.map_features(&with_kings(pieces, mover), |stm, ntm| {
                                trainer.push((stm, ntm));
                            });
                            let mut engine = pieces.map(|(piece, sq)| {
                                let other = mover.flip();
                                (
                                    mirrored_feature_index(mover, own(mover), piece, sq),
                                    mirrored_feature_index(other, own(other), piece, sq),
                                )
                            });
                            trainer.sort_unstable();
                            engine.sort_unstable();
                            assert_eq!(trainer, engine, "{pieces:?}, {mover:?} to move");
                            compared += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(compared, 2 * 64 * 63 * 2 * 5 * 62);
}

/// Castling positions, Chess960's among them, each with the file White's king and then Black's
/// lands on. Between them a king crosses the middle either way, stays on its half, stays on its
/// square, and trades squares with its rook.
const CASTLES: [(&str, [u8; 2]); 9] = [
    (
        "r3k2r/pppppppp/2n5/8/8/5N2/PPPPPPPP/R3K2R w KQkq - 0 1",
        *b"gc",
    ),
    (
        "r3k2r/pppppppp/2n5/8/8/5N2/PPPPPPPP/R3K2R w KQkq - 0 1",
        *b"cg",
    ),
    ("1k5r/pppppppp/8/8/8/8/PPPPPPPP/1K5R w Hh - 0 1", *b"gg"),
    ("r4k2/pppppppp/8/8/8/8/PPPPPPPP/R4K2 w Aa - 0 1", *b"cc"),
    ("6kr/pppppppp/8/8/8/8/PPPPPPPP/6KR w Hh - 0 1", *b"gg"),
    ("r1k5/pppppppp/8/8/8/8/PPPPPPPP/R1K5 w Aa - 0 1", *b"cc"),
    ("3k3r/pppppppp/8/8/8/8/PPPPPPPP/3K3R w Hh - 0 1", *b"gg"),
    ("r2k4/pppppppp/8/8/8/8/PPPPPPPP/R2K4 w Aa - 0 1", *b"cc"),
    ("2rk4/pppppppp/8/8/8/8/PPPPPPPP/2RK4 w Cc - 0 1", *b"cc"),
];

/// One real board: the trainer's record of it must be the bytes the converter writes, king
/// squares included, and its mirrored features the engine's index for every piece.
fn board_agrees(board: &Board) {
    let mut bitboards = [0_u64; 8];
    for sq in board.occupied() {
        let piece = board.piece_at(sq).expect("an occupied square");
        let bit = 1_u64 << sq.index();
        bitboards[piece.colour().index()] |= bit;
        bitboards[2 + piece.piece_type().index()] |= bit;
    }
    let (mover, other) = (board.side_to_move(), board.side_to_move().flip());
    let record = ChessBoard::from_raw(bitboards, mover.index(), 0, 0.5).expect("a record");
    assert_eq!(
        ChessBoard::as_bytes_slice(&[record]),
        encode(board, 0, DRAW),
        "{}",
        board.to_fen(cadence_core::FenStyle::Shredder)
    );
    let mut trainer = Vec::new();
    ChessBucketsMirrored::default().map_features(&record, |stm, ntm| trainer.push((stm, ntm)));
    let mut engine: Vec<(usize, usize)> = board
        .occupied()
        .into_iter()
        .map(|sq| {
            let piece = board.piece_at(sq).expect("an occupied square");
            (
                mirrored_feature_index(mover, board.king_square(mover), piece, sq),
                mirrored_feature_index(other, board.king_square(other), piece, sq),
            )
        })
        .collect();
    trainer.sort_unstable();
    engine.sort_unstable();
    assert_eq!(
        trainer,
        engine,
        "{}",
        board.to_fen(cadence_core::FenStyle::Shredder)
    );
}

/// The side to move castles so that its king lands on `file`, and whether the king changed half.
fn castle(board: &mut Board, file: u8) -> bool {
    let us = board.side_to_move();
    let before = board.king_square(us).file().index() > 3;
    let mut castles = Vec::new();
    for &m in generate_legal(board).as_slice() {
        if m.is_castle() {
            board.make_move(m);
            let lands = board.king_square(us).file().index();
            board.unmake_move(m);
            if lands == usize::from(file - b'a') {
                castles.push(m);
            }
        }
    }
    assert_eq!(
        castles.len(),
        1,
        "one castle to the {} file",
        char::from(file)
    );
    board.make_move(castles[0]);
    before != (board.king_square(us).file().index() > 3)
}

/// Each castling position is compared before any castle, after White's and after Black's, so both
/// perspectives are read on either side of a king's crossing.
#[test]
fn castling_positions_mirror_as_the_trainer_does() {
    let mut crossings = 0;
    for (fen, files) in CASTLES {
        let mut board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        board_agrees(&board);
        for file in files {
            crossings += usize::from(castle(&mut board, file));
            board_agrees(&board);
        }
    }
    assert_eq!(crossings, 8);
}
