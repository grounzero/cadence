// SPDX-License-Identifier: GPL-3.0-or-later

//! The count of extensions each line takes, capped at the root depth. The record of each call these
//! read exists only in a debug build, so these gates do too.

#![cfg(debug_assertions)]

mod support;

use std::sync::atomic::AtomicBool;

use cadence_engine::search::Limits;
use support::table;

/// Checks for one side, for both, and a rook beside a queen; the last two run lines of checks past
/// twice the root depth when only the ply limits the extension.
const CHECKING: [&str; 5] = [
    "7k/8/8/8/8/8/8/Q6K w - - 0 1",
    "1q5k/8/8/8/8/8/8/1Q5K w - - 0 1",
    "8/8/4k3/8/8/2Q5/8/4K2R w - - 0 1",
    "3qk3/8/8/8/8/8/8/3QK3 w - - 0 1",
    "4k3/8/8/8/8/8/q7/4K2Q w - - 0 1",
];

/// What a search showed: the rises in the count, the calls at the cap, and the deepest interior
/// ply against its iteration's root depth.
struct Counted {
    rises: usize,
    capped: usize,
    past: Vec<(usize, u32)>,
}

/// Every call against its parent, the last call one ply up: the root has no extension, the count
/// rises by one at most, and it never passes the root depth.
fn counted(fen: &str, depth: u32) -> Counted {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(fen);
    let mut s = support::search(Limits::depth(depth), &stop, &tt);
    s.trace_calls();
    s.run(&mut b, &mut Vec::new());
    let mut out = Counted {
        rises: 0,
        capped: 0,
        past: Vec::new(),
    };
    let mut root = 0;
    let mut line: Vec<u32> = Vec::new();
    for call in s.calls() {
        if call.ply == 0 {
            assert_eq!(
                call.extensions, 0,
                "{fen}: the root at depth {}",
                call.depth
            );
            root = call.depth;
            line.clear();
            line.push(0);
            continue;
        }
        line.truncate(call.ply);
        assert_eq!(line.len(), call.ply, "{fen}: no parent recorded");
        let rise = call.extensions.checked_sub(line[call.ply - 1]);
        assert!(
            matches!(rise, Some(0 | 1)),
            "{fen}: {call:?} under {line:?}"
        );
        assert!(call.extensions <= root, "{fen}: {call:?} past {root}");
        out.rises += usize::from(rise == Some(1));
        out.capped += usize::from(call.extensions == root);
        if call.depth > 0 && call.ply + 1 > 2 * root as usize {
            out.past.push((call.ply, root));
        }
        line.push(call.extensions);
    }
    out
}

/// Through every rule that searches a child, the count rises only by the extension taken, and the
/// checking positions reach the cap.
#[test]
fn the_count_rises_by_the_extension_taken_and_stops_at_the_root_depth() {
    let mut rises = 0;
    let mut capped = 0;
    for fen in CHECKING {
        for depth in 1..=6 {
            let c = counted(fen, depth);
            rises += c.rises;
            capped += c.capped;
        }
    }
    for fen in support::standard_fens().iter().take(4) {
        rises += counted(fen, 6).rises;
    }
    assert!(rises > 1000, "only {rises} extensions counted");
    assert!(capped > 100, "only {capped} calls at the cap");
}

/// With the count capped at the root depth, an interior node needs a ply of depth, so none lies
/// past twice the root depth less one.
#[test]
fn no_interior_node_lies_past_twice_the_root_depth() {
    for fen in CHECKING {
        for depth in 1..=6 {
            let c = counted(fen, depth);
            assert!(
                c.past.is_empty(),
                "{fen}: depth {depth}, interior calls past the bound: {:?}",
                &c.past[..c.past.len().min(5)]
            );
        }
    }
}
