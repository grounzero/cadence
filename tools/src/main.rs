// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-tools`: magic generation, net inspection and format conversion.
//!
//! Subcommand dispatch is a `match` on the first argument, the same shape as
//! the engine binary. Nothing here ships; it is the workbench.

#![forbid(unsafe_code)]

mod magics;

use std::process::ExitCode;

const SUBCOMMANDS: &[(&str, &str)] = &[(
    "magics",
    "search for the sliding-attack magic numbers and print them as Rust; optional seed",
)];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("magics") => magics::run(&args[2..]),
        Some(other) => Err(format!("unknown subcommand `{other}`")),
        None => Err("no subcommand".to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cadence-tools: {e}");
            usage();
            ExitCode::from(2)
        }
    }
}

fn usage() {
    eprintln!();
    eprintln!("usage: cadence-tools <subcommand> [args]");
    eprintln!();
    eprintln!("subcommands:");
    for (name, blurb) in SUBCOMMANDS {
        eprintln!("  {name:<10} {blurb}");
    }
}
