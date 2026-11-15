// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-tools forward` on seeded nets: it passes a file against itself and its own exact
//! weights, and fails weights that differ, so a pass is a reading and not a default.

use std::path::{Path, PathBuf};
use std::process::Command;

use cadence_nnue::net::{HEADER_LEN, QA, QB};
use cadence_nnue::{HIDDEN, Network, Provenance};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadence-forward-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// Whether it passed, and what it printed.
fn forward(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cadence-tools"))
        .arg("forward")
        .args(args)
        .output()
        .expect("runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

/// The file's weights as the trainer would have saved them unquantised, with `edit` applied.
fn raw(file: &[u8], edit: impl Fn(usize, f32) -> f32) -> Vec<u8> {
    let words: Vec<i16> = file[HEADER_LEN..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&w| i16::from_le_bytes(w))
        .collect();
    let outputs = words.len() - 2 * HIDDEN - 1;
    let scale = |q: i32| f32::from(i16::try_from(q).expect("a scale fits i16"));
    let (qa, qb) = (scale(QA), scale(QB));
    let mut out = Vec::with_capacity(words.len() * 4);
    for (i, &w) in words.iter().enumerate() {
        let scale = if i == words.len() - 1 {
            qa * qb
        } else if i >= outputs {
            qb
        } else {
            qa
        };
        out.extend_from_slice(&edit(i, f32::from(w) / scale).to_le_bytes());
    }
    out
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("writes");
    path.to_string_lossy().into_owned()
}

#[test]
fn a_net_agrees_with_its_file_and_its_unquantised_weights() {
    let dir = scratch("agree");
    let file = Network::seeded(5, Provenance::SelfPlay).to_bytes();
    let net = write(&dir, "net.cdnn", &file);
    let exact = write(&dir, "raw.bin", &raw(&file, |_, w| w));
    let (ok, text) = forward(&["--net", &net, "--raw", &exact]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{text}");
    assert_eq!(text.matches(", PASS (bounds 5 and 1)").count(), 2, "{text}");
    assert!(
        text.contains("against the net file, 1000 positions"),
        "{text}"
    );
}

/// One output weight a tenth larger in the raw file moves every position it reaches.
#[test]
fn weights_that_differ_fail() {
    let dir = scratch("differ");
    let file = Network::seeded(5, Provenance::SelfPlay).to_bytes();
    let net = write(&dir, "net.cdnn", &file);
    let words = (file.len() - HEADER_LEN) / 2;
    let outputs = words - 2 * HIDDEN - 1;
    let wrong = raw(&file, |i, w| {
        if i >= outputs && i < words - 1 {
            w * 1.1
        } else {
            w
        }
    });
    let wrong = write(&dir, "raw.bin", &wrong);
    let (ok, text) = forward(&["--net", &net, "--raw", &wrong]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!ok, "{text}");
    assert!(
        text.contains("against the net file, 1000 positions"),
        "{text}"
    );
    assert!(text.contains(", PASS (bounds"), "{text}");
    assert!(text.contains(", FAIL (bounds"), "{text}");
}

#[test]
fn too_few_positions_or_a_short_raw_file_is_refused() {
    let dir = scratch("refused");
    let file = Network::seeded(5, Provenance::SelfPlay).to_bytes();
    let net = write(&dir, "net.cdnn", &file);
    let short = write(&dir, "raw.bin", &raw(&file, |_, w| w)[4..]);
    let (few, few_text) = forward(&["--net", &net, "--positions", "999"]);
    let (cut, cut_text) = forward(&["--net", &net, "--raw", &short]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !few && few_text.contains("999 positions, where a reading needs 1000"),
        "{few_text}"
    );
    assert!(
        !cut && cut_text.contains("bytes where a raw file is"),
        "{cut_text}"
    );
}
