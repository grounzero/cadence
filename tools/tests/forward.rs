// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-tools forward` on seeded nets: the net file decides, against an f32 pass over the file
//! itself, and the trainer's raw weights are a reading that decides nothing either way.

use std::path::{Path, PathBuf};
use std::process::Command;

use cadence_nnue::net::{HEADER_LEN, QA, QB};
use cadence_nnue::{HIDDEN, Network, Provenance, WIDE};

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

/// The file's weights as the trainer would have saved them unquantised, `hidden` a perspective,
/// with `edit` applied.
fn raw(file: &[u8], hidden: usize, edit: impl Fn(usize, f32) -> f32) -> Vec<u8> {
    let words: Vec<i16> = file[HEADER_LEN..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&w| i16::from_le_bytes(w))
        .collect();
    let outputs = words.len() - 2 * hidden - 1;
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

/// A wide net's file is read at the width its header names and agrees with its own reference and
/// its unquantised weights, as a narrow one does.
#[test]
fn a_wide_net_agrees_with_its_file_and_its_unquantised_weights() {
    let dir = scratch("wide");
    let file = Network::<WIDE>::generate(5, Provenance::SelfPlay).to_bytes();
    let net = write(&dir, "net.cdnn", &file);
    let exact = write(&dir, "raw.bin", &raw(&file, WIDE, |_, w| w));
    let (ok, text) = forward(&["--net", &net, "--raw", &exact]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{text}");
    assert!(text.contains("hidden width 512: max"), "{text}");
    assert_eq!(text.matches(", PASS (bounds 5 and 1)").count(), 1, "{text}");
    assert!(text.contains("within the bounds"), "{text}");
}

#[test]
fn a_net_agrees_with_its_file_and_its_unquantised_weights() {
    let dir = scratch("agree");
    let file = Network::seeded(5, Provenance::SelfPlay).to_bytes();
    let net = write(&dir, "net.cdnn", &file);
    let exact = write(&dir, "raw.bin", &raw(&file, HIDDEN, |_, w| w));
    let (ok, text) = forward(&["--net", &net, "--raw", &exact]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{text}");
    assert_eq!(text.matches(", PASS (bounds 5 and 1)").count(), 1, "{text}");
    assert!(
        text.contains("against the net file, 1000 positions"),
        "{text}"
    );
    assert!(text.contains("decides nothing: max"), "{text}");
    assert!(text.contains("within the bounds"), "{text}");
}

/// Output weights a tenth larger in the raw file move every position they reach, which the reading
/// reports and the exit ignores.
#[test]
fn raw_weights_that_differ_are_reported_and_decide_nothing() {
    let dir = scratch("differ");
    let file = Network::seeded(5, Provenance::SelfPlay).to_bytes();
    let net = write(&dir, "net.cdnn", &file);
    let words = (file.len() - HEADER_LEN) / 2;
    let outputs = words - 2 * HIDDEN - 1;
    let wrong = raw(&file, HIDDEN, |i, w| {
        if i >= outputs && i < words - 1 {
            w * 1.1
        } else {
            w
        }
    });
    let wrong = write(&dir, "raw.bin", &wrong);
    let (ok, text) = forward(&["--net", &net, "--raw", &wrong]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{text}");
    assert!(
        text.contains("against the net file, 1000 positions"),
        "{text}"
    );
    assert!(text.contains(", PASS (bounds"), "{text}");
    assert!(text.contains("outside the bounds"), "{text}");
    assert!(!text.contains("FAIL"), "{text}");
}

#[test]
fn too_few_positions_or_a_short_raw_file_is_refused() {
    let dir = scratch("refused");
    let file = Network::seeded(5, Provenance::SelfPlay).to_bytes();
    let net = write(&dir, "net.cdnn", &file);
    let short = write(&dir, "raw.bin", &raw(&file, HIDDEN, |_, w| w)[4..]);
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

/// The engine's own scores at random positions, as the trainer's evaluation would print them, each
/// shifted by `shift` centipawns.
fn trainer_file(net: &Network, shift: f64) -> String {
    let mut rng = cadence_core::rng::Rng::new(77);
    let mut out = String::new();
    for _ in 0..200 {
        let mut board =
            cadence_core::position::Board::from_fen(cadence_core::START_FEN).expect("a start");
        for _ in 0..rng.below(60) {
            let legal = cadence_core::generate_legal(&board);
            let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                break;
            };
            board.play(m);
        }
        let cp = f64::from(net.evaluate(&net.refresh(&board), board.side_to_move())) + shift;
        let fen = board.to_fen(cadence_core::FenStyle::XFen);
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{cp:.4}\t{fen}\n"));
    }
    out
}

/// The trainer's own outputs are compared with the reference over the raw weights at the same
/// positions, so a reference that has drifted from the trainer shows as distance, not as a pass.
#[test]
fn the_reference_is_measured_against_the_trainers_own_outputs() {
    let dir = scratch("against");
    let seeded = Network::seeded(5, Provenance::SelfPlay);
    let file = seeded.to_bytes();
    let net = write(&dir, "net.cdnn", &file);
    let exact = write(&dir, "raw.bin", &raw(&file, HIDDEN, |_, w| w));
    let same = write(&dir, "same.txt", trainer_file(&seeded, 0.0).as_bytes());
    let off = write(&dir, "off.txt", trainer_file(&seeded, 50.0).as_bytes());
    let (ok, close) = forward(&["--net", &net, "--raw", &exact, "--against", &same]);
    assert!(ok, "{close}");
    assert!(
        close.contains("against the trainer's own evaluation of 200 positions"),
        "{close}"
    );
    assert!(
        close.contains("integer path max |delta| 0.000 cp"),
        "{close}"
    );
    let reference_max = |text: &str| -> f64 {
        let at = text
            .find("reference max |delta| ")
            .expect("a reference reading")
            + 22;
        text[at..]
            .split(' ')
            .next()
            .expect("a number")
            .parse()
            .expect("a number")
    };
    assert!(reference_max(&close) < 1.0, "{close}");
    let (ok, far) = forward(&["--net", &net, "--raw", &exact, "--against", &off]);
    assert!(ok, "{far}");
    assert!((reference_max(&far) - 50.0).abs() < 1.0, "{far}");
    let (ok, refused) = forward(&["--net", &net, "--against", &same]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !ok && refused.contains("--against needs --raw"),
        "{refused}"
    );
}

/// The file with its output weights and bias replaced by `raw`'s times `factor`, quantised once,
/// which is what an export with a scale factor writes.
fn folded(file: &[u8], raw: &[u8], hidden: usize, factor: f64) -> Vec<u8> {
    let words = (file.len() - HEADER_LEN) / 2;
    let mut out = file.to_vec();
    for i in words - 2 * hidden - 1..words {
        let scale = if i == words - 1 { QA * QB } else { QB };
        let mut float = [0; 4];
        float.copy_from_slice(&raw[4 * i..4 * i + 4]);
        let scaled = (f64::from(scale) * f64::from(f32::from_le_bytes(float)) * factor).round();
        let at = HEADER_LEN + 2 * i;
        out[at..at + 2].copy_from_slice(&(scaled as i16).to_le_bytes());
    }
    out
}

/// A net scaled at export is held to its unquantised weights times the factor, within the error
/// the unscaled export already had and the bounds more; the wrong factor fails it.
#[test]
fn a_scaled_net_is_held_to_its_raw_weights_times_the_factor() {
    let dir = scratch("scaled");
    let file = Network::seeded(11, Provenance::SelfPlay).to_bytes();
    let outputs = (file.len() - HEADER_LEN) / 2 - 2 * HIDDEN - 1;
    // Each output weight a third of a step off its quantised value, as a trained one would be.
    let unquantised = raw(
        &file,
        HIDDEN,
        |i, x| {
            if i >= outputs { x + 0.3 / 64.0 } else { x }
        },
    );
    let net = write(&dir, "net.cdnn", &file);
    let raw_path = write(&dir, "raw.bin", &unquantised);
    let scaled = write(
        &dir,
        "scaled.cdnn",
        &folded(&file, &unquantised, HIDDEN, 0.8),
    );
    let flags = ["--net", &scaled, "--raw", &raw_path, "--unscaled", &net];
    let (held, text) = forward(&[&flags[..], &["--scale-factor", "0.8"]].concat());
    let (wrong, other) = forward(&[&flags[..], &["--scale-factor", "0.2"]].concat());
    let (reading, alone) = forward(&[
        "--net",
        &scaled,
        "--raw",
        &raw_path,
        "--scale-factor",
        "0.8",
    ]);
    let (bare, why) = forward(&["--net", &scaled, "--scale-factor", "0.8"]);
    let (lone, lone_why) = forward(&["--net", &scaled, "--raw", &raw_path, "--unscaled", &net]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        held && text.contains("after scaling by 0.8") && text.contains(", PASS (the unscaled"),
        "{text}"
    );
    assert!(!wrong && other.contains(", FAIL (the unscaled"), "{other}");
    assert!(
        reading && alone.contains("against the raw weights times 0.8"),
        "{alone}"
    );
    assert!(!bare && why.contains("--scale-factor reads --raw"), "{why}");
    assert!(
        !lone && lone_why.contains("--unscaled is read with --scale-factor"),
        "{lone_why}"
    );
}
