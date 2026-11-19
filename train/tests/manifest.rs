// SPDX-License-Identifier: GPL-3.0-or-later

//! A run trains only on the file its manifest names by SHA-256: a changed byte, a holdout split or
//! another dataset's training file is refused, and the command refuses before it builds a trainer.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use cadence_train::data::{
    BOOK_HEADER, COMMIT_FILE, VERIFIED_MANIFESTS, matched, record_manifests, training_sha256,
    verify, verify_export, verify_holdout,
};
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

/// `cadence-train run` with these arguments: whether it succeeded, and what it printed.
fn train(args: &[&std::ffi::OsStr]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cadence-train"))
        .arg("run")
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

/// A file passed twice, or two files holding the same records, would be trained on twice.
#[test]
fn the_same_records_twice_are_refused() {
    let dir = scratch("same-records");
    let (a, _) = two_units(&dir);
    let (train_a, manifest_a) = (a.join("train.bin"), a.join("MANIFEST.tsv"));
    let copy = dir.join("copy.bin");
    std::fs::copy(&train_a, &copy).expect("copies");
    for second in [&train_a, &copy] {
        let refused = verify(&[&train_a, second], &[&manifest_a]).expect_err("the same records");
        assert!(refused.contains("the same records as"), "{refused}");
    }
    let (ok, text) = train(&[
        "--data".as_ref(),
        train_a.as_os_str(),
        "--data".as_ref(),
        copy.as_os_str(),
        "--manifest".as_ref(),
        manifest_a.as_os_str(),
    ]);
    assert!(!ok && text.contains("the same records as"), "{text}");
    assert!(!text.contains("Training Preamble"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The holdout is checked only against a manifest the training data matched, so another
/// dataset's holdout is refused even beside its own manifest.
#[test]
fn the_holdout_is_the_training_datas_own() {
    let dir = scratch("own-holdout");
    let (a, b) = two_units(&dir);
    let (manifest_a, manifest_b) = (a.join("MANIFEST.tsv"), b.join("MANIFEST.tsv"));
    let verified =
        matched(&[&a.join("train.bin")], &[&manifest_a, &manifest_b]).expect("matches A");
    assert_eq!(verified, vec![manifest_a.as_path()]);
    assert_eq!(verify_holdout(&a.join("holdout.bin"), &verified), Ok(()));
    assert!(verify_holdout(&b.join("holdout.bin"), &verified).is_err());
    let (ok, text) = train(&[
        "--data".as_ref(),
        a.join("train.bin").as_os_str(),
        "--manifest".as_ref(),
        manifest_a.as_os_str(),
        "--manifest".as_ref(),
        manifest_b.as_os_str(),
        "--holdout".as_ref(),
        b.join("holdout.bin").as_os_str(),
        "--out".as_ref(),
        dir.join("checkpoints").as_os_str(),
        "--id".as_ref(),
        "refused".as_ref(),
    ]);
    assert!(
        !ok && text.contains("only a manifest naming a --data file counts"),
        "{text}"
    );
    assert!(!dir.join("checkpoints").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A second run in one directory would overwrite the first's record of what trained its
/// checkpoints; `--superbatches 0` stops the command if the check does not.
#[test]
fn a_run_never_shares_its_directory() {
    let dir = scratch("shared-directory");
    let (a, _) = two_units(&dir);
    let used = dir.join("checkpoints");
    for name in [COMMIT_FILE, VERIFIED_MANIFESTS] {
        let _ = std::fs::remove_dir_all(&used);
        std::fs::create_dir_all(&used).expect("a directory");
        std::fs::write(used.join(name), "an earlier run\n").expect("writes");
        let (ok, text) = train(&[
            "--data".as_ref(),
            a.join("train.bin").as_os_str(),
            "--manifest".as_ref(),
            a.join("MANIFEST.tsv").as_os_str(),
            "--holdout".as_ref(),
            a.join("holdout.bin").as_os_str(),
            "--out".as_ref(),
            used.as_os_str(),
            "--id".as_ref(),
            "refused".as_ref(),
            "--superbatches".as_ref(),
            "0".as_ref(),
        ]);
        assert!(
            !ok && text.contains("never shares its directory"),
            "{name}: {text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a run records beside its checkpoints admits each manifest it verified and nothing else.
#[test]
fn an_export_names_only_a_manifest_its_run_verified() {
    let dir = scratch("verified-manifests");
    let (a, b) = two_units(&dir);
    let (manifest_a, manifest_b) = (a.join("MANIFEST.tsv"), b.join("MANIFEST.tsv"));
    let run = dir.join("checkpoints");
    std::fs::create_dir_all(&run).expect("a directory");
    let read = |path: &Path| std::fs::read(path).expect("reads");
    assert!(
        verify_export(&run, &read(&manifest_a)).is_err(),
        "no record"
    );
    record_manifests(&run, &[&manifest_a]).expect("records");
    assert_eq!(verify_export(&run, &read(&manifest_a)), Ok(()));
    let refused = verify_export(&run, &read(&manifest_b)).expect_err("B was not verified");
    assert!(refused.contains("is not one the run verified"), "{refused}");
    let mut changed = read(&manifest_a);
    changed.push(b'\n');
    assert!(verify_export(&run, &changed).is_err(), "a byte added");
    let _ = std::fs::remove_dir_all(&dir);
}
