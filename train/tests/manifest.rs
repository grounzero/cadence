// SPDX-License-Identifier: GPL-3.0-or-later

//! A run trains only on the file its manifest names by SHA-256: a changed byte, a holdout split or
//! another dataset's training file is refused, and the command refuses before it builds a trainer.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use cadence_train::data::{BOOK_HEADER, training_sha256, verify};
use cadence_train::export::CONVERTED;
use common::{convert_unit, dataset, games, scratch};

/// Two converted datasets, each a unit with its training file, holdout and manifest.
fn two_units(dir: &Path) -> (PathBuf, PathBuf) {
    let a = convert_unit(
        &dataset(&dir.join("a"), 5, &[games(21, 30, |_| 0)]),
        &dir.join("out-a"),
    );
    let b = convert_unit(
        &dataset(&dir.join("b"), 6, &[games(22, 30, |_| 0)]),
        &dir.join("out-b"),
    );
    (a, b)
}

fn changed(path: &Path, to: &Path) {
    let mut bytes = std::fs::read(path).expect("reads");
    bytes[40] ^= 1;
    std::fs::write(to, bytes).expect("writes");
}

#[test]
fn a_training_file_matches_only_its_own_manifest() {
    let dir = scratch("run-manifest");
    let (a, b) = two_units(&dir);
    let (train_a, manifest_a) = (a.join("train.bin"), a.join("MANIFEST.tsv"));
    assert_eq!(verify(&[&train_a], &[&manifest_a]), Ok(()));
    let crossed = verify(&[&train_a], &[&b.join("MANIFEST.tsv")]).expect_err("another dataset");
    assert!(
        crossed.contains("is not the training file any manifest names"),
        "{crossed}"
    );
    verify(&[&a.join("holdout.bin")], &[&manifest_a]).expect_err("the holdout split");
    let one_byte = dir.join("changed.bin");
    changed(&train_a, &one_byte);
    verify(&[&one_byte], &[&manifest_a]).expect_err("one byte changed");
    verify(&[&train_a], &[]).expect_err("no manifest");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_book_conversion_matches_only_its_own_manifest() {
    let dir = scratch("book-manifest");
    let book = dir.join("set.book");
    std::fs::write(
        &book,
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1 [0.5]\n\
         rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1 [1.0]\n\
         rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2 [0.0]\n",
    )
    .expect("writes");
    let records = dir.join("set.bin");
    let run = Command::new(env!("CARGO_BIN_EXE_cadence-train"))
        .arg("book")
        .arg("--in")
        .arg(&book)
        .arg("--out")
        .arg(&records)
        .args(["--seed", "1"])
        .output()
        .expect("cadence-train runs");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let manifest = dir.join("set.bin.manifest");
    assert_eq!(verify(&[&records], &[&manifest]), Ok(()));
    let one_byte = dir.join("changed.bin");
    changed(&records, &one_byte);
    verify(&[&one_byte], &[&manifest]).expect_err("one byte changed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_manifest_names_exactly_one_training_file() {
    let sha = "ab".repeat(32);
    let converted = format!(
        "{CONVERTED}file\ttrain.bin\t{sha}\t10\nfile\tholdout.bin\t{}\t2\n",
        "cd".repeat(32)
    );
    assert_eq!(training_sha256(&converted), Ok(sha.clone()));
    let book = format!("{BOOK_HEADER}\nrecords-sha256\t{sha}\n");
    assert_eq!(training_sha256(&book), Ok(sha.clone()));
    for refused in [
        format!("# a list of files\nfile\ttrain.bin\t{sha}\t10\n"),
        format!("{CONVERTED}file\tholdout.bin\t{sha}\t2\n"),
        format!("{CONVERTED}file\ttrain.bin\tnot-hex\t10\n"),
        format!("{BOOK_HEADER}\nrecords-sha256\t{sha}\nrecords-sha256\t{sha}\n"),
    ] {
        assert!(training_sha256(&refused).is_err(), "{refused}");
    }
}

/// Refused before the trainer is built: the trainer's preamble never prints.
#[test]
fn run_refuses_before_it_builds_a_trainer() {
    let dir = scratch("run-refuses");
    let (a, b) = two_units(&dir);
    let train_a = a.join("train.bin");
    let run = |extra: &[&Path]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cadence-train"));
        command.arg("run").arg("--data").arg(&train_a);
        for manifest in extra {
            command.arg("--manifest").arg(manifest);
        }
        let out = command
            .arg("--out")
            .arg(dir.join("checkpoints"))
            .args(["--id", "refused"])
            .output()
            .expect("cadence-train runs");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        (out.status.success(), text)
    };
    let (ok, text) = run(&[]);
    assert!(!ok && text.contains("--manifest"), "{text}");
    let (ok, text) = run(&[&b.join("MANIFEST.tsv")]);
    assert!(!ok && text.contains("is not the training file"), "{text}");
    assert!(!text.contains("Training Preamble"), "{text}");
    assert!(!dir.join("checkpoints").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
