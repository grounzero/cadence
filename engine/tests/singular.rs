// SPDX-License-Identifier: GPL-3.0-or-later

//! Singular extensions: where the table names a move and a search of every
//! other move falls short of it by a margin, that move is searched a ply
//! deeper.
//!
//! **What these gates demonstrate is that the extension is bounded and that
//! the question it rests on is asked honestly.** Those are different claims
//! and the second is the one this rule needs and the check extension's does
//! not. A check is a fact about the position, read off the board in one
//! call; singularity is the verdict of a search, and a search that was
//! answered by the entry it is testing, or that searched the move it was
//! told to leave out, would return the same verdict on nothing.
//!
//! So the exclusion is gated as three separate properties of a node --
//! **the move is not searched, the table is not believed, and the table is
//! not written** -- through [`Search::node_excluding`], one at a time, and
//! not through the rule that asks for one. A gate driven through the rule
//! tests whichever exclusions this tree's table happens to produce.
//!
//! **The bound is gated as arithmetic, because it is the only thing between
//! this rule and a line that never ends.** Where every node's table move is
//! singular the depth never falls, exactly as it never falls in a line of
//! checks, and [`extension`]'s ply cap is what both rules stand on. One ply
//! whatever grants it, nothing past the cap, and the two reasons do not
//! add.
//!
//! **And the counters are the wiring.** [`Search::singular_extensions`] is
//! incremented where the child's depth is computed and only where the value
//! computed there was one, so a counter that has moved is the extension
//! having reached the depth a move was searched at, which no assertion about
//! [`extension`] alone can say.
//!
//! The counters are written wherever the rule runs and read on no decision
//! path, so a depth-limited search here reads no clock and the assertions
//! are exact.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, generate_legal};
use cadence_engine::score::{INFINITE, MATE_IN_MAX_PLY, Score, mate_in};
use cadence_engine::search::{
    Limits, Search, extension, singular_candidate, singular_depth, singular_margin,
};
use cadence_engine::tt::{Bound, Table};
use support::table;

/// The depth the set gate below searches to.
///
/// **Eleven, and it is measured rather than chosen**, the way the margin's
/// `GATE_DEPTH` and late move pruning's are. This rule needs three things at
/// one node -- a depth of at least seven left, an entry in the table within
/// three plies of that depth, and a move in it -- and the second is what
/// takes the depth up: at a node seven plies from the horizon the entry has
/// to have been written by an earlier iteration, so nothing fires until the
/// search has run enough iterations to have written one. Over the four
/// positions below the first verification search runs at depth nine and the
/// first extension is granted at ten.
///
/// Eleven is taken, one ply past the first depth at which both counters
/// move, because a coverage assertion standing exactly on the first depth
/// that works is one the next tree change empties in silence.
const GATE_DEPTH: u32 = 11;

/// A quiet middlegame, the same position the reduction, margin and
/// late-move-pruning gates use, so the rules are asked about one tree.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// White is three queens down and has one thing on the board: a rook that
/// reaches a8 along an empty file, where it mates. Late move pruning's gates
/// stand on it for the check exemption; here it is the position that makes
/// an exclusion visible without reading a node count, because taking `Ra8`
/// away turns a mate into a position white is losing.
const QUIET_MATE: &str = "6k1/5ppp/8/8/8/8/1qqq4/R5K1 w - - 0 1";

/// How many times the root depth an extension is granted within.
///
/// Stated here rather than imported from the search, for the reason
/// `tests/search.rs` states beside its own copy: a gate that reads the
/// constant it is checking cannot see that constant change, and changing the
/// cap changes which lines are searched deeper.
const EXTEND_WITHIN: usize = 2;

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The move `uci` names in `fen`, which the gate reads off the generator
/// rather than spelling as bits.
fn move_named(fen: &str, uci: &str) -> Move {
    generate_legal(&board(fen))
        .iter()
        .find(|m| m.to_uci_chess960() == uci)
        .unwrap_or_else(|| panic!("{fen} has no move {uci}"))
}

// ---------------------------------------------------------------------------
// The bound, as arithmetic
// ---------------------------------------------------------------------------

/// One ply whatever grants it, and the two reasons do not add.
///
/// A move that gives check and was found singular is extended by one, not
/// two. That is the whole of what keeps `search.rs`'s stated bound on the
/// deepest interior node an iteration can reach true with a second extension
/// in the tree, and a formulation that summed the two would pass every other
/// gate here.
#[test]
fn the_extension_is_one_ply_whatever_grants_it() {
    let root_depth = 8;
    let ply = 3;
    assert_eq!(extension(false, false, ply, root_depth), 0);
    assert_eq!(extension(true, false, ply, root_depth), 1);
    assert_eq!(extension(false, true, ply, root_depth), 1);
    assert_eq!(
        extension(true, true, ply, root_depth),
        1,
        "a checking singular move was extended twice"
    );
}

/// Nothing is extended past the cap, for either reason.
///
/// The check extension's own gate walks every case either side of the
/// boundary and this walks the same cases for the other reason, because the
/// cap is the bound on a line of singular extensions in exactly the sense it
/// is the bound on a line of checks: where the reason keeps arriving the
/// depth never falls.
#[test]
fn the_ply_cap_refuses_a_singular_extension() {
    for root_depth in [0u32, 1, 2, 7, 20] {
        let cap = EXTEND_WITHIN * root_depth as usize;
        for ply in 0..cap + 4 {
            assert_eq!(
                extension(false, true, ply, root_depth),
                u32::from(ply < cap),
                "root depth {root_depth}, ply {ply}: the cap is {cap}"
            );
        }
    }
}

/// The verification search is always shallower than the node that asks for
/// it, and never below one ply of real search.
///
/// The first half is what stops the question costing more than the answer
/// can be worth; the second is what stops it being handed to the quiescence
/// search, which would answer a question about a move ordering with a
/// standing pat.
#[test]
fn the_verification_search_is_shallower_than_the_node() {
    for depth in 7..=64u32 {
        let d = singular_depth(depth);
        assert!(d < depth, "depth {depth}: the verification search is {d}");
        assert!(d >= 3, "depth {depth}: the verification search is {d}");
    }
}

/// The margin is the depth and nothing else: it grows with the depth,
/// strictly, from zero at zero.
#[test]
fn the_margin_grows_with_the_depth() {
    assert_eq!(singular_margin(0), 0);
    for depth in 1..=64u32 {
        assert!(
            singular_margin(depth) > singular_margin(depth - 1),
            "depth {depth}: the margin did not grow"
        );
    }
}

// ---------------------------------------------------------------------------
// What the entry has to say before it is read
// ---------------------------------------------------------------------------

/// Each refusal decides on its own, with everything else held admitting.
///
/// One function asked twice with one field moved, the way the count's own
/// gate is made: a rule that refused on the wrong field, or that refused on
/// none of them, answers the same on both sides of every pair below and
/// fails here.
#[test]
fn each_refusal_decides_on_its_own() {
    let depth = 9;
    let entry = u8::try_from(depth).expect("a depth inside a byte");
    assert!(
        singular_candidate(depth, entry, Bound::Lower, 40),
        "the admitting case does not admit"
    );

    assert!(
        !singular_candidate(6, entry, Bound::Lower, 40),
        "a node below the threshold asked"
    );
    assert!(
        singular_candidate(7, entry, Bound::Lower, 40),
        "a node at the threshold did not ask"
    );

    assert!(
        !singular_candidate(depth, entry - 4, Bound::Lower, 40),
        "an entry four plies shallow was read"
    );
    assert!(
        singular_candidate(depth, entry - 3, Bound::Lower, 40),
        "an entry three plies shallow was refused"
    );

    assert!(
        !singular_candidate(depth, entry, Bound::Upper, 40),
        "an upper bound was read as evidence of a floor"
    );
    assert!(
        singular_candidate(depth, entry, Bound::Exact, 40),
        "an exact score was refused"
    );

    assert!(
        !singular_candidate(depth, entry, Bound::Lower, mate_in(9)),
        "a mate score was compared against a centipawn margin"
    );
    assert!(
        !singular_candidate(depth, entry, Bound::Lower, -mate_in(9)),
        "a mate score was compared against a centipawn margin"
    );
}

// ---------------------------------------------------------------------------
// The three properties of an excluded node
// ---------------------------------------------------------------------------

/// One node at `depth`, with `excluded` taken away from it, on a table of
/// its own. The score and the table are what the callers read.
fn excluding(fen: &str, depth: u32, excluded: Move, tt: &Table) -> Score {
    let stop = AtomicBool::new(false);
    let mut b = board(fen);
    let mut s = Search::new(Limits::depth(depth), &stop, tt);
    s.node_excluding(&mut b, depth, 0, -INFINITE, INFINITE, excluded)
}

/// The excluded move is not searched, and a mate is how the gate sees it.
///
/// `Ra8` mates in [`QUIET_MATE`] and every other move loses to three
/// queens, so a search that keeps the move returns a mate score for the side
/// to move and a search that has genuinely dropped it cannot. No node count
/// is read: a rule that searched the move at some reduced depth, or searched
/// it and discarded its score, would still find the mate.
#[test]
fn an_excluded_move_is_not_searched() {
    let mate = move_named(QUIET_MATE, "a1a8");
    let kept = {
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut b = board(QUIET_MATE);
        let mut s = Search::new(Limits::depth(2), &stop, &tt);
        s.node(&mut b, 2, 0)
    };
    assert!(
        kept >= MATE_IN_MAX_PLY,
        "the position does not mate at all: {kept}"
    );
    let without = excluding(QUIET_MATE, 2, mate, &table());
    assert!(
        without < MATE_IN_MAX_PLY,
        "the excluded move was searched: {without}"
    );
}

/// An excluded node writes nothing to the table.
///
/// What it establishes is a bound on a different question -- this position
/// less one move -- and a later probe of this key would read it as a bound
/// on the position. Nothing distinguishes the two once it is stored, so the
/// gate is that the key is absent afterwards rather than that its contents
/// are right.
#[test]
fn an_excluded_node_stores_nothing() {
    let tt = table();
    let key = board(MIDDLEGAME).key();
    let first = generate_legal(&board(MIDDLEGAME))
        .iter()
        .next()
        .expect("the position has a move");
    let _ = excluding(MIDDLEGAME, 4, first, &tt);
    assert!(
        tt.probe(key).is_none(),
        "the excluded node stored a bound on a position it did not search"
    );
}

/// An excluded node does not take the table's cutoff.
///
/// The entry it would take is the entry naming the move it was told to leave
/// out, so a node that believed it would answer the question with the move
/// the question is about. The stored score is one no evaluation of this
/// position produces, so the gate reads the score and needs no counter.
#[test]
fn an_excluded_node_refuses_the_tables_cutoff() {
    const IMPOSSIBLE: Score = 12_345;
    let tt = table();
    let b = board(MIDDLEGAME);
    let first = generate_legal(&b).iter().next().expect("a move");
    tt.store(
        b.key(),
        first,
        cadence_engine::score::to_tt(IMPOSSIBLE, 0),
        64,
        Bound::Exact,
    );
    let score = excluding(MIDDLEGAME, 4, first, &tt);
    assert_ne!(
        score, IMPOSSIBLE,
        "the excluded node was answered by the entry naming the move it excluded"
    );
}

/// A node whose only move is excluded has nothing left to search, and what
/// it returns says so.
///
/// This is the degenerate end of the rule and it is the case that decides
/// correctly by construction: no other move can reach the margin because
/// there is no other move. The value never leaves the verification search --
/// an excluded node stores nothing and only [`Search::singular`] reads one --
/// so what matters is that it is below every margin rather than what it is.
#[test]
fn a_node_whose_only_move_is_excluded_falls_short_of_everything() {
    // White is in check from the queen on d2 and taking it is the only
    // legal reply.
    const ONE_MOVE: &str = "4k3/8/8/8/8/8/3q4/4K3 w - - 0 1";
    let legal = generate_legal(&board(ONE_MOVE));
    assert_eq!(legal.len(), 1, "the position is meant to have one move");
    let only = legal.iter().next().expect("the one move");
    let score = excluding(ONE_MOVE, 4, only, &table());
    assert_eq!(
        score, -INFINITE,
        "a node with nothing to search returned a value"
    );
}

/// The row the verification search borrows is put back.
///
/// It is indexed by ply and the verification search runs at the ply it was
/// asked at, so the rule writes into the row the asking node is using. A
/// search that failed to clear it would leave a move unsearchable at a node
/// with no rule running, which nothing else here would notice.
#[test]
fn the_excluded_row_is_put_back() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(MIDDLEGAME);
    let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
    let _ = s.run(&mut b, &mut std::io::sink());
    for ply in 0..64 {
        assert_eq!(
            s.excluded_at(ply),
            Move::NULL,
            "ply {ply} still has a move excluded after the search"
        );
    }
}

// ---------------------------------------------------------------------------
// End to end: the rule decides, and both ways
// ---------------------------------------------------------------------------

/// The positions the set gate below searches. Four, chosen for having a
/// table worth reading: two middlegames where the ordering is informative
/// and two endgames where it is not.
fn gate_fens() -> [&'static str; 4] {
    [
        MIDDLEGAME,
        "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
        "4rrk1/pp1n1pp1/2pbp2p/8/3P4/2NBPN2/PP3PPP/2R2RK1 w - - 0 1",
    ]
}

/// The rule fires, extends, and does not extend everything it asks about.
///
/// Three assertions and the third is the one worth having: a rule that
/// extended every move it tested would pass a gate reading only the first
/// two, and would be an unconditional extension of the table's move wearing
/// this rule's name. The counters are exact, so the gate is an inequality
/// between two counts and not a threshold.
#[test]
fn the_rule_asks_and_answers_both_ways() {
    let stop = AtomicBool::new(false);
    let mut tests = 0;
    let mut extensions = 0;
    for fen in gate_fens() {
        let tt = table();
        let mut b = board(fen);
        let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
        let _ = s.run(&mut b, &mut std::io::sink());
        tests += s.singular_tests();
        extensions += s.singular_extensions();
    }
    assert!(
        tests > 0,
        "no node asked whether its table move was singular"
    );
    assert!(extensions > 0, "no move was ever found singular");
    assert!(
        extensions < tests,
        "every move asked about was found singular: {extensions} of {tests}"
    );
}

/// The verification search is a search, and what it costs is a node count.
///
/// The rule's price is not a term in any other counter here: an extension
/// enlarges the tree below the move it extends, and this is the other half,
/// the tree spent deciding. A gate that saw only the extensions would report
/// a rule that had spent a fifth of the search on questions as working
/// perfectly.
#[test]
fn the_verification_searches_are_counted() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(MIDDLEGAME);
    let mut s = Search::new(Limits::depth(GATE_DEPTH), &stop, &tt);
    let _ = s.run(&mut b, &mut std::io::sink());
    assert!(s.singular_tests() > 0, "no verification search ran");
    assert!(
        s.singular_nodes() >= s.singular_tests(),
        "{} verification searches cost {} nodes",
        s.singular_tests(),
        s.singular_nodes()
    );
    assert!(
        s.singular_nodes() < s.nodes(),
        "the verification searches cost the whole search"
    );
}
