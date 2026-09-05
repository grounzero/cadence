// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence bench` and its determinism contract.
//!
//! The node count is the regression detector for the whole engine, and it
//! is only a usable signal if it is a function of the code alone. This
//! file pins what can be pinned from outside: the position list is checked
//! in and covers what it should; two runs in one process agree to the node
//! and to the move, in every position; two processes agree with each other
//! and with the in-process run; the last line is the count in the format
//! the CI step and `OpenBench` read; and the count equals `bench.txt`, so a
//! change to it is declared here before it is declared anywhere else.

mod support;

use cadence_core::position::Board;
use cadence_core::{Colour, generate_legal};
use cadence_engine::bench::{self, DEPTH, POSITIONS};
use cadence_engine::search::Limits;
use cadence_engine::time::budget;

/// The FENs of the checked-in list, comments and blank lines dropped.
fn positions() -> Vec<String> {
    POSITIONS
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Whether any castling right of `board` is a DFRC one.
fn is_dfrc(board: &Board) -> bool {
    let layout = board.layout();
    for c in Colour::ALL {
        if board.castling_rights().any(c)
            && let Some(k) = layout.king_from[c.index()].get()
            && k.file() != cadence_core::types::File::E
        {
            return true;
        }
    }
    layout.rook_from.iter().any(|r| {
        r.get().is_some_and(|sq| {
            sq.file() != cadence_core::types::File::A && sq.file() != cadence_core::types::File::H
        })
    })
}

#[test]
fn the_position_list_is_checked_in_and_covers_the_game() {
    let fens = positions();
    assert!(fens.len() >= 24, "only {} positions", fens.len());
    // Through black_box: a stub depth of zero would otherwise make this a
    // comparison clippy can decide at compile time.
    let depth = std::hint::black_box(DEPTH);
    assert!(depth >= 3, "depth {depth}");
    let mut dfrc = 0;
    let mut black = 0;
    let mut rights = 0;
    let mut endings = 0;
    let mut in_check = 0;
    for fen in &fens {
        let b = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        // Playable: the side not to move is not in check, and there is a
        // move to find. The engine survives a position that is not (a king
        // is never a target, `core/tests/opponent_in_check.rs`), but the
        // bench list is meant to be a set of positions a game could reach,
        // and this assertion catches an unreachable entry at its source.
        assert!(
            !b.opponent_in_check(),
            "{fen}: the side not to move is in check"
        );
        assert!(!generate_legal(&b).is_empty(), "{fen}: no legal move");
        if is_dfrc(&b) {
            dfrc += 1;
        }
        if b.side_to_move() == Colour::Black {
            black += 1;
        }
        if b.castling_rights() != cadence_core::CastlingRights::NONE {
            rights += 1;
        }
        if cadence_engine::eval::phase(&b) <= 6 {
            endings += 1;
        }
        if b.in_check() {
            in_check += 1;
        }
    }
    // No duplicates: a repeated position adds nothing but time.
    let mut sorted = fens.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), fens.len(), "duplicate positions");
    println!(
        "{} positions: {dfrc} DFRC, {black} Black to move, {rights} with rights, {endings} endings, {in_check} in check",
        fens.len()
    );
    assert!(dfrc >= 6, "only {dfrc} DFRC positions");
    assert!(black >= 6, "only {black} with Black to move");
    assert!(rights >= 8, "only {rights} with castling rights");
    assert!(endings >= 4, "only {endings} endings");
    assert!(in_check >= 1, "no position in check");
}

#[test]
fn the_bench_runs_under_no_time_budget() {
    // The contract at the allocation: a depth limit yields no budget, so
    // the search never reads the clock on a decision path.
    assert_eq!(budget(&Limits::depth(DEPTH), Colour::White, 0), None);
    assert_eq!(budget(&Limits::depth(DEPTH), Colour::Black, 0), None);
}

#[test]
fn two_runs_in_one_process_agree_to_the_node_in_every_position() {
    let a = bench::bench();
    let b = bench::bench();
    assert!(a.nodes > 100_000, "{} nodes is not a bench", a.nodes);
    assert_eq!(a.lines.len(), positions().len());
    assert_eq!(a.nodes, a.lines.iter().map(|l| l.nodes).sum::<u64>());
    for (x, y) in a.lines.iter().zip(&b.lines) {
        assert_eq!(x, y, "{}", x.fen);
    }
    assert_eq!(a.nodes, b.nodes);
}

/// The binary's last line, parsed: `(nodes, nps)`.
fn last_line(out: &str) -> (u64, u64) {
    let line = out.lines().last().unwrap_or_else(|| panic!("empty output"));
    let toks: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(
        toks.len(),
        4,
        "last line is not `<nodes> nodes <nps> nps`: {line:?}"
    );
    assert_eq!(toks[1], "nodes", "{line:?}");
    assert_eq!(toks[3], "nps", "{line:?}");
    (
        toks[0].parse().unwrap_or_else(|_| panic!("{line:?}")),
        toks[2].parse().unwrap_or_else(|_| panic!("{line:?}")),
    )
}

fn run_binary() -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cadence"))
        .arg("bench")
        .output()
        .expect("run cadence bench");
    assert!(out.status.success(), "exited with {:?}", out.status);
    String::from_utf8(out.stdout).expect("stdout is UTF-8")
}

#[test]
fn two_processes_agree_with_each_other_and_with_the_library() {
    let a = run_binary();
    let b = run_binary();
    let (na, _) = last_line(&a);
    let (nb, _) = last_line(&b);
    assert_eq!(na, nb, "two processes disagree:\n{a}\n{b}");
    assert!(na > 100_000, "{na} nodes is not a bench");
    // One line per position before the summary, each naming its node count,
    // so a diff between two runs says where.
    let fens = positions();
    for fen in &fens {
        assert!(
            a.lines().any(|l| l.contains(fen.as_str())),
            "no line for {fen} in {a}"
        );
    }
    let lib = bench::bench();
    assert_eq!(na, lib.nodes, "the binary and the library disagree");
}

/// `bench.txt` at the repository root holds the current expected count,
/// and the `Bench: <n>` trailer on the commit that changes it must agree
/// (the commit-msg hook enforces that). This is the local copy of the CI
/// step that diffs the last line of `cadence bench` against it.
#[test]
fn the_count_equals_bench_txt() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench.txt");
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("bench.txt is missing at the repository root ({e}); it ships with bench")
    });
    let expected: u64 = text
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("bench.txt holds {text:?}, not a count"));
    let got = bench::bench().nodes;
    assert_eq!(
        got, expected,
        "bench is {got} nodes, bench.txt says {expected}: if the change is intended, update \
         bench.txt and put `Bench: {got}` in the commit message"
    );
}
