// SPDX-License-Identifier: GPL-3.0-or-later

//! The harness the subprocess tests share, so CI's run against the release binary reaches every one
//! of them.

mod support;

use std::path::Path;

/// `--help` is asked for, so its text is the output and goes to stdout, where a pager or a grep reads
/// it; only a usage error goes to stderr.
#[test]
fn help_is_printed_to_stdout() {
    for flag in ["--help", "-h"] {
        let out = std::process::Command::new(support::binary())
            .arg(flag)
            .output()
            .expect("run cadence");
        assert!(out.status.success(), "{flag}: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("usage: cadence"), "{flag}: {stdout:?}");
        assert!(
            out.stderr.is_empty(),
            "{flag}: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// A mistyped path in CI must fail the tests rather than pass them on some other binary.
#[test]
#[should_panic(expected = "CADENCE_BIN names no file")]
fn a_cadence_bin_naming_no_file_is_refused() {
    let missing = Path::new(env!("CARGO_TARGET_TMPDIR")).join("missing");
    support::resolve(Some(missing.into_os_string()));
}

/// `support::binary` is the one place `CADENCE_BIN` is read, so a test that names the test build
/// itself is never run against the release binary.
#[test]
fn every_test_starts_the_engine_through_the_support_module() {
    let built = concat!("CARGO_BIN", "_EXE_cadence");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut direct = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read the test directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a test file");
        if text.contains(built) {
            direct.push(path.display().to_string());
        }
    }
    direct.sort();
    assert_eq!(
        direct,
        Vec::<String>::new(),
        "these start the test build directly"
    );
}
