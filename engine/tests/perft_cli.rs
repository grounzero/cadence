// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence perft` reproduces the corpus from the command line.
//!
//! Drives the real binary as a subprocess, the same way a user or a script
//! would, and compares its output with `tests/fixtures/perft-corpus.txt`, read here
//! with a small parser of its own, because the engine crate does not see
//! `core`'s test support. As there, nothing transcribes a node count into
//! Rust; the fixture is the only source of expected values.
//!
//! Depths are kept to what a debug binary runs in seconds: the point is the
//! command line, not the node counts, which the core tests already hold to
//! depth 5.

mod support;

use std::process::Command;

use support::{standard_fen, tsv};

/// Run `cadence perft <args...>`, returning stdout lines.
fn perft(args: &[&str]) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .arg("perft")
        .args(args)
        .output()
        .expect("run cadence perft");
    assert!(
        out.status.success(),
        "cadence perft {args:?} exited with {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("stdout is UTF-8")
        .lines()
        .map(str::to_string)
        .collect()
}

fn nodes_line(lines: &[String]) -> u64 {
    lines
        .iter()
        .find_map(|l| l.strip_prefix("nodes "))
        .unwrap_or_else(|| panic!("no `nodes` line in {lines:?}"))
        .parse()
        .expect("nodes is a number")
}

/// A FEN as command-line words.
fn words(fen: &str) -> Vec<&str> {
    fen.split_whitespace().collect()
}

#[test]
fn standard_suite_to_depth_4_and_startpos_to_5() {
    for row in tsv("standard-perft") {
        let (name, depth, want) = (&row[0], &row[1], row[2].parse::<u64>().expect("nodes"));
        let d: u32 = depth.parse().expect("depth");
        let limit = if name == "startpos" { 5 } else { 4 };
        if d > limit {
            continue;
        }
        let fen = standard_fen(name);
        let mut args = words(&fen);
        args.push(depth);
        let got = nodes_line(&perft(&args));
        assert_eq!(got, want, "{name} d{depth}");
    }
}

#[test]
fn startpos_keyword_is_the_start_position() {
    let via_keyword = nodes_line(&perft(&["startpos", "4"]));
    let fen = standard_fen("startpos");
    let mut args = words(&fen);
    args.push("4");
    assert_eq!(via_keyword, nodes_line(&perft(&args)));
}

#[test]
fn dfrc_arrays_to_depth_4() {
    for row in tsv("dfrc-arrays") {
        let fen = &row[2];
        let want: u64 = row[6].parse().expect("d4 nodes");
        let mut args = words(fen);
        args.push("4");
        assert_eq!(nodes_line(&perft(&args)), want, "{}/{} d4", row[0], row[1]);
    }
}

#[test]
fn castling_legality_positions_to_depth_3() {
    for row in tsv("castling-legality") {
        let fen = &row[2];
        let want: u64 = row[5].parse().expect("d3 nodes");
        let mut args = words(fen);
        args.push("3");
        assert_eq!(nodes_line(&perft(&args)), want, "{fen}");
    }
}

/// `--divide` prints one sorted `move: nodes` line per root move, and the
/// lines are exactly the corpus divide rows.
#[test]
fn divide_matches_the_corpus_rows() {
    for (name, depth) in [("startpos", "1"), ("startpos", "2"), ("kiwipete", "1")] {
        let mut want: Vec<(String, u64)> = tsv("perft-divide")
            .into_iter()
            .filter(|r| r[0] == name && r[1] == depth)
            .map(|r| (r[2].clone(), r[3].parse().expect("nodes")))
            .collect();
        want.sort();
        let mut args = vec!["--divide"];
        let fen = standard_fen(name);
        args.extend(words(&fen));
        args.push(depth);
        let lines = perft(&args);
        let got: Vec<(String, u64)> = lines
            .iter()
            .filter_map(|l| l.split_once(": "))
            .map(|(m, n)| (m.to_string(), n.parse().expect("nodes")))
            .collect();
        assert_eq!(got, want, "{name} d{depth} divide");
        assert_eq!(nodes_line(&lines), want.iter().map(|(_, n)| n).sum::<u64>());
    }
}

/// The total is a function of the position alone: one thread, several
/// threads, and more threads than root moves all agree.
#[test]
fn thread_count_does_not_change_the_total() {
    let fen = standard_fen("kiwipete");
    let mut base = words(&fen);
    base.push("3");
    let one = nodes_line(&perft(&[&["--threads", "1"], base.as_slice()].concat()));
    let four = nodes_line(&perft(&[&["--threads", "4"], base.as_slice()].concat()));
    let many = nodes_line(&perft(&[&["--threads", "200"], base.as_slice()].concat()));
    let default = nodes_line(&perft(&base));
    assert!(
        one == four && four == many && many == default,
        "{one} {four} {many} {default}"
    );
    // Depth 0 and 1 by definition.
    assert_eq!(nodes_line(&perft(&["startpos", "0"])), 1);
    assert_eq!(nodes_line(&perft(&["startpos", "1"])), 20);
}

#[test]
fn usage_errors_exit_2() {
    for args in [
        vec![],
        vec!["startpos"],
        vec!["startpos", "x"],
        vec!["--threads", "0", "startpos", "1"],
        vec!["not", "a", "fen", "1"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_cadence"))
            .arg("perft")
            .args(&args)
            .output()
            .expect("run cadence perft");
        assert_eq!(out.status.code(), Some(2), "cadence perft {args:?}");
        let err = String::from_utf8(out.stderr).expect("stderr is UTF-8");
        assert!(err.contains("cadence perft"), "{args:?}: {err}");
    }
}

/// `cadence perft` on a position the side not to move is in check in returns
/// a count instead of killing the process.
///
/// Regression: the first of these died with SIGABRT when generation offered
/// the king capture and the check info recomputed after `make_move`
/// asked for a king that had just been taken off the board. There is no
/// external oracle for a position that cannot occur, so what is asserted is
/// what the command can be held to: it exits cleanly (the `perft` helper
/// checks the status), it prints a count, and `--divide` sums to it.
#[test]
fn a_position_with_the_side_not_to_move_in_check_is_counted_not_fatal() {
    for fen in [
        "k7/8/8/8/8/8/8/R6K w - - 0 1",
        "4k3/8/8/8/8/8/8/4R2K w - - 0 1",
        "kK6/8/8/8/8/8/8/8 w - - 0 1",
    ] {
        for depth in ["1", "3"] {
            let mut args = words(fen);
            args.push(depth);
            let total = nodes_line(&perft(&args));
            assert!(total > 0, "{fen} at depth {depth}: no nodes");

            let mut divide_args = vec!["--divide"];
            divide_args.extend(words(fen));
            divide_args.push(depth);
            let lines = perft(&divide_args);
            let summed: u64 = lines
                .iter()
                .filter_map(|l| l.split_once(": "))
                .filter_map(|(_, n)| n.parse::<u64>().ok())
                .sum();
            assert_eq!(summed, total, "{fen} at depth {depth}: divide disagrees");
        }
    }
}
