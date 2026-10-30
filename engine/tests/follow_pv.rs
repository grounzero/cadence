// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether each node is on the previous iteration's line, which nothing reads yet. The record of
//! each call that these read exists only in a debug build, so these gates do too.

#![cfg(debug_assertions)]

mod support;

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use cadence_core::{Move, generate_legal, parse_uci};
use cadence_engine::search::Limits;
use support::table;

/// Kiwipete, a quiet middlegame and a pawn endgame.
fn positions() -> [String; 3] {
    [
        support::standard_fen("kiwipete"),
        MIDDLEGAME.to_string(),
        support::PAWN_ENDGAMES[0].to_string(),
    ]
}

const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// Each iteration's lines as the root reported them, best first, every move read on the board it
/// is played from.
fn reported(fen: &str, out: &[u8]) -> BTreeMap<u32, Vec<Vec<Move>>> {
    let mut lines: BTreeMap<u32, Vec<Vec<Move>>> = BTreeMap::new();
    for text in String::from_utf8_lossy(out).lines() {
        let words: Vec<&str> = text.split_whitespace().collect();
        let (Some(at), Some(pv)) = (
            words.iter().position(|&w| w == "depth"),
            words.iter().position(|&w| w == "pv"),
        ) else {
            continue;
        };
        let depth: u32 = words[at + 1].parse().expect("a depth");
        let mut board = support::position(fen);
        let mut line = Vec::new();
        for uci in &words[pv + 1..] {
            let m = parse_uci(&generate_legal(&board), uci).expect("a legal move");
            board.make_move(m);
            line.push(m);
        }
        lines.entry(depth).or_default().push(line);
    }
    lines
}

/// What a search showed: per line, the calls below the root that followed it, and the calls whose
/// parent followed and which did not.
struct Followed {
    on: Vec<usize>,
    left: usize,
}

/// Every call checked against the previous iteration's line its root search follows: the root has
/// the flag, and a node below it exactly where its path is a prefix of that line. With several
/// lines, each root search of an iteration is the next line's.
fn checked(fen: &str, depth: u32, lines: usize) -> Followed {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(fen);
    let mut s = support::search(Limits::depth(depth), &stop, &tt);
    s.set_multipv(lines);
    s.trace_calls();
    let mut out = Vec::new();
    s.run(&mut b, &mut out);
    let reported = reported(fen, &out);
    let mut followed = Followed {
        on: vec![0; lines],
        left: 0,
    };
    let mut root: Option<(u32, usize)> = None;
    let mut previous: Option<&Vec<Move>> = None;
    let mut path: Vec<Move> = Vec::new();
    let mut flags: Vec<bool> = Vec::new();
    for call in s.calls() {
        if call.ply == 0 {
            let index = match root {
                Some((d, i)) if d == call.depth && lines > 1 => i + 1,
                _ => 0,
            };
            root = Some((call.depth, index));
            previous = reported.get(&(call.depth - 1)).and_then(|l| l.get(index));
            assert!(call.follows, "{fen}: the root at depth {}", call.depth);
            path.clear();
            flags.clear();
            flags.push(true);
            continue;
        }
        path.truncate(call.ply - 1);
        flags.truncate(call.ply);
        assert_eq!(path.len() + 1, call.ply, "{fen}: no parent recorded");
        path.push(call.moved);
        let on = previous.is_some_and(|line| line.starts_with(&path));
        assert_eq!(call.follows, on, "{fen}, {lines} lines: {path:?}");
        let (_, index) = root.expect("the root first");
        followed.on[index] += usize::from(on);
        followed.left += usize::from(flags[call.ply - 1] && !on);
        flags.push(on);
    }
    followed
}

/// Nothing was searched before the first iteration, so no node below its root has a line.
#[test]
fn at_the_first_depth_the_root_alone_follows() {
    let stop = AtomicBool::new(false);
    for fen in positions() {
        let tt = table();
        let mut b = support::position(&fen);
        let mut s = support::search(Limits::depth(1), &stop, &tt);
        s.trace_calls();
        s.run(&mut b, &mut Vec::new());
        assert!(s.calls().len() > 1, "{fen}: no child searched");
        for call in s.calls() {
            assert_eq!(call.follows, call.ply == 0, "{fen}: {call:?}");
        }
    }
}

/// Along the line each node has the flag, a sibling of a line move does not, and nothing below a
/// deviation does, through the aspiration windows and every rule that searches a child.
#[test]
fn a_node_follows_exactly_where_its_path_is_the_previous_lines() {
    for fen in positions() {
        let followed = checked(&fen, 8, 1);
        assert!(followed.on[0] > 20, "{fen}: too few nodes on the line");
        assert!(followed.left > 100, "{fen}: too few nodes leaving it");
    }
}

/// Line k follows the previous iteration's line k, and not the first line.
#[test]
fn at_two_lines_each_follows_its_own_previous_line() {
    for fen in positions() {
        let followed = checked(&fen, 9, 2);
        for (k, &on) in followed.on.iter().enumerate() {
            assert!(on > 10, "{fen}: too few nodes on line {}", k + 1);
        }
    }
}
