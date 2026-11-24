// SPDX-License-Identifier: GPL-3.0-or-later

//! Nothing here ships. Dispatch is a `match` on the first argument, so a subcommand is a row in
//! [`SUBCOMMANDS`] and an arm beside it.

#![forbid(unsafe_code)]

mod convert;
mod forward;
mod help;
mod magics;

use std::process::ExitCode;

const SUBCOMMANDS: &[(&str, &str)] = &[
    (
        "magics",
        "search for the sliding-attack magic numbers and print them as Rust; optional seed",
    ),
    (
        "convert",
        "datasets of packed chunks to the trainer's position records, published as one unit",
    ),
    (
        "forward",
        "a net file's quantised evaluation against an f32 one over random legal positions",
    ),
];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args[1..].iter().any(|a| a == "--help" || a == "-h") {
        match args.get(1).map(String::as_str).and_then(help::text) {
            Some(text) => println!("{text}"),
            None => print!("{}", usage()),
        }
        return ExitCode::SUCCESS;
    }
    let result = match args.get(1).map(String::as_str) {
        Some("magics") => magics::run(&args[2..]),
        Some("convert") => convert::run(&args[2..]),
        Some("forward") => forward::run(&args[2..]),
        Some(other) => Err(format!("unknown subcommand `{other}`")),
        None => Err("no subcommand".to_string()),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("cadence-tools: {e}");
            eprint!("{}", usage());
            ExitCode::from(2)
        }
    }
}

/// To stdout where `--help` asked for it, and to stderr after a usage error.
fn usage() -> String {
    use std::fmt::Write;
    let mut text = String::from("\nusage: cadence-tools <subcommand> [args]\n\nsubcommands:\n");
    for (name, blurb) in SUBCOMMANDS {
        let _ = writeln!(text, "  {name:<10} {blurb}");
    }
    text
}
