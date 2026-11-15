// SPDX-License-Identifier: GPL-3.0-or-later

//! Validation loss from the trainer's own forward pass: a holdout record read back as the position
//! it was trained on, the trainer's loss on it, and the holdout held to its manifest. The trainer's
//! outputs need a GPU, so they are read on one and not here.

mod common;

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use bulletformat::ChessBoard;
use cadence_core::chess960::dfrc_fen;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{FenStyle, START_FEN, generate_legal};
use cadence_dataset::trainer::encode;
use cadence_train::data::{holdout_sha256, verify, verify_holdout};
use cadence_train::validate::{Example, example, loss, training_loss};
use common::{convert_unit, dataset, games, scratch};

/// Positions from random walks, standard and DFRC, with either side to move.
fn positions(seed: u64, count: usize) -> Vec<Board> {
    let mut rng = Rng::new(seed);
    (0..count)
        .map(|_| {
            let fen = if rng.below(2) == 0 {
                START_FEN.to_string()
            } else {
                dfrc_fen(rng.below(960) as u32, rng.below(960) as u32)
            };
            let mut board = Board::from_fen(&fen).expect("a start");
            for _ in 0..rng.below(90) {
                let legal = generate_legal(&board);
                let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                    break;
                };
                board.play(m);
            }
            board
        })
        .collect()
}

/// The FEN a record decodes to, read by the trainer's own parser, is the record it trained on:
/// the same occupancy, pieces, score and result, with either side to move in the source.
#[test]
fn a_holdout_record_reads_back_as_the_position_it_trained_on() {
    let mut rng = Rng::new(9);
    let mut black = 0;
    for board in positions(31, 600) {
        let score = i16::try_from(rng.below(2_001)).expect("small") - 1_000;
        let result = u8::try_from(rng.below(3)).expect("small");
        let record = encode(&board, score, result);
        let decoded = example(&record).expect("a record decodes");
        assert_eq!((decoded.score, decoded.result), (score, result));
        let spelled = ["0.0", "0.5", "1.0"][usize::from(result)];
        let parsed: ChessBoard = format!("{} | {score} | {spelled}", decoded.fen)
            .parse()
            .unwrap_or_else(|e| panic!("{}: {e}", decoded.fen));
        let mut occupancy = [0; 8];
        occupancy.copy_from_slice(&record[..8]);
        assert_eq!(
            parsed.occ,
            u64::from_le_bytes(occupancy),
            "{}",
            board.to_fen(FenStyle::Shredder)
        );
        assert_eq!(
            &parsed.pcs[..],
            &record[8..24],
            "{}",
            board.to_fen(FenStyle::Shredder)
        );
        assert_eq!((parsed.score, parsed.result), (score, result));
        black += usize::from(board.side_to_move() == cadence_core::Colour::Black);
    }
    assert!(black > 150, "{black} of 600 with Black to move");
}

#[test]
fn a_record_naming_no_piece_is_refused() {
    let mut record = encode(&Board::from_fen(START_FEN).expect("a start"), 0, 1);
    record[8] = 0x7;
    assert!(example(&record).is_err());
    let mut record = encode(&Board::from_fen(START_FEN).expect("a start"), 0, 1);
    record[26] = 3;
    assert!(example(&record).is_err());
}

/// The trainer's loss: the output's sigmoid against the result and scaled score blended, squared.
#[test]
fn the_loss_is_the_trainers() {
    let example = |score, result| Example {
        fen: String::new(),
        score,
        result,
    };
    assert!((loss(0.0, &example(0, 2), 1.0, 400.0) - 0.25).abs() < 1e-12);
    assert!(loss(0.0, &example(0, 1), 0.0, 400.0).abs() < 1e-12);
    assert!(loss(0.0, &example(0, 1), 0.75, 400.0).abs() < 1e-12);
    assert!(loss(20.0, &example(0, 2), 1.0, 400.0) < 1e-12);
    let target: f64 = 0.75 + 0.25 / (1.0 + (-1.0_f64).exp());
    let want = (0.5 - target) * (0.5 - target);
    assert!((loss(0.0, &example(400, 2), 0.75, 400.0) - want).abs() < 1e-12);
}

#[test]
fn the_training_loss_is_the_logged_superbatchs_mean() {
    let log = "1,0,0.08\n1,32,0.07\n2,0,0.06\n2,436,0.05\n";
    assert!((training_loss(log, 1).expect("logged") - 0.075).abs() < 1e-12);
    assert!((training_loss(log, 2).expect("logged") - 0.055).abs() < 1e-12);
    assert_eq!(training_loss(log, 3), None);
}

fn train(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cadence-train"))
        .args(args)
        .output()
        .expect("cadence-train runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A book conversion holds out every `every`-th line and names that file in its manifest, beside
/// the training file, each by SHA-256.
#[test]
fn a_book_conversion_names_its_holdout() {
    let dir = scratch("book-holdout");
    let book = dir.join("set.book");
    let mut lines = String::new();
    for board in positions(41, 40) {
        let _ = writeln!(lines, "{} [0.5]", board.to_fen(FenStyle::XFen));
    }
    std::fs::write(&book, lines).expect("writes");
    let records = dir.join("set.bin");
    let (ok, text) = train(&[
        "book",
        "--in",
        &path(&book),
        "--out",
        &path(&records),
        "--seed",
        "1",
        "--every",
        "4",
    ]);
    assert!(ok, "{text}");
    let (holdout, manifest) = (dir.join("set.bin.holdout"), dir.join("set.bin.manifest"));
    assert_eq!(std::fs::metadata(&holdout).expect("written").len(), 10 * 32);
    assert_eq!(std::fs::metadata(&records).expect("written").len(), 30 * 32);
    let text = std::fs::read_to_string(&manifest).expect("a manifest");
    assert!(holdout_sha256(&text).is_ok(), "{text}");
    assert_eq!(verify_holdout(&holdout, &[&manifest]), Ok(()));
    assert!(verify_holdout(&records, &[&manifest]).is_err());
    assert!(verify(&[&holdout], &[&manifest]).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Both commands check the holdout against the manifest, and a checkpoint for being one, before
/// either builds a trainer.
#[test]
fn run_and_validate_refuse_before_building_a_trainer() {
    let dir = scratch("validate-refuses");
    let a = convert_unit(
        &dataset(&dir.join("a"), 5, &[games(21, 30, |_| 0)]),
        &dir.join("out-a"),
    );
    let b = convert_unit(
        &dataset(&dir.join("b"), 6, &[games(22, 30, |_| 0)]),
        &dir.join("out-b"),
    );
    let (train_a, holdout_a, manifest_a) = (
        a.join("train.bin"),
        a.join("holdout.bin"),
        a.join("MANIFEST.tsv"),
    );
    let checkpoints = dir.join("checkpoints");
    let run = |holdout: Option<&Path>| {
        let mut args = vec![
            "run".to_string(),
            "--data".into(),
            path(&train_a),
            "--manifest".into(),
            path(&manifest_a),
        ];
        if let Some(h) = holdout {
            args.extend(["--holdout".to_string(), path(h)]);
        }
        args.extend([
            "--out".to_string(),
            path(&checkpoints),
            "--id".into(),
            "refused".into(),
        ]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        train(&args)
    };
    let (ok, text) = run(None);
    assert!(!ok && text.contains("--holdout"), "{text}");
    let (ok, text) = run(Some(&b.join("holdout.bin")));
    assert!(!ok && text.contains("is not the holdout split"), "{text}");
    assert!(!text.contains("Training Preamble"), "{text}");
    assert!(!checkpoints.exists());
    let (ok, text) = train(&[
        "validate",
        "--checkpoint",
        &path(&dir),
        "--holdout",
        &path(&holdout_a),
        "--manifest",
        &path(&manifest_a),
    ]);
    assert!(
        !ok && text.contains("is not a checkpoint the trainer saved"),
        "{text}"
    );
    assert!(!text.contains("Training Preamble"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
