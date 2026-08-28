// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence`: the UCI binary.
//!
//! Argument handling is a `match` on `args().nth(1)` and will stay one. The
//! engine's whole command surface is a handful of verbs, none of them
//! flag-heavy, and a parser crate would buy nothing for a dependency that
//! ends up linked into the binary that plays the games.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use cadence_engine::{bench, perft, uci};

/// The subcommand table.
///
/// `cadence` with no subcommand speaks UCI on stdin. `perft` and `bench` are
/// the whole of the rest of it: one row here and one arm below for each.
const SUBCOMMANDS: &[(&str, &str)] = &[
    (
        "perft",
        "count the legal move tree: perft [--divide] [--threads N] <fen | startpos> <depth>",
    ),
    (
        "bench",
        "the fixed-depth search over the checked-in position set; the last line is the node count",
    ),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        None => uci::run(),
        // Each subcommand is one arm here and one row in SUBCOMMANDS.
        Some("perft") => perft::run(&args[2..]),
        Some("bench") => bench::run(&args[2..]),
        Some(name) => {
            eprintln!("cadence: unknown subcommand `{name}`");
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    eprintln!();
    eprintln!("usage: cadence [<subcommand>]");
    eprintln!();
    eprintln!("With no subcommand, cadence speaks UCI on stdin.");
    if SUBCOMMANDS.is_empty() {
        return;
    }
    eprintln!();
    eprintln!("subcommands:");
    for (name, blurb) in SUBCOMMANDS {
        eprintln!("  {name:<10} {blurb}");
    }
}
