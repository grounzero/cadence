// SPDX-License-Identifier: GPL-3.0-or-later

//! The cache is exact by construction, so a seam that only moved code would pass every gate here;
//! each was shown failing on a deliberate break instead. A debug build also checks every cached
//! evaluation against the walk, which makes the searches below gates of every node.

mod support;

use std::collections::BTreeMap;
#[cfg(debug_assertions)]
use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Colour, PieceType, generate_legal};
use cadence_engine::bench;
use cadence_engine::eval::{PAWN_CACHE_ENTRIES, PawnCache, evaluate, evaluate_cached};
#[cfg(debug_assertions)]
use cadence_engine::search::Limits;
#[cfg(debug_assertions)]
use support::table;

/// A rook, a queen and a bishop pair with no pawn on the board.
const PAWNLESS: [&str; 3] = [
    "4k3/8/8/8/8/8/8/R3K3 w - - 0 1",
    "8/8/3k4/8/8/2Q5/8/4K3 b - - 0 1",
    "4k3/8/8/8/8/8/8/2B1KB2 w - - 0 1",
];

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

fn pawns(board: &Board) -> (u64, u64) {
    (
        board.pieces(Colour::White, PieceType::Pawn).0,
        board.pieces(Colour::Black, PieceType::Pawn).0,
    )
}

/// The corpus, the bench positions and the pawnless set, each followed by its children, so a
/// king move meets its parent's pawn structure in the cache.
fn positions() -> Vec<Board> {
    let mut fens = support::corpus_fens();
    fens.extend(bench::positions().into_iter().map(str::to_string));
    fens.extend(PAWNLESS.iter().map(|f| (*f).to_string()));
    let mut out = Vec::new();
    for fen in &fens {
        let parent = board(fen);
        let moves = generate_legal(&parent);
        out.push(parent);
        for m in moves.iter() {
            let mut child = board(fen);
            child.make_move(m);
            out.push(child);
        }
    }
    out
}

/// One cache across every position, so each lookup meets whatever the positions before it left.
/// Each is read twice, and the second reading is a hit.
#[test]
fn cached_and_walked_evaluations_agree_at_every_position() {
    let boards = positions();
    let mut cache = PawnCache::new();
    let mut structures: BTreeMap<usize, Vec<(u64, u64)>> = BTreeMap::new();
    for b in &boards {
        for _ in 0..2 {
            assert_eq!(
                evaluate_cached(b, &mut cache),
                evaluate(b),
                "{}",
                b.to_fen(cadence_core::FenStyle::Shredder)
            );
        }
        let at = structures
            .entry(b.pawn_key() as usize % PAWN_CACHE_ENTRIES)
            .or_default();
        if !at.contains(&pawns(b)) {
            at.push(pawns(b));
        }
    }
    let pawnless = boards.iter().filter(|b| pawns(b) == (0, 0)).count();
    let shared = structures.values().filter(|s| s.len() > 1).count();
    assert!(pawnless >= PAWNLESS.len(), "{pawnless} pawnless positions");
    assert!(shared > 0, "no two structures share an index");
    let reads = 2 * boards.len() as u64;
    assert_eq!(cache.probes(), reads);
    assert!(cache.hits() >= reads / 2, "{} hits", cache.hits());
}

/// Two real structures at one index, read in turn: each lookup misses and recomputes, so the
/// earlier structure's sums never answer for the later one.
#[test]
fn two_structures_at_one_index_each_miss_the_other() {
    let boards = positions();
    let mut first: BTreeMap<usize, usize> = BTreeMap::new();
    let pair = boards.iter().enumerate().find_map(|(i, b)| {
        let index = b.pawn_key() as usize % PAWN_CACHE_ENTRIES;
        match first.get(&index) {
            Some(&j) if pawns(&boards[j]) != pawns(b) => Some((j, i)),
            Some(_) => None,
            None => {
                first.insert(index, i);
                None
            }
        }
    });
    let (a, b) = pair.expect("two structures sharing an index");
    let mut cache = PawnCache::new();
    for board in [&boards[a], &boards[b], &boards[a], &boards[b]] {
        assert_eq!(
            evaluate_cached(board, &mut cache),
            evaluate(board),
            "{}",
            board.to_fen(cadence_core::FenStyle::Shredder)
        );
    }
    assert_eq!((cache.probes(), cache.hits()), (4, 0));
}

/// Over a million evaluations across the bench positions, in about a second of a test build.
#[cfg(debug_assertions)]
const DEPTH: u32 = 8;

/// A fresh `Search` per bench position, as the bench builds one, and then one `Search` across them
/// all, as a datagen game keeps one across its moves.
#[cfg(debug_assertions)]
#[test]
fn every_evaluation_of_a_search_agrees_with_the_walk() {
    let stop = AtomicBool::new(false);
    let fens = bench::positions();
    let (mut probes, mut hits) = (0, 0);
    for fen in &fens {
        let tt = table();
        let mut s = support::search(Limits::depth(DEPTH), &stop, &tt);
        s.run(&mut support::position(fen), &mut Vec::new());
        assert!(s.pawn_cache_hits() > 0, "{fen}: no hits");
        (probes, hits) = (probes + s.pawn_cache_probes(), hits + s.pawn_cache_hits());
    }
    // A pawnless position never misses: its empty entry is its value.
    assert!(hits < probes, "{hits} hits of {probes} probes");
    let tt = table();
    let mut kept = support::search(Limits::depth(DEPTH), &stop, &tt);
    for fen in &fens {
        kept.run(&mut support::position(fen), &mut Vec::new());
    }
}
