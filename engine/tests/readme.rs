// SPDX-License-Identifier: GPL-3.0-or-later

//! The README against the binary it describes, so an option or a subcommand added to one is in the
//! other.

mod support;

use std::process::Command;

const README: &str = include_str!("../../README.md");

/// Every option `uci` declares is named in the first column of the README's table, where a row
/// may name several, `Move Overhead` and the tuning options among them.
#[test]
fn the_readme_documents_every_uci_option() {
    let named: Vec<&str> = README
        .lines()
        .filter_map(|l| l.strip_prefix("| "))
        .filter_map(|row| row.split_once(" | "))
        .flat_map(|(cell, _)| cell.split(", "))
        .collect();
    let out = support::talk("uci\nquit\n");
    let missing: Vec<&str> = out
        .lines()
        .filter_map(|l| l.strip_prefix("option name "))
        .map(|rest| rest.split(" type ").next().unwrap_or(rest))
        .filter(|name| !named.contains(&format!("`{name}`").as_str()))
        .collect();
    assert_eq!(
        missing,
        Vec::<&str>::new(),
        "options the README's table leaves out"
    );
}

/// Every subcommand the binary lists is named in the README as `cadence <name>`.
#[test]
fn the_readme_documents_every_subcommand() {
    let out = Command::new(support::binary())
        .arg("no-such-subcommand")
        .output()
        .expect("runs cadence");
    let usage = String::from_utf8(out.stderr).expect("stderr is UTF-8");
    let listed: Vec<&str> = usage
        .lines()
        .skip_while(|l| *l != "subcommands:")
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert!(listed.len() >= 5, "no subcommand list in {usage}");
    let missing: Vec<&str> = listed
        .iter()
        .filter(|name| !README.contains(&format!("cadence {name}")))
        .copied()
        .collect();
    assert_eq!(
        missing,
        Vec::<&str>::new(),
        "subcommands the README leaves out"
    );
}
