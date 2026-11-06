// SPDX-License-Identifier: GPL-3.0-or-later

//! The binary as a user runs it.

/// An output that cannot be written fails with 1 rather than aborting. Linux's `/dev/full` refuses
/// every byte, as a full disk does.
#[cfg(target_os = "linux")]
#[test]
fn magics_that_cannot_be_written_exit_1() {
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("opens /dev/full");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cadence-tools"))
        .arg("magics")
        .stdout(full)
        .output()
        .expect("runs cadence-tools");
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
