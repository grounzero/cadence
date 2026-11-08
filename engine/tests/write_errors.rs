// SPDX-License-Identifier: GPL-3.0-or-later

//! A subcommand whose output cannot be written says so and exits 1, rather than exiting 0 or
//! aborting. Linux's `/dev/full` refuses every byte, as a full disk does.

mod support;

#[cfg(target_os = "linux")]
#[test]
fn a_subcommand_that_cannot_write_its_output_exits_1() {
    for args in [&["bench"][..], &["perft", "startpos", "2"], &["spsa"]] {
        let full = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .expect("opens /dev/full");
        let out = std::process::Command::new(support::binary())
            .args(args)
            .stdout(full)
            .output()
            .expect("runs cadence");
        assert_eq!(
            out.status.code(),
            Some(1),
            "cadence {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
