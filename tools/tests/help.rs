// SPDX-License-Identifier: GPL-3.0-or-later

//! The usage, as a user asks for it.

/// `--help` is asked for, so its text is the output and goes to stdout, where a pager or a grep reads
/// it; only a usage error goes to stderr.
#[test]
fn help_is_printed_to_stdout() {
    for flag in ["--help", "-h"] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_cadence-tools"))
            .arg(flag)
            .output()
            .expect("run cadence-tools");
        assert!(out.status.success(), "{flag}: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("usage: cadence-tools"),
            "{flag}: {stdout:?}"
        );
        assert!(
            out.stderr.is_empty(),
            "{flag}: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
