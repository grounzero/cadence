// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-train eval` reads a checkpoint the trainer saved, and refuses anything else before it
//! builds a trainer; its outputs need a GPU, so they are read on one and not here.

use std::process::Command;

fn eval(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cadence-train"))
        .arg("eval")
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

#[test]
fn eval_refuses_what_is_not_a_checkpoint() {
    let dir = std::env::temp_dir().join(format!("cadence-eval-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a directory");
    let fens = dir.join("fens.txt");
    std::fs::write(
        &fens,
        "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1\n",
    )
    .expect("writes");
    let dir_text = dir.to_string_lossy().into_owned();
    let fens_text = fens.to_string_lossy().into_owned();
    let (ok, text) = eval(&["--checkpoint", &dir_text, "--fens", &fens_text]);
    assert!(
        !ok && text.contains("is not a checkpoint the trainer saved"),
        "{text}"
    );
    assert!(!text.contains("Training Preamble"), "{text}");
    let (ok, text) = eval(&["--checkpoint", &dir_text]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!ok && text.contains("--fens"), "{text}");
}
