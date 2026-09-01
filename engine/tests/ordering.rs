// SPDX-License-Identifier: GPL-3.0-or-later

//! The interior nodes' move ordering: the transposition table's move
//! first, then the captures.
//!
//! The table has written a move at every node it stored since it landed,
//! and nothing read it. This is the first thing that does, and it is the
//! first ordering the main search has had at all: before it, `negamax`
//! took `generate_legal`'s order unchanged.
//!
//! **What a gate can see here, and what it cannot.** Ordering changes no
//! result. A search that tries the same moves in a different order returns
//! the same move for the same reason, only sooner or later, so there is no
//! position whose answer is wrong before the change and right after it.
//! What is observable is the node count and what the ordering *refuses*,
//! and those are what these gates pin.
//!
//! Three of them carry the weight.
//!
//! **A move the position does not have must never be played.** A hit's key
//! matched in full (`tt::verify`), so its move belongs to this position on
//! the same assumption about Zobrist collisions that the stored score
//! already rests on. The two do not cost the same when that assumption
//! fails. A wrong score is one node evaluated wrongly. A wrong move reaches
//! `Board::make_move`, which is not defensive and does not intend to be:
//! it panics on `make_move: no piece on the from square` and on `capture:
//! no victim`, in release as much as in debug, because both are `expect`
//! and not `debug_assert`. That is an engine that dies in the middle of a
//! rated game rather than one that plays a slightly worse move, so
//! `order_first` validates, and it validates for free: finding the move is
//! the operation, and a move with no index has nothing to rotate to the
//! front. The gate names both panics and shows that the search survives a
//! table that supplies either.
//!
//! **The rest of the list must keep the order it was generated in.** A
//! swap would be cheaper by a few moves of two bytes and would throw
//! whatever was at the head into the middle. Generation order is the only
//! order the remainder has today, and a later stable sort over it (the
//! capture ordering, next) inherits whatever this leaves behind, so it is
//! pinned here rather than left to be discovered as a bench number nobody
//! can account for.
//!
//! **The seam must actually be live.** A `order_first` that is correct and
//! never called passes every unit gate above. So a depth-two search is run
//! over a table poisoned at every key it can probe -- at depth two the
//! interior nodes are exactly the root's children, so "every key it can
//! probe" is a set this test can enumerate -- and the node count must move
//! when the poison is a legal move and must not move at all when it is
//! not.
//!
//! **The capture sort is in this file because it is the same ordering.**
//! `picker::sort_from` brings MVV-LVA to the main search's list, behind
//! whatever the table's move left at the head, and its gates come in the
//! same two kinds. The unit ones state the order as a rank -- a noisy
//! move's MVV-LVA key, and every quiet move below all of them -- and then
//! require descending rank, stability within a rank, and a move set that
//! did not change. The stage in front of it has its own: every legal move
//! is put at the head by `order_first` and has to still be there after the
//! sort, whatever it ranks.
//!
//! **What the sort saves can be attributed, which the table's move's
//! saving could not be.** Its end-to-end gate runs with a table of no
//! buckets, so every probe misses, `order_first` never fires, and the sort
//! is the only ordering the search has.
//!
//! **A sort must not move a score.** Alpha-beta returns the exact value of
//! the tree at the root whichever order the moves are tried in, so a score
//! that moves is a move dropped or searched twice rather than an ordering.
//! That gate runs against a table of no buckets as well, because a
//! transposition table may legitimately move a score: an entry stored by a
//! deeper search and read by a shallower one carries information the
//! shallower search would not otherwise have had, and which entries exist
//! depends on the order the moves were tried in.
//!
//! **The killers are the same ordering again, and gates of the same two
//! kinds.** `picker::sort_from` grows two ranks in the band between the
//! quiet moves and the noisy ones, and `search::remember_killer` decides
//! what goes in them. The unit gates state the bands and the slot order;
//! the seam gate is a node count, because a killer remembered and never
//! read, or read and never remembered, passes every unit gate above it.
//!
//! **Two of them carry weight beyond the change.** The slots are ordered by
//! their slot and not by the generator, which is the whole reason there are
//! two ranks rather than one shared rank behind a stable sort: with one,
//! the two would come out in generation order, which is right half the
//! time. And a killer the position does not hold must change nothing,
//! because a killer names a move that cut at a *sibling* and may be illegal
//! here. The comparison that finds it in the list is the entire check, and
//! that is what makes a pseudo-legality checker not a precondition of this
//! change: a checker is wanted by a picker that yields before it generates
//! and so has no list to check against, and this one has the list in hand.

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicBool;

use cadence_core::fen::FenStyle;
use cadence_core::position::Board;
use cadence_core::{Move, Square, generate_legal, generate_noisy};
use cadence_engine::picker::{noisy_key, sort_from, sort_noisy};
use cadence_engine::score::Score;
use cadence_engine::search::{Limits, Search, order_first, remember_killer};
use cadence_engine::see::see;
use cadence_engine::tt::{self, Bound, Table};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// One search of `board` to `depth` against `tt`: move, score, nodes.
fn search_with(board: &mut Board, depth: u32, tt: &Table) -> (Move, Score, u64) {
    let stop = AtomicBool::new(false);
    let mut sink = Vec::new();
    let mut s = Search::new(Limits::depth(depth), &stop, tt);
    let best = s.run(board, &mut sink);
    (best, s.score(), s.nodes())
}

/// The positions the deep gate runs over: **the same set as
/// `tests/tt.rs`**, deliberately, so that the node counts here and the ones
/// that test prints are comparable. Why it is this set and not the corpus
/// is written there.
fn deep_fens() -> Vec<String> {
    let mut out: Vec<String> = support::ENDGAME_FENS
        .iter()
        .map(|f| (*f).to_string())
        .collect();
    out.push(support::standard_fen("startpos"));
    out.push(support::standard_fen("pos3"));
    let arrays = support::dfrc_arrays();
    out.push(arrays.first().expect("a DFRC array").2.clone());
    out.push(arrays.last().expect("a DFRC array").2.clone());
    out
}

/// The depth the deep gate uses, as in `tests/tt.rs`.
const DEEP: u32 = 7;

/// A handful of positions to run the exhaustive bit-pattern gate over: all
/// four standard positions, both ends of the DFRC range so that the
/// king-takes-rook castling encoding is in it, and two endings.
fn dense_fens() -> Vec<String> {
    let mut out = support::standard_fens();
    let arrays = support::dfrc_arrays();
    out.push(arrays.first().expect("a DFRC array").2.clone());
    out.push(arrays.last().expect("a DFRC array").2.clone());
    out.push(support::ENDGAME_FENS[3].to_string());
    out.push(support::ENDGAME_FENS[6].to_string());
    out
}

// ---------------------------------------------------------------------------
// `order_first`, on its own
// ---------------------------------------------------------------------------

/// Every legal move of every corpus position, put at the front from
/// wherever it was generated.
#[test]
fn the_tables_move_goes_to_the_front() {
    let mut positions = 0;
    let mut moves = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        if legal.is_empty() {
            continue;
        }
        positions += 1;
        for m in legal.iter() {
            let mut list = legal.clone();
            assert!(
                order_first(&mut list, m),
                "{fen}: {m:?} is legal here and was not found"
            );
            assert_eq!(
                list.as_slice()[0],
                m,
                "{fen}: {m:?} was found and is not first"
            );
            assert_eq!(
                list.len(),
                legal.len(),
                "{fen}: the list changed length around {m:?}"
            );
            moves += 1;
        }
    }
    println!("{moves} moves over {positions} positions");
    assert!(positions > 30 && moves > 1000, "{positions}, {moves}");
}

/// The move goes to the front and the rest of the list is untouched:
/// generation order, with that one move taken out of it.
///
/// Stated as "the remainder equals the generated list without this move"
/// rather than as a rotation, so that the gate is not the implementation
/// written twice. The two agree only because `generate_legal` never emits
/// a move twice.
#[test]
fn the_rest_of_the_list_keeps_its_generated_order() {
    let mut checked = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        let generated: Vec<Move> = legal.iter().collect();
        for m in legal.iter() {
            let mut list = legal.clone();
            assert!(order_first(&mut list, m));
            let rest: Vec<Move> = list.as_slice()[1..].to_vec();
            let without: Vec<Move> = generated.iter().copied().filter(|&x| x != m).collect();
            assert_eq!(rest, without, "{fen}: the order behind {m:?} changed");
            checked += 1;
        }
    }
    assert!(checked > 1000, "{checked}");
}

/// Every one of the 65,536 things a sixteen-bit move field can hold, over
/// positions with dense move lists and both ends of the DFRC range.
///
/// A slot's move is sixteen bits and `Entry::decode` hands back whatever is
/// there: no bit pattern is reserved, so this is the whole space a
/// collision could produce. Exactly the legal ones are accepted; every
/// other pattern leaves the list byte for byte as it was.
#[test]
fn a_move_this_position_does_not_have_is_refused() {
    let mut refused = 0u64;
    let mut accepted = 0u64;
    for fen in dense_fens() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        let generated: Vec<Move> = legal.iter().collect();
        for bits in 0..=u16::MAX {
            let m = Move::from_bits(bits);
            let mut list = legal.clone();
            let found = order_first(&mut list, m);
            if generated.contains(&m) {
                assert!(found, "{fen}: {m:?} is legal and was refused");
                accepted += 1;
            } else {
                assert!(!found, "{fen}: {m:?} is not legal here and was accepted");
                assert_eq!(
                    list.as_slice(),
                    legal.as_slice(),
                    "{fen}: refusing {m:?} changed the list"
                );
                refused += 1;
            }
        }
    }
    println!("{accepted} accepted, {refused} refused");
    assert!(
        accepted > 200,
        "only {accepted} legal patterns were reached"
    );
    assert_eq!(refused + accepted, 65_536 * dense_fens().len() as u64);
}

/// `Move::NULL` is what a slot holds when nothing better was stored, and
/// it is `a1a1` quiet: a pattern no generator emits. It is refused
/// everywhere.
#[test]
fn the_null_move_is_refused_everywhere() {
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        let mut list = legal.clone();
        assert!(!order_first(&mut list, Move::NULL), "{fen}");
        assert_eq!(list.as_slice(), legal.as_slice(), "{fen}");
    }
}

// ---------------------------------------------------------------------------
// What refusing is worth: the two panics it stands in front of
// ---------------------------------------------------------------------------

/// A capture whose origin square is empty, and a capture whose target
/// square is: the two shapes of bogus move that `Board::make_move` does
/// not survive.
fn bogus_moves(b: &Board) -> Vec<(Move, &'static str)> {
    let us = b.side_to_move();
    let empty = Square::all().find(|&sq| b.piece_at(sq).is_none());
    let ours = Square::all().find(|&sq| b.piece_at(sq).is_some_and(|p| p.colour() == us));
    let mut out = Vec::new();
    if let (Some(e), Some(o)) = (empty, ours) {
        // From a square with nothing on it, and on to a square with
        // nothing on it: one panic each, and neither is a move any
        // generator can emit.
        out.push((
            Move::new_capture(e, o),
            "make_move: no piece on the from square",
        ));
        out.push((Move::new_capture(o, e), "capture: no victim"));
    }
    out
}

/// The reason `order_first` validates, executed rather than asserted in
/// prose: both bogus moves reach a panic in `core` if they are played, and
/// both are refused before they can be.
///
/// The panic is the *release* behaviour too: `make_move` reads the mover
/// and the victim out of the mailbox with `expect`, not with
/// `debug_assert`, so a table move nobody checked is a process that dies
/// mid-game.
#[test]
fn the_two_bogus_moves_that_would_kill_the_process() {
    let mut checked = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        for (m, expected) in bogus_moves(&b) {
            assert!(
                !legal.contains(m),
                "{fen}: {m:?} was meant to be impossible here"
            );
            let mut list = legal.clone();
            assert!(
                !order_first(&mut list, m),
                "{fen}: {m:?} was accepted into the ordering"
            );

            let mut victim = board(&fen);
            let err = catch_unwind(AssertUnwindSafe(|| victim.make_move(m)))
                .err()
                .unwrap_or_else(|| panic!("{fen}: {m:?} did not panic, so this gate is stale"));
            let msg = err
                .downcast_ref::<String>()
                .map_or_else(String::new, Clone::clone);
            assert!(
                msg.contains(expected),
                "{fen}: {m:?} panicked with {msg:?}, not with {expected:?}"
            );
            checked += 1;
        }
    }
    println!("{checked} bogus moves refused, each of which panics if it is played");
    assert!(checked > 60, "only {checked}");
}

// ---------------------------------------------------------------------------
// The seam: what the search does with a table's move
// ---------------------------------------------------------------------------

/// Store `pick(child)` under the key of every child of `board`, shallow
/// enough that no probe can ever cut on it.
///
/// At depth two the main search's interior nodes are exactly the root's
/// children -- the root does not probe, and their own children are the
/// horizon -- so this poisons every key a depth-two search can look up,
/// and `depth = 0` is below the depth one any of those probes needs, so
/// the entry is read for its move and for nothing else.
fn poison(b: &mut Board, tt: &Table, pick: impl Fn(&Board) -> Move) {
    for m in generate_legal(b).iter() {
        b.make_move(m);
        let mv = pick(b);
        tt.store(b.key(), mv, 0, 0, Bound::Upper);
        b.unmake_move(m);
    }
}

/// The last move `generate_legal` emits here: legal, and last, so that
/// putting it first is a change.
fn last_legal(b: &Board) -> Move {
    generate_legal(b).iter().last().unwrap_or(Move::NULL)
}

/// The positions the poison gates run over: the corpus, minus whatever
/// has no legal move.
fn poisonable() -> Vec<String> {
    support::corpus_fens()
        .into_iter()
        .filter(|f| !generate_legal(&board(f)).is_empty())
        .collect()
}

const POISON_DEPTH: u32 = 2;

/// The table's move is read at every interior node of a depth-two search.
///
/// The comparison is against the same table poisoned with `Move::NULL`:
/// same slots, same scores, same depths, same bounds, same replacement
/// pressure, and only the move field different. So a node count that moves
/// between the two moved because of the move field and nothing else.
///
/// This is the coverage gate for the whole change. An `order_first` that
/// is correct and is never called passes every gate above it and fails
/// this one.
#[test]
fn the_tables_move_is_read_at_every_interior_node() {
    let fens = poisonable();
    let mut moved = 0;
    for fen in &fens {
        let quiet = table();
        let mut b = board(fen);
        poison(&mut b, &quiet, |_| Move::NULL);
        let (_, _, null_nodes) = search_with(&mut b, POISON_DEPTH, &quiet);

        let loud = table();
        let mut b = board(fen);
        poison(&mut b, &loud, last_legal);
        let (_, _, loud_nodes) = search_with(&mut b, POISON_DEPTH, &loud);

        if loud_nodes != null_nodes {
            moved += 1;
        }
    }
    println!(
        "{moved} of {} positions changed their node count when the table named a move",
        fens.len()
    );
    assert!(
        moved * 2 > fens.len(),
        "the table's move changed the search in only {moved} of {} positions, \
         so nothing here is testing an ordering",
        fens.len()
    );
}

/// A move the table supplies and the position does not have changes
/// nothing at all: not the move, not the score, not one node.
///
/// The same null-move baseline as above, so the only difference between
/// the two runs is a move field holding something unplayable. Were it
/// played, `make_move` would panic and this test would not report a
/// mismatch, it would abort.
#[test]
fn a_move_the_table_cannot_supply_is_ignored_by_the_search() {
    let fens = poisonable();
    let mut checked = 0;
    for fen in &fens {
        let quiet = table();
        let mut b = board(fen);
        poison(&mut b, &quiet, |_| Move::NULL);
        let clean = search_with(&mut b, POISON_DEPTH, &quiet);

        for which in 0..2 {
            let poisoned = table();
            let mut b = board(fen);
            poison(&mut b, &poisoned, |child| {
                bogus_moves(child)
                    .get(which)
                    .map_or(Move::NULL, |&(m, _)| m)
            });
            let dirty = search_with(&mut b, POISON_DEPTH, &poisoned);
            assert_eq!(
                dirty, clean,
                "{fen}: a bogus table move changed the search (bogus shape {which})"
            );
            checked += 1;
        }
    }
    println!("{checked} poisoned searches, none of them different");
    assert!(checked > 60, "only {checked}");
}

/// End to end: the table's move has to be worth nodes.
///
/// The ratio is against a search with no table at all, so it mixes the
/// saving from the table's *score* with the saving from its move and from
/// the capture sort, and attributes none of them. That is what it is for:
/// it is the one gate that fails if the seam is not wired into `negamax`,
/// at a depth where the table is doing its real work rather than the two
/// ply the bench sees.
///
/// Measured on the M5 Max, 16 positions at depth 7, when this gate was
/// written: 17,292,004 nodes with no table, 5,009,163 with the table and
/// no ordering, 2,349,015 with the table's move. The capture sort has
/// since moved the two of those that this tree can still produce, to
/// 12,092,088 and 1,754,505, and the killers have moved them again, to
/// 3,559,436 and 1,034,950.
///
/// **The bound has now been narrowed by a change that was not about the
/// table, and the reason is worth more than the new number.** The ratio was
/// 6.89 with the capture sort and is 3.44 with the killers, because the
/// killers are worth a factor of 3.40 where there is no table and a factor
/// of 1.70 where there is one. Both stages put a quiet move that has
/// already worked at the front of the list, so what one of them finds, the
/// other does not have to: the two overlap, and the overlap is charged to
/// whichever arrives second. This gate is coverage and not a claim about a
/// size -- it is the one that fails if the seam is not wired into `negamax`
/// -- so the bound sits where it still fails for that reason rather than
/// where the ratio happens to be. The history heuristic should narrow it
/// again.
#[test]
fn the_tables_move_saves_nodes() {
    let fens = deep_fens();
    let (mut with, mut without) = (0u64, 0u64);
    for fen in &fens {
        let (_, _, w) = search_with(
            &mut board(fen),
            DEEP,
            &Table::new(tt::DEFAULT_HASH_MB).expect("a table"),
        );
        let (_, _, wo) = search_with(
            &mut board(fen),
            DEEP,
            &Table::with_buckets(0).expect("a table of no buckets"),
        );
        with += w;
        without += wo;
    }
    println!(
        "depth {DEEP}, {} positions: {without} nodes with no table, {with} with the table \
         and its move",
        fens.len()
    );
    assert!(
        with * 5 < 2 * without,
        "{with} against {without} is what the table was worth before its move was read"
    );
}

fn table() -> Table {
    Table::new(tt::DEFAULT_HASH_MB).expect("a default-sized transposition table")
}

// ---------------------------------------------------------------------------
// The capture sort, on its own
// ---------------------------------------------------------------------------

/// Two slots holding nothing: what every gate written before the killers
/// existed passes, and what the quiescence search's sort is handed.
const NO_KILLERS: [Move; 2] = [Move::NULL; 2];

/// This file's own numbers for the bands below the noisy one, chosen far
/// apart and far from the picker's so that the gates below say "below
/// every noisy move" rather than "the same constants the picker uses".
/// `noisy_key` spans -64 to 101, so bands a thousand apart cannot collide.
const KILLER_ONE: i32 = -1_000;
const KILLER_TWO: i32 = -1_001;
const LOSING_BASE: i32 = -10_000;
const QUIET_RANK: i32 = -100_000;

/// The rank the sort has to produce, written out here rather than taken
/// from the code under test. Five bands, and their order is the whole of
/// the specification: every noisy move whose exchange does not lose
/// material, by its MVV-LVA key; then the first killer; then the second;
/// then the noisy moves whose exchange does lose material, keeping their
/// MVV-LVA order among themselves; then every other quiet move.
///
/// **The losing band is what the third exchange-evaluation change moves.**
/// Before it every noisy move ranked above every killer, losing ones
/// included. `see` is the boundary, and it is asked here through the same
/// public function the picker reads, because the thing being gated is
/// where the answer puts the move and not what the answer is:
/// `tests/see.rs` is where the answer itself is held to an oracle.
fn rank_with(b: &Board, m: Move, killers: [Move; 2]) -> i32 {
    if m.is_noisy() {
        if see(b, m) < 0 {
            LOSING_BASE + noisy_key(b, m)
        } else {
            noisy_key(b, m)
        }
    } else if m == killers[0] {
        KILLER_ONE
    } else if m == killers[1] {
        KILLER_TWO
    } else {
        QUIET_RANK
    }
}

/// The noisy moves of `b` whose exchange loses material, and the ones whose
/// exchange does not, each in the order the generator emitted them.
fn losing_and_rest(b: &Board) -> (Vec<Move>, Vec<Move>) {
    let noisy: Vec<Move> = generate_legal(b).iter().filter(|m| m.is_noisy()).collect();
    let losing = noisy.iter().copied().filter(|&m| see(b, m) < 0).collect();
    let rest = noisy.iter().copied().filter(|&m| see(b, m) >= 0).collect();
    (losing, rest)
}

/// The corpus, and every position one legal move from a corpus position.
///
/// The corpus is 67 positions and holds only a handful with a capture that
/// loses material, which is too few to say a gate covered anything. Its
/// children are a few thousand and are reached deterministically, which is
/// what the gates below need: the same widening `tests/quiescence.rs` uses
/// for the in-check lists, for the same reason.
fn corpus_and_children() -> Vec<String> {
    let mut out = Vec::new();
    for fen in support::corpus_fens() {
        let mut b = board(&fen);
        out.push(fen.clone());
        for m in generate_legal(&b).iter() {
            b.make_move(m);
            out.push(b.to_fen(FenStyle::Shredder));
            b.unmake_move(m);
        }
    }
    out
}

/// The positions that can tell the losing band from the one above it: a
/// noisy move that loses material, a noisy move that does not, and three
/// quiet moves so that a killer is not the only quiet move there is.
fn losing_capture_fens() -> Vec<String> {
    corpus_and_children()
        .into_iter()
        .filter(|fen| {
            let b = board(fen);
            let (losing, rest) = losing_and_rest(&b);
            !losing.is_empty() && !rest.is_empty() && quiets(&b).len() >= 3
        })
        .collect()
}

/// The rank with no killers: every quiet move alike, below every noisy one.
///
/// `Move::NULL` is `a1a1` quiet, which no generator emits, so neither
/// killer branch above can fire and this is the rank the file had before
/// the killers existed.
fn rank(b: &Board, m: Move) -> i32 {
    rank_with(b, m, NO_KILLERS)
}

/// The quiet moves of `b`, in the order the generator emitted them.
fn quiets(b: &Board) -> Vec<Move> {
    generate_legal(b).iter().filter(|m| !m.is_noisy()).collect()
}

/// The legal moves of `fen`, generated and then sorted from `start` with
/// `killers` in the slots.
fn sorted_with(fen: &str, start: usize, killers: [Move; 2]) -> (Board, Vec<Move>, Vec<Move>) {
    let b = board(fen);
    let generated: Vec<Move> = generate_legal(&b).iter().collect();
    let mut list = generate_legal(&b);
    sort_from(&b, &mut list, start, killers, &[]);
    let after: Vec<Move> = list.iter().collect();
    (b, generated, after)
}

/// The legal moves of `fen`, generated and then sorted from `start`.
fn sorted(fen: &str, start: usize) -> (Board, Vec<Move>, Vec<Move>) {
    sorted_with(fen, start, NO_KILLERS)
}

/// Nothing is lost and nothing is invented: the sorted list holds exactly
/// the moves the generator emitted.
#[test]
fn the_sort_is_a_permutation_of_the_list_it_was_given() {
    let mut checked = 0;
    for fen in support::corpus_fens() {
        let (_, generated, after) = sorted(&fen, 0);
        let mut before: Vec<u16> = generated.iter().map(|m| m.to_bits()).collect();
        let mut sorted_after: Vec<u16> = after.iter().map(|m| m.to_bits()).collect();
        before.sort_unstable();
        sorted_after.sort_unstable();
        assert_eq!(before, sorted_after, "{fen}: the sort changed the move set");
        checked += generated.len();
    }
    println!(
        "{checked} moves over {} positions",
        support::corpus_fens().len()
    );
    assert!(checked > 1000, "{checked}");
}

/// Descending rank, everywhere in the list.
#[test]
fn the_list_comes_out_in_descending_rank_order() {
    let mut positions = 0;
    for fen in support::corpus_fens() {
        let (b, generated, after) = sorted(&fen, 0);
        if generated.len() < 2 {
            continue;
        }
        for w in after.windows(2) {
            assert!(
                rank(&b, w[0]) >= rank(&b, w[1]),
                "{fen}: {:?} ranks {} and comes before {:?}, which ranks {}",
                w[0],
                rank(&b, w[0]),
                w[1],
                rank(&b, w[1])
            );
        }
        positions += 1;
    }
    assert!(positions > 30, "only {positions} positions had two moves");
}

/// The property that buys the nodes, named on its own: every capture and
/// every promotion is tried before any quiet move.
///
/// Counted from the generated list, so a sort that dropped a noisy move
/// could not satisfy it by having fewer of them at the front.
#[test]
fn every_noisy_move_is_tried_before_every_quiet_one() {
    let mut mixed = 0;
    for fen in support::corpus_fens() {
        let (_, generated, after) = sorted(&fen, 0);
        let noisy = generated.iter().filter(|m| m.is_noisy()).count();
        let quiet = generated.len() - noisy;
        assert!(
            after[..noisy].iter().all(|m| m.is_noisy()),
            "{fen}: a quiet move is inside the first {noisy}"
        );
        assert!(
            after[noisy..].iter().all(|m| !m.is_noisy()),
            "{fen}: a noisy move is behind the first {noisy}"
        );
        if noisy > 0 && quiet > 0 {
            mixed += 1;
        }
    }
    println!("{mixed} positions had both a noisy move and a quiet one");
    assert!(
        mixed > 10,
        "only {mixed} positions could tell the two apart"
    );
}

/// Ties keep generation order. Stated as subsequence equality per rank,
/// which covers the quiet moves -- they all share one rank -- and the
/// captures that share a victim and an attacker.
#[test]
fn moves_of_equal_rank_keep_the_order_they_were_generated_in() {
    let mut ranks = 0;
    for fen in support::corpus_fens() {
        let (b, generated, after) = sorted(&fen, 0);
        let mut seen: Vec<i32> = generated.iter().map(|&m| rank(&b, m)).collect();
        seen.sort_unstable();
        seen.dedup();
        for r in seen {
            let want: Vec<Move> = generated
                .iter()
                .copied()
                .filter(|&m| rank(&b, m) == r)
                .collect();
            let got: Vec<Move> = after
                .iter()
                .copied()
                .filter(|&m| rank(&b, m) == r)
                .collect();
            assert_eq!(got, want, "{fen}: rank {r} was reordered within itself");
            ranks += 1;
        }
    }
    assert!(ranks > 60, "only {ranks} rank classes");
}

/// The moves in front of `start` are not touched, and everything behind it
/// is sorted as if the list began there.
#[test]
fn the_sort_leaves_the_head_of_the_list_alone() {
    let mut checked = 0;
    for fen in dense_fens() {
        let b = board(&fen);
        let generated: Vec<Move> = generate_legal(&b).iter().collect();
        for start in 0..=generated.len() {
            let mut list = generate_legal(&b);
            sort_from(&b, &mut list, start, NO_KILLERS, &[]);
            let after: Vec<Move> = list.iter().collect();
            assert_eq!(
                after[..start],
                generated[..start],
                "{fen}: sorting from {start} moved something in front of it"
            );
            for w in after[start..].windows(2) {
                assert!(
                    rank(&b, w[0]) >= rank(&b, w[1]),
                    "{fen}: sorting from {start} left the tail out of order"
                );
            }
            checked += 1;
        }
    }
    println!("{checked} (position, start) pairs");
    assert!(checked > 100, "{checked}");
}

/// The two stages meet here: the table's move first, then the sort behind
/// it.
///
/// Every legal move is put at the head by `order_first` and the list is
/// then sorted from one, which is what the search does. The move stays at
/// the head whatever it ranks -- a quiet move in front of a queen capture
/// is the whole point of the stage order -- and the tail comes out in
/// descending rank.
#[test]
fn the_tables_move_stays_in_front_of_the_sort() {
    let mut quiet_in_front = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        for m in legal.iter() {
            let mut list = legal.clone();
            assert!(order_first(&mut list, m), "{fen}: {m:?} is legal here");
            sort_from(&b, &mut list, 1, NO_KILLERS, &[]);
            let after: Vec<Move> = list.iter().collect();
            assert_eq!(after[0], m, "{fen}: the sort moved the table's move");
            for w in after[1..].windows(2) {
                assert!(
                    rank(&b, w[0]) >= rank(&b, w[1]),
                    "{fen}: the tail behind {m:?} is out of order"
                );
            }
            if !m.is_noisy() && after[1..].iter().any(|m| m.is_noisy()) {
                quiet_in_front += 1;
            }
        }
    }
    println!("{quiet_in_front} quiet table moves kept ahead of a noisy one");
    assert!(
        quiet_in_front > 150,
        "only {quiet_in_front} cases where the stage order was observable"
    );
}

// ---------------------------------------------------------------------------
// The losing captures, on their own
// ---------------------------------------------------------------------------
//
// Every capture used to be tried before the first killer, losing ones
// included, because every noisy rank sat above the killer band. The ones
// whose exchange loses material now sit in a band of their own below both
// killers and above every other quiet move.
//
// Three things a gate can see, and they are separate claims. Where the band
// sits, which is the stage order. What decides membership, which is the
// exchange and not whether the move captures. And that the group moves
// without being reordered inside itself, which is what makes this one
// change rather than two.

/// A losing capture is tried after both killers and ahead of every quiet
/// move that is not one. The stage order, stated as indices into the
/// sorted list.
#[test]
fn a_losing_capture_is_tried_after_both_killers_and_ahead_of_every_other_quiet() {
    let fens = losing_capture_fens();
    for fen in &fens {
        let b = board(fen);
        let killers = late_killers(&b);
        let (losing, rest) = losing_and_rest(&b);
        let (_, _, after) = sorted_with(fen, 0, killers);
        // [ the noisy moves that do not lose ][ killer 0 ][ killer 1 ]
        // [ the noisy moves that do lose ][ every other quiet move ]
        let n = rest.len();
        assert!(
            after[..n].iter().all(|m| m.is_noisy() && see(&b, *m) >= 0),
            "{fen}: the first {n} are not the noisy moves that keep material"
        );
        assert_eq!(after[n], killers[0], "{fen}: the first killer is misplaced");
        assert_eq!(
            after[n + 1],
            killers[1],
            "{fen}: the second killer is misplaced"
        );
        let band = &after[n + 2..n + 2 + losing.len()];
        assert!(
            band.iter().all(|m| m.is_noisy() && see(&b, *m) < 0),
            "{fen}: the band behind the killers is not the losing captures"
        );
        assert!(
            after[n + 2 + losing.len()..].iter().all(|m| !m.is_noisy()),
            "{fen}: a noisy move sits behind the quiet block"
        );
    }
    println!(
        "{} positions with a losing capture, a keeping one and three quiet moves",
        fens.len()
    );
    assert!(fens.len() > 50, "only {} positions", fens.len());
}

/// What decides the band is the exchange, not whether the move captures and
/// not what it captures.
///
/// The gate that separates this change from "rank the cheap victims last".
/// Every position here has a losing capture and a keeping one; where their
/// MVV-LVA keys are in the *other* order -- the losing capture takes the
/// more valuable piece -- a rule that read the victim would put them the
/// wrong way round, and the count of those cases is the coverage.
#[test]
fn the_band_is_decided_by_the_exchange_and_not_by_the_victim() {
    let mut inverted = 0;
    for fen in losing_capture_fens() {
        let b = board(&fen);
        let (losing, rest) = losing_and_rest(&b);
        let (_, _, after) = sorted_with(&fen, 0, NO_KILLERS);
        let pos = |m: Move| after.iter().position(|&x| x == m).expect("in the list");
        for &l in &losing {
            for &r in &rest {
                assert!(
                    pos(r) < pos(l),
                    "{fen}: {} loses material and is tried before {}",
                    l.to_uci_chess960(),
                    r.to_uci_chess960()
                );
                if noisy_key(&b, l) > noisy_key(&b, r) {
                    inverted += 1;
                }
            }
        }
    }
    println!("{inverted} pairs where the losing capture has the better victim");
    assert!(
        inverted > 90,
        "only {inverted} pairs could tell the exchange from the victim"
    );
}

/// The group moves and nothing inside it does: the losing captures come out
/// in the same relative order they had before, which is MVV-LVA.
///
/// This is what makes the change one change. A flat band would also put
/// them behind the killers and would additionally throw away the order they
/// already had among themselves, which is a second claim with a second
/// number.
#[test]
fn the_losing_captures_keep_their_order_among_themselves() {
    let mut checked = 0;
    for fen in losing_capture_fens() {
        let b = board(&fen);
        let (_, _, after) = sorted_with(&fen, 0, NO_KILLERS);
        let got: Vec<Move> = after
            .into_iter()
            .filter(|&m| m.is_noisy() && see(&b, m) < 0)
            .collect();
        let mut want = got.clone();
        want.sort_by_key(|&m| -noisy_key(&b, m));
        assert_eq!(
            got, want,
            "{fen}: the losing captures are not in MVV-LVA order among themselves"
        );
        if got.len() > 1 {
            checked += 1;
        }
    }
    println!("{checked} positions with two losing captures or more");
    assert!(checked > 40, "only {checked} positions could tell");
}

/// The quiescence search's out-of-check list is sorted by the victim alone,
/// with the losing captures left among the rest.
///
/// `sort_noisy` is the one sort that does not demote, and this states it
/// rather than leaving it to be discovered: the list comes out in
/// descending `noisy_key`, which is what it was before this change.
#[test]
fn the_quiescence_searchs_noisy_sort_does_not_demote() {
    let mut mixed = 0;
    for fen in losing_capture_fens() {
        let b = board(&fen);
        if b.in_check() {
            continue;
        }
        let mut list = generate_noisy(&b);
        sort_noisy(&b, &mut list);
        let after: Vec<Move> = list.iter().collect();
        for w in after.windows(2) {
            assert!(
                noisy_key(&b, w[0]) >= noisy_key(&b, w[1]),
                "{fen}: the noisy sort is not in victim order"
            );
        }
        if after.iter().any(|&m| see(&b, m) < 0) && after.iter().any(|&m| see(&b, m) >= 0) {
            mixed += 1;
        }
    }
    println!("{mixed} out-of-check positions with both kinds of noisy move");
    assert!(mixed > 50, "only {mixed} positions could tell");
}

/// And demoting there would move no node, which is why it is not done.
///
/// `quiesce` refuses every noisy move whose exchange loses material before
/// searching it, so the moves it actually searches are the ones `see`
/// keeps. Take both sorts, drop the moves the search would refuse, and the
/// two sequences are equal: the searched moves come out in the same order
/// either way. That is the whole argument for `sort_noisy` not paying for a
/// `see` call per move, executed rather than asserted in prose.
#[test]
fn demoting_the_moves_the_quiescence_search_refuses_would_reorder_nothing() {
    let mut compared = 0;
    for fen in corpus_and_children() {
        let b = board(&fen);
        if b.in_check() {
            continue;
        }
        let mut plain = generate_noisy(&b);
        sort_noisy(&b, &mut plain);
        let mut demoted = generate_noisy(&b);
        sort_from(&b, &mut demoted, 0, NO_KILLERS, &[]);
        let searched = |l: &cadence_core::MoveList| -> Vec<Move> {
            l.iter().filter(|&m| see(&b, m) >= 0).collect()
        };
        assert_eq!(
            searched(&plain),
            searched(&demoted),
            "{fen}: the moves the quiescence search would try come out in a different order"
        );
        if plain.iter().any(|m| see(&b, m) < 0) {
            compared += 1;
        }
    }
    println!("{compared} out-of-check positions with a move the search would refuse");
    assert!(compared > 50, "only {compared} positions could tell");
}

// ---------------------------------------------------------------------------
// The seam: what the search does with the sort
// ---------------------------------------------------------------------------

/// The depth the two end-to-end gates below run at.
const SORT_DEPTH: u32 = 7;

/// End to end, with the table switched off: the sort is worth nodes on its
/// own.
///
/// A table of no buckets misses every probe, so `order_first` never fires
/// and the sort is the only ordering the main search has. What it saves is
/// therefore attributable to it and to nothing else, which
/// `the_tables_move_saves_nodes` above deliberately is not.
///
/// This is the coverage gate for the change. A sort that is correct and is
/// never called passes every gate above it and fails this one.
///
/// **Re-measured when the check extension landed, and the reference moved
/// by a factor of 23.** The same 16 positions at depth 7 with neither the
/// table nor the sort read 17,292,004 nodes before anything extended and
/// **396,887,401** after, against 29,775,675 for the search that ships.
/// That is what an extension costs a badly ordered search: the ordered
/// tree grew 11 times and the unordered one 23, because a bad first move
/// at a node that has been handed a ply back is a bad first move over a
/// subtree that no longer shrinks. Both points are measured on the
/// extending build, so the ratio between them is still attributable to the
/// sort alone.
///
/// **Re-measured again when the null window landed, and this time the
/// unordered point moved four and a half times further than the sorted
/// one**: 89,173,515 nodes with no ordering in the main search against
/// 17,187,706 with it, where the same two points were 396,887,401 and
/// 29,775,675 before. A null window refutes a move over a smaller tree, and
/// an unordered node is nothing but moves waiting to be refuted, so the
/// change is worth most exactly where the ordering is worst. The gate is
/// unaffected either way -- it discriminates by a factor of five -- but the
/// old reference would have left it passing with the sort removed, which is
/// the one thing it exists to fail.
///
/// **Re-measured again when null-move pruning landed, for the same
/// reason**: 72,422,385 nodes with no ordering in the main search against
/// 14,565,111 with it. The pruning cut both trees, the ceiling of nine
/// tenths of 89,173,515 had fallen above the new unordered point, and the
/// build this gate exists to fail had started passing it. The shape of the
/// assertion is unchanged and the gate discriminates by a factor of five
/// again.
///
/// **And again when late move reductions landed**: 23,395,608 nodes with
/// no ordering in the main search against 5,495,043 with it, and the old
/// ceiling had once more fallen above the unordered point. Every pruning
/// or reduction change cuts the counterfactual as fast as the shipped
/// tree, so this re-measurement is due at each of them, not once. The
/// gate discriminates by a factor of about four.
///
/// **And when the history heuristic landed, where for the first time the
/// old ceiling would still have failed the build it exists to fail**:
/// 26,826,075 nodes with no ordering in the main search against 2,514,122
/// with it. The counterfactual **grew** by 15% while the shipped tree more
/// than halved, which is the opposite of the four re-measurements above
/// and is what an ordering change does rather than what a pruning change
/// does: it widens the gap it is measured across instead of cutting both
/// sides of it. Re-based anyway, because the constant is meant to be a
/// number this tree produces. The gate now discriminates by a factor of
/// about eleven, the widest it has been since the check extension.
///
/// **And futility pruning survived it too, which is a pruning change and
/// so was not supposed to.** 25,881,212 nodes with no ordering in the main
/// search against 2,291,752 with it: the counterfactual fell 3.5% where the
/// shipped tree fell 8.8%, so the gap widened and the old ceiling of nine
/// tenths of 26,826,075 still sits below the counterfactual. **The
/// discriminating property is not whether the change prunes, it is whether
/// the counterfactual takes the change's own trigger away.** This rule
/// fires where alpha stands a margin above the static evaluation, and alpha
/// stands there because the ordering put a good move first; an unordered
/// search raises alpha slowly and hands the rule far less to skip. So a
/// build with no ordering loses most of the pruning as well as the
/// ordering, and the two losses compound in the counterfactual's favour.
/// Re-based anyway, on the same ground as last time.
///
/// **And reverse futility survived it too, for a third reason that is
/// neither of the two above.** 25,835,504 nodes with no ordering in the
/// main search against 2,313,475 with it. Neither side moved much: the
/// counterfactual fell 0.18% and the shipped tree **grew** 0.95%, so the
/// ratio went from 11.29 to 11.17 and the gate never came near its
/// ceiling. The reason is not that the counterfactual kept the change's
/// trigger, which is what the paragraph above established for futility
/// pruning. It is that **the change is worth nothing on this set**: these
/// sixteen positions are twelve endgames, and measured on the two halves
/// separately the rule costs 0.98% on the endgames (2,227,553 to
/// 2,249,395) and saves 0.19% on the other four, against the 34.6% it
/// takes off the bench positions. A ceiling gate can outlive a change
/// because the counterfactual keeps its trigger, or because the change
/// cannot reach the gate's positions, and only the first says anything
/// about the ordering. Re-based on the same ground as before.
///
/// **Late move pruning killed it, after four items it survived, and the
/// mechanism is a third one again.** 8,511,242 nodes with no ordering in
/// the main search against 1,311,497 with it. The counterfactual fell 67%
/// where the shipped tree fell 43%, so the ratio went from 11.17 to 6.49
/// and the old ceiling of nine tenths of 25,835,504 sat far above the
/// unordered point: the build this gate exists to fail had started passing
/// it. **What is new is that the counterfactual does not merely keep the
/// change's trigger, it over-fires it.** This rule gives a move up on its
/// rank alone, and in a build with no ranking every rank is arbitrary, so
/// it deletes as many moves from a tree where the deletions are worthless
/// as from one where they are not. Futility's counterfactual kept a trigger
/// it could not raise alpha to reach; this one keeps a trigger that needs
/// no evidence at all. The gate is re-based and it discriminates by a
/// factor of about six.
#[test]
fn the_capture_sort_saves_nodes() {
    let fens = deep_fens();
    let mut total = 0u64;
    for fen in &fens {
        let (_, _, n) = search_with(
            &mut board(fen),
            SORT_DEPTH,
            &Table::with_buckets(0).expect("a table of no buckets"),
        );
        total += n;
    }
    println!(
        "depth {SORT_DEPTH}, {} positions, no table: {total} nodes",
        fens.len()
    );
    assert!(
        total * 10 < 9 * 8_511_242,
        "{total} nodes against the 8,511,242 the same search took with no ordering at all"
    );
}

/// End to end: demoting the losing captures has to be worth nodes.
///
/// The same 16 positions at depth 7 with no table, so the sort is the only
/// ordering the main search has and what moves is attributable to it. A
/// demotion that is correct in the picker and never reaches a search passes
/// every gate above this one and fails this.
///
/// Measured on the M5 Max when this gate was written: 3,234,886 nodes with
/// every capture ahead of the killers, 2,654,840 with the losing ones
/// behind them, a ceiling of 3,000,000 between the two.
///
/// **Re-measured when the check extension landed, and the window it
/// discriminates by narrowed from 18% to 3.2%**: 30,749,287 nodes with
/// every capture ahead of the killers against 29,775,675 with the losing
/// ones behind them, and the ceiling is 30,250,000, about 1.6% either side.
/// It is still coverage rather than a claim about the effect's size, and
/// the narrowing is the claim it is now closer to making: a demotion the
/// extension has made worth less on this set is a different statement from
/// a demotion that is not wired in, and this gate can no longer tell them
/// apart by much. Both points are measured on the extending build. Node
/// counts are exact and not timings, so 1.6% is a margin and not a band.
///
/// **Re-measured again when the null window landed, and the window is now
/// 1.3%**: 17,418,454 nodes with every capture ahead of the killers against
/// 17,187,706 with the losing ones behind them, and the ceiling is
/// 17,300,000, about 0.65% either side. The old ceiling would have passed
/// the build with the demotion switched off, so re-measuring was not
/// optional. **What this gate is close to, said before it arrives:** the
/// margin has gone 18%, 3.2%, 1.3% over three changes, none of which
/// touched the demotion, and the next change to the tree can be expected to
/// halve it again. At that point it stops being a gate against a demotion
/// that never reaches a search and becomes a gate against nothing, and the
/// answer will be a set or a depth where the band is worth more rather than
/// a tighter ceiling on this one.
///
/// **That point arrived with null-move pruning, further than predicted**:
/// 14,582,345 nodes with every capture ahead of the killers against
/// 14,565,111 with the losing ones behind them, a window of 0.12%, and the
/// old ceiling sat far above both. The counts are exact, so a ceiling
/// between the two new points still separates the builds today, and it is
/// re-baselined once more on that ground alone. What the paragraph above
/// asked for is now due rather than approaching: the pruning has cut away
/// most of the subtrees the demotion was saving on this set at this depth,
/// and the next tree change should replace this gate's set or depth
/// instead of its constant.
///
/// **The depth replacement was tried when late move reductions landed,
/// and it runs the wrong way.** The pair on that tree reads 5,506,625
/// against 5,495,043 at depth 7 (a window of 0.21%), 30,279,091 against
/// 30,253,806 at depth 8 (0.084%), and 164,009,563 against 163,899,386
/// at depth 9 (0.067%): deeper is narrower, because the pruning and the
/// reductions remove the late-move subtrees the demotion was saving in
/// proportion to how many there are. So a depth change cannot restore the
/// window and the ceiling is re-based between the new exact points once
/// more. What remains open is a position set chosen for losing captures
/// that compete with killers, and that is a design task with its own
/// measurement, not a constant: until it exists this gate separates the
/// builds by 0.21% of exact counts and no more.
///
/// **The history heuristic re-based it again and did not narrow it**:
/// 2,527,884 nodes with every capture ahead of the killers against
/// 2,514,122 with the losing ones behind them, a window of 0.55%, which is
/// wider than the 0.21% the reductions left. Both points more than halved
/// and the ratio between them barely moved, so what the ordering did here
/// was shrink the tree rather than take the demotion's work, which the
/// killer gate below cannot say about itself. The open item is unchanged
/// and is still a position set rather than a constant.
///
/// **Futility pruning re-based it once more and narrowed it again**:
/// 2,301,352 nodes with every capture ahead of the killers against
/// 2,291,752 with the losing ones behind them, a window of 0.42% against
/// the 0.55% the history heuristic left. The old ceiling sat above both.
/// The open item is still the position set and has now outlived four
/// re-baselines, which is worth saying plainly: this gate has been
/// separating exact counts by under one per cent since null-move pruning
/// landed, and every item since has re-based a constant instead of
/// building the set that would restore the window.
///
/// **Reverse futility re-based it a fifth time and narrowed it again**:
/// 2,319,727 nodes with every capture ahead of the killers against
/// 2,313,475 with the losing ones behind them, a window of 0.27% against
/// the 0.42% futility pruning left. Both points **rose**, which no change
/// has done to this gate before, and the reason is the set rather than the
/// demotion: twelve of these sixteen positions are endgames, where a
/// static evaluation of material and piece-square tables is at its least
/// informative and the margin rule above costs nodes instead of saving
/// them. The open item is unchanged and is still the position set.
///
/// **Late move pruning re-based it a sixth time and widened it, from
/// 0.27% to 0.40%**: 1,316,775 nodes with every capture ahead of the
/// killers against 1,311,497 with the losing ones behind them. Both points
/// nearly halved and the window grew, which the history heuristic is the
/// only other change to have done here. The reason is this rule's index:
/// a losing capture ahead of the killers pushes every quiet move one place
/// further down the list, and one place further down is nearer the count
/// this rule gives up at, so promoting the losing captures now costs
/// searched quiet moves as well as order. That is the demotion being read
/// by something new rather than the gate's set improving, and the open
/// item is unchanged and is still the position set.
#[test]
fn demoting_the_losing_captures_saves_nodes() {
    let fens = deep_fens();
    let mut total = 0u64;
    for fen in &fens {
        let (_, _, n) = search_with(
            &mut board(fen),
            SORT_DEPTH,
            &Table::with_buckets(0).expect("a table of no buckets"),
        );
        total += n;
    }
    println!(
        "depth {SORT_DEPTH}, {} positions, no table, losing captures demoted: {total} nodes",
        fens.len()
    );
    assert!(
        total < 1_314_000,
        "{total} nodes against the 1,316,775 the same search took with every capture ahead of the killers"
    );
}

// The sort changes no score: **retired 2026-09-01, and it is the second
// property here that late move pruning ended rather than moved.**
//
// It asserted that these sixteen positions score the same at a fixed
// depth on the shipped build and on a build with no sort in `negamax`, on
// the ground that alpha-beta returns the exact value of the tree whichever
// order the moves are tried in, so a moved score is a dropped or
// duplicated move rather than a differently ordered search.
//
// **Late move reductions ended the premise and the gate retreated to a
// depth below their threshold; this rule acts from depth one and there is
// nowhere left to retreat to.** Both read the index the sort assigned a
// move, so the sorted and unsorted builds search different trees by
// construction: with no sort the index is generation order, and a rule
// keyed on the index gives up different moves. Measured here at depth two,
// where the count is four: four of the sixteen scores move, and the two
// largest by 89 and 6 centipawns. Depth one is vacuous, because
// `search_root` does its own ordering and `negamax`'s sort is never
// called.
//
// **A moved score is now the expected answer and not a defect**, which is
// what makes this a retirement rather than a re-baseline: re-measuring the
// array on the unsorted build would produce a number that agrees by
// construction with nothing, and a gate whose counterfactual is a
// different search is not a gate.
//
// What still covers the claim it was making: the permutation gates above
// pin that the sort drops and duplicates no move, which is the defect this
// one was reaching for through the score, and the counterfactual ceilings
// pin that the ordering is worth nodes at depth. What is lost is the
// end-to-end reading, and the honest statement is that no gate here now
// asserts the search's value is order-independent, because on this tree it
// is not.

// ---------------------------------------------------------------------------
// The killers, on their own
// ---------------------------------------------------------------------------

/// The positions that can tell the killer band from the quiet one: three
/// quiet moves is the fewest that leaves a quiet move outside both slots.
fn killer_fens() -> Vec<String> {
    support::corpus_fens()
        .into_iter()
        .filter(|fen| quiets(&board(fen)).len() >= 3)
        .collect()
}

/// The two slots, taken from the end of the generated order.
///
/// From the end and not the start on purpose: the first quiet move
/// generated is where an unsorted list already puts it, so slots filled
/// from the front would be satisfied by a sort that ignored them.
fn late_killers(b: &Board) -> [Move; 2] {
    let q = quiets(b);
    [q[q.len() - 1], q[q.len() - 2]]
}

/// A killer is tried after every noisy move that keeps material and ahead
/// of every other quiet one. The stage order, stated as two indices.
///
/// **Narrowed by the losing band.** This said "after every noisy move"
/// until the third exchange-evaluation change, and it was the gate that
/// change had to break: a noisy move whose exchange loses material is now
/// tried *after* both killers, so the index the killers sit at is the count
/// of the noisy moves that keep material and not of all of them. What the
/// killers are still ahead of is every quiet move that is not a killer, and
/// that half is unchanged.
#[test]
fn a_killer_is_tried_after_every_keeping_noisy_move_and_ahead_of_every_other_quiet() {
    // The corpus and its children rather than the corpus: five corpus
    // positions have a capture that loses material, which is too few to
    // say the narrowing below was exercised.
    let fens: Vec<String> = corpus_and_children()
        .into_iter()
        .filter(|fen| quiets(&board(fen)).len() >= 3)
        .collect();
    let mut narrowed = 0;
    for fen in &fens {
        let b = board(fen);
        let killers = late_killers(&b);
        let (losing, rest) = losing_and_rest(&b);
        let (_, _, after) = sorted_with(fen, 0, killers);
        let noisy = rest.len();
        assert!(
            after[..noisy].iter().all(|m| m.is_noisy()),
            "{fen}: a killer was ordered in among the noisy moves"
        );
        assert_eq!(
            after[noisy], killers[0],
            "{fen}: the first killer does not head the quiet moves"
        );
        assert_eq!(
            after[noisy + 1],
            killers[1],
            "{fen}: the second killer does not follow the first"
        );
        if !losing.is_empty() {
            narrowed += 1;
        }
    }
    println!(
        "{} positions with three quiet moves or more, {narrowed} of them with a losing capture",
        fens.len()
    );
    assert!(fens.len() > 30, "{}", fens.len());
    assert!(
        narrowed > 5,
        "only {narrowed} positions exercise the narrowing"
    );
}

/// The slots are ordered by their slot and not by the generator.
///
/// This is the gate that separates two ranks from one. The killers here are
/// always the reverse of generation order, so a single shared rank behind a
/// stable sort fails on every position rather than on half of them.
#[test]
fn the_first_killer_is_tried_before_the_second_whatever_their_generation_order() {
    let fens = killer_fens();
    for fen in &fens {
        let b = board(fen);
        let q = quiets(&b);
        let killers = [q[q.len() - 1], q[0]];
        let (_, _, after) = sorted_with(fen, 0, killers);
        let first = after
            .iter()
            .position(|&m| m == killers[0])
            .expect("the first killer is a legal move here");
        let second = after
            .iter()
            .position(|&m| m == killers[1])
            .expect("the second killer is a legal move here");
        assert!(
            first < second,
            "{fen}: the generator emitted {:?} first and the sort kept it there",
            killers[1]
        );
    }
    assert!(fens.len() > 30, "{}", fens.len());
}

/// Descending rank everywhere, with the killer bands in the scale.
///
/// The band statement in full: the noisy moves are still ordered among
/// themselves, and the two killers sit between them and the rest.
#[test]
fn the_list_comes_out_in_descending_rank_order_with_killers() {
    let fens = killer_fens();
    for fen in &fens {
        let b = board(fen);
        let killers = late_killers(&b);
        let (_, _, after) = sorted_with(fen, 0, killers);
        for w in after.windows(2) {
            assert!(
                rank_with(&b, w[0], killers) >= rank_with(&b, w[1], killers),
                "{fen}: {:?} ranks {} and comes before {:?}, which ranks {}",
                w[0],
                rank_with(&b, w[0], killers),
                w[1],
                rank_with(&b, w[1], killers)
            );
        }
    }
    assert!(fens.len() > 30, "{}", fens.len());
}

/// A killer the position does not have changes nothing.
///
/// A killer named a move that cut at a sibling of this node, in a different
/// position, and nothing has checked that it is legal here. Nothing needs
/// to: the comparison that finds it in the list is the check, and a move
/// the list does not hold matches nothing and ranks nobody. That is what
/// makes a pseudo-legality checker not a precondition of this change, and
/// it is the same argument `order_first` already makes for the table's
/// move.
#[test]
fn a_killer_the_position_does_not_have_changes_nothing() {
    let fens = support::corpus_fens();
    let mut checked = 0;
    for (i, fen) in fens.iter().enumerate() {
        let here: Vec<Move> = generate_legal(&board(fen)).iter().collect();
        let donor = board(&fens[(i + 1) % fens.len()]);
        let foreign: Vec<Move> = quiets(&donor)
            .into_iter()
            .filter(|m| !here.contains(m))
            .collect();
        if foreign.len() < 2 {
            continue;
        }
        let (_, _, with) = sorted_with(fen, 0, [foreign[0], foreign[1]]);
        let (_, _, without) = sorted(fen, 0);
        assert_eq!(
            with, without,
            "{fen}: a killer this position does not have moved something"
        );
        checked += 1;
    }
    println!("{checked} positions given two killers of another position");
    assert!(checked > 30, "{checked}");
}

/// A noisy move handed in as a killer keeps its noisy rank.
///
/// Nothing in the signature says a slot holds a quiet move, and what
/// enforces it is the order of the branches in `picker::move_key`. Ranked
/// as a killer, a capture would sort below every other capture, which is an
/// inversion the search would pay for at exactly the nodes whose cutoff
/// move was noisy.
#[test]
fn a_noisy_move_named_as_a_killer_keeps_its_noisy_rank() {
    let mut checked = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let noisy: Vec<Move> = generate_legal(&b).iter().filter(|m| m.is_noisy()).collect();
        if noisy.is_empty() {
            continue;
        }
        let (_, _, with) = sorted_with(&fen, 0, [noisy[noisy.len() - 1], Move::NULL]);
        let (_, _, without) = sorted(&fen, 0);
        assert_eq!(
            with,
            without,
            "{fen}: {:?} was ranked as a killer rather than as a capture",
            noisy[noisy.len() - 1]
        );
        checked += 1;
    }
    println!("{checked} positions with a noisy move to offer");
    assert!(checked > 10, "{checked}");
}

/// Nothing is lost and nothing is invented once the slots are in the scale.
#[test]
fn the_sort_with_killers_is_still_a_permutation() {
    let fens = killer_fens();
    for fen in &fens {
        let b = board(fen);
        let killers = late_killers(&b);
        let (_, generated, after) = sorted_with(fen, 0, killers);
        let mut before: Vec<u16> = generated.iter().map(|m| m.to_bits()).collect();
        let mut sorted_after: Vec<u16> = after.iter().map(|m| m.to_bits()).collect();
        before.sort_unstable();
        sorted_after.sort_unstable();
        assert_eq!(before, sorted_after, "{fen}: the sort changed the move set");
    }
    assert!(fens.len() > 30, "{}", fens.len());
}

/// The three stages meet: the table's move, then the killers, then the
/// rest.
///
/// Two cases and both matter. When the table's move is not a killer, the
/// killer heads the quiet moves behind it. When the table's move *is* the
/// killer, it stays at the head and is not ranked at all -- it sits in
/// front of `start`, where the killer band never sees it -- and no second
/// copy of it appears.
#[test]
fn the_tables_move_stays_in_front_of_a_killer() {
    let (mut apart, mut same) = (0, 0);
    for fen in killer_fens() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        let q = quiets(&b);
        for (tt_move, killer) in [(q[0], q[q.len() - 1]), (q[0], q[0])] {
            let mut list = legal.clone();
            assert!(
                order_first(&mut list, tt_move),
                "{fen}: {tt_move:?} is legal here"
            );
            sort_from(&b, &mut list, 1, [killer, Move::NULL], &[]);
            let after: Vec<Move> = list.iter().collect();
            assert_eq!(after[0], tt_move, "{fen}: the sort moved the table's move");
            let mut bits: Vec<u16> = after.iter().map(|m| m.to_bits()).collect();
            bits.sort_unstable();
            bits.dedup();
            assert_eq!(bits.len(), after.len(), "{fen}: a move was duplicated");
            if tt_move == killer {
                same += 1;
            } else {
                // The keeping noisy moves, not every noisy move: a losing
                // capture is behind both killers now, so it is part of the
                // tail this index has to skip past rather than of the head.
                let keeping = after[1..]
                    .iter()
                    .filter(|m| m.is_noisy() && see(&b, **m) >= 0)
                    .count();
                assert_eq!(
                    after[1 + keeping],
                    killer,
                    "{fen}: the killer does not head the quiet moves behind the table's move"
                );
                apart += 1;
            }
        }
    }
    println!(
        "{apart} positions with the killer behind the table's move, {same} where they are one move"
    );
    assert!(apart > 30 && same > 30, "{apart} and {same}");
}

// ---------------------------------------------------------------------------
// `remember_killer`, on its own
// ---------------------------------------------------------------------------

/// A position with three quiet moves and a capture, for the slot gates.
fn slots_fixture() -> (Vec<Move>, Vec<Move>) {
    let b = board(&support::standard_fen("kiwipete"));
    let quiet = quiets(&b);
    let noisy: Vec<Move> = generate_legal(&b).iter().filter(|m| m.is_noisy()).collect();
    assert!(
        quiet.len() >= 3 && !noisy.is_empty(),
        "the fixture is not one"
    );
    (quiet, noisy)
}

/// Only a quiet move is remembered, whether the slots are empty or full.
#[test]
fn a_noisy_move_is_never_remembered_as_a_killer() {
    let mut checked = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        for m in generate_legal(&b).iter().filter(|m| m.is_noisy()) {
            let mut slots = NO_KILLERS;
            remember_killer(&mut slots, m);
            assert_eq!(
                slots, NO_KILLERS,
                "{fen}: {m:?} is noisy and took an empty slot"
            );
            checked += 1;
        }
    }
    let (quiet, noisy) = slots_fixture();
    let full = [quiet[0], quiet[1]];
    for m in &noisy {
        let mut slots = full;
        remember_killer(&mut slots, *m);
        assert_eq!(slots, full, "{m:?} is noisy and displaced a killer");
    }
    println!(
        "{checked} noisy moves offered to empty slots, {} to full ones",
        noisy.len()
    );
    assert!(checked > 40, "{checked}");
}

/// A new killer shifts slot zero into slot one, and a move already in slot
/// one is promoted rather than duplicated.
#[test]
fn a_new_killer_shifts_the_first_slot_into_the_second() {
    let (quiet, _) = slots_fixture();
    let (a, b, c) = (quiet[0], quiet[1], quiet[2]);

    let mut slots = NO_KILLERS;
    remember_killer(&mut slots, a);
    assert_eq!(slots, [a, Move::NULL], "the first killer takes slot zero");
    remember_killer(&mut slots, b);
    assert_eq!(slots, [b, a], "the second shifts the first back");
    remember_killer(&mut slots, c);
    assert_eq!(slots, [c, b], "the third displaces the oldest");
    remember_killer(&mut slots, b);
    assert_eq!(
        slots,
        [b, c],
        "a move in slot one is promoted, not duplicated"
    );
}

/// Remembering the move already in slot zero changes nothing. The shift
/// would fill both slots with one move and leave the stage one move wide.
#[test]
fn remembering_the_first_slot_again_leaves_the_second_alone() {
    let (quiet, _) = slots_fixture();
    let (a, b) = (quiet[0], quiet[1]);
    let mut slots = [a, b];
    remember_killer(&mut slots, a);
    assert_eq!(slots, [a, b], "slot one was overwritten with slot zero");
}

// ---------------------------------------------------------------------------
// The seam: what the search does with the killers
// ---------------------------------------------------------------------------

// **End to end: the ceiling that said the killers are worth nodes is
// retired, and what retired it is a measurement rather than a failure.**
//
// This was `the_killers_save_nodes`: the same sixteen positions, the same
// depth and the same table as `the_tables_move_saves_nodes`, asserting the
// shipped total below a count measured on a build with `remember_killer`
// emptied. It was re-based eight times and its window ran 18%, 3.2%,
// 1.3%, 0.12%, 23%, 1.4%, 3.2% and 0.146%, and its own comment said to
// read a failure here as the set having run out rather than as the killers
// having stopped paying.
//
// **The measurement was taken on the champion as well, which is what
// says this is not the change that landed beside it.** Eighteen
// configurations -- these positions, the standard suite and twelve bench
// positions, at depths seven, eight and nine, with and without a table --
// were run against the killerless build on the tree that retired this and
// on the tree before it. The window has no stable sign in either: it runs
// `-10.4%` to `+10.9%` on the champion and `-18.3%` to `+7.5%` here, and
// it changes sign inside every one of the three sets as the depth or the
// table moves. In this gate's own configuration the champion's window was
// 244 nodes out of 167,673, which is what it had been passing on.
//
// **What that retires is the ceiling and not the coverage.** A ceiling
// needs the counterfactual on one side of the shipped count, and the
// quantity here does not stay on a side. What the killers do is still
// gated exactly, above: they take the ranks the picker gives them, they
// sit ahead of every other quiet move and behind the losing captures, a
// new one shifts the slots, and none of them crosses a search boundary.
// What is lost is the end-to-end claim that they are worth nodes, and no
// test in this file can carry it: the claim compares two builds and a
// test runs one.
//
// **And none of this is a reading about Elo.** The killers measured
// `+56.48` by SPRT, and they are the row both of this project's
// node-based summaries already fail on. A change that costs nodes on a
// set and gains strength in games is what that row has always said, and
// this measurement is more of it rather than a contradiction of it.
//
// **What would restore a ceiling is the position set, which is what this
// gate has been asking for since null-move pruning.** It is not written
// here, because a set chosen from eighteen readings by taking the one
// where the sign came out right is the instrument being fitted, and that
// is the failure this whole family of gates keeps finding.

/// The depth `a_reused_search_remembers_no_killers` searches to.
const REUSE_DEPTH: u32 = 5;

/// The killers do not cross a search boundary.
///
/// `run` clears them, so a `Search` that has already searched something is
/// not a different engine from a fresh one. Nothing in the tree reaches
/// this today: `bench` builds a fresh `Search` per position and the UCI
/// layer builds one per `go`. It is pinned because the determinism contract
/// is that the node count is a function of the code and of
/// the table the search is handed, and killers carried across a seam would
/// make it a function of whatever ran before as well, which is the failure
/// clearing the table between bench positions exists to prevent.
///
/// Against a table of no buckets, so the table cannot carry anything either
/// and the killers are the only state that could.
#[test]
fn a_reused_search_remembers_no_killers() {
    let fens = deep_fens();
    let stop = AtomicBool::new(false);
    let mut sink = Vec::new();
    let mut pairs = 0;
    for pair in fens.windows(2) {
        let tt = Table::with_buckets(0).expect("a table of no buckets");
        let mut reused = Search::new(Limits::depth(REUSE_DEPTH), &stop, &tt);
        reused.run(&mut board(&pair[0]), &mut sink);
        sink.clear();
        let again = reused.run(&mut board(&pair[1]), &mut sink);
        let after = reused.nodes();

        let fresh_tt = Table::with_buckets(0).expect("a table of no buckets");
        let mut fresh = Search::new(Limits::depth(REUSE_DEPTH), &stop, &fresh_tt);
        sink.clear();
        let alone = fresh.run(&mut board(&pair[1]), &mut sink);
        assert_eq!(
            (again, after),
            (alone, fresh.nodes()),
            "{}: the search after another one is not the search on its own",
            pair[1]
        );
        pairs += 1;
    }
    assert!(pairs >= 15, "{pairs}");
}
