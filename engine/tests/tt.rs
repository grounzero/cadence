// SPDX-License-Identifier: GPL-3.0-or-later

//! The transposition table: the slot codec, the replacement scheme, the
//! `Hash` option, and what the table does to a search.
//!
//! A table has no oracle either. It cannot be perft'd, and every wrong
//! version of it plays legal chess: a bound compared the wrong way round,
//! a mate score stored at the root's scale, a slot returned without
//! checking the key, all produce a search that finishes, reports a move
//! and is worth less than it should be. So the gates here pin the things
//! that define the structure and are observable without an opponent.
//!
//! Three of them are the load-bearing ones.
//!
//! **A torn read must fail validation.** The two words of a slot cannot be
//! read as a pair, so the scheme is `key ^ data` beside `data` and the
//! reader checks. That is testable without a race: the words of two
//! different writes, combined, are what a race would produce.
//!
//! **The move must not change when the search is repeated.** This is a
//! public search invariant, and the transposition table is the
//! change most likely to break it, because it is the first thing that
//! makes the same position return a different answer depending on what was
//! searched before it. A gate for it is only worth having if it can fail,
//! so it carries its own coverage assertion: the repeated search must
//! actually be cheaper than the first, or the table was not warm and the
//! stability being observed is nothing.
//!
//! **`bench` must clear between positions.** Otherwise the total depends
//! on the order the positions are run in and the determinism contract is
//! quietly gone. The gate compares every position's
//! count against a standalone search of it and, so that the comparison is
//! not vacuous, checks that an uncleared run really would differ.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::types::PromoPiece;
use cadence_core::{Move, Square, generate_legal};
use cadence_engine::score::{self, MAX_EVAL, Score, mate_in, mated_in};
use cadence_engine::search::{Limits, Search};
use cadence_engine::tt::{self, Bound, Entry, Table};
use cadence_engine::{bench, uci::Session};
use support::Rng;

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

fn table(mb: usize) -> Table {
    Table::new(mb).unwrap_or_else(|| panic!("a {mb} MB table"))
}

/// A table with no buckets: the search with no transposition table at all,
/// which is the baseline every "what does the table change" gate needs.
fn no_table() -> Table {
    Table::with_buckets(0).expect("a table of no buckets")
}

/// The positions the search gates run over.
///
/// **Not the corpus**, and the reason is the depth. A table needs a main
/// search with interior nodes for two paths to meet in: at depth three
/// there are two ply of them and nothing transposes, and the measured
/// saving over the whole bench is 45 nodes in 29 million. So these gates
/// run at [`GATE_DEPTH`], and at that depth Kiwipete is 2.3 billion nodes
/// and not a test. These are the endgame seeds, where few pieces make
/// transpositions dense and trees small, with the start position, the
/// third standard position, and the first and last DFRC arrays so that
/// the castling half of the Zobrist key is in the gate too.
fn gate_fens() -> Vec<String> {
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

/// The depth the search gates use. See [`gate_fens`] for why it is not
/// three, and why the positions are the ones they are.
const GATE_DEPTH: u32 = 7;

/// A spread of results to pack: both ends of every field, a real move, a
/// promotion, the null move, and a bit pattern that is not a move at all.
fn packable() -> Vec<(Move, i16, u8, Bound)> {
    let moves = [
        Move::NULL,
        Move::new_quiet(Square::E2, Square::E4),
        Move::new_capture(Square::D5, Square::E4),
        Move::new_promotion(Square::A7, Square::A8, PromoPiece::Queen),
        Move::new_castle(Square::E1, Square::H1),
        Move::from_bits(u16::MAX),
    ];
    let scores = [0i16, 1, -1, 694, -694, i16::MAX, i16::MIN, 31_744, -31_744];
    let depths = [0u8, 1, 5, 127, 255];
    let bounds = [Bound::Lower, Bound::Upper, Bound::Exact];
    let mut out = Vec::new();
    for &mv in &moves {
        for &score in &scores {
            for &depth in &depths {
                for &bound in &bounds {
                    out.push((mv, score, depth, bound));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The slot codec
// ---------------------------------------------------------------------------

#[test]
fn an_entry_round_trips_through_the_two_words() {
    let mut rng = Rng::new(0x7A11_0C10);
    let mut checked = 0;
    for (mv, score, depth, bound) in packable() {
        for age in [0u8, 1, 31, 63] {
            let key = rng.next_u64();
            let entry = Entry::new(mv, score, depth, bound, age);
            let word1 = entry.to_bits();
            let word0 = key ^ word1;
            let hit = tt::verify(word0, word1, key)
                .unwrap_or_else(|| panic!("{mv:?} {score} {depth} {bound:?} did not verify"));
            assert_eq!(hit.mv, mv);
            assert_eq!(hit.score, score);
            assert_eq!(hit.depth, depth);
            assert_eq!(hit.bound, bound);
            checked += 1;
        }
    }
    assert_eq!(checked, packable().len() * 4);
    assert!(checked >= 1000, "only {checked} packings checked");
}

/// The high sixteen bits of the data word are unused and must stay zero:
/// a static evaluation goes there when something reads one, and this is
/// what makes that a deliberate change rather than a silent one.
#[test]
fn the_unused_bits_of_a_slot_are_zero() {
    for (mv, score, depth, bound) in packable() {
        for age in 0..64u8 {
            let bits = Entry::new(mv, score, depth, bound, age).to_bits();
            assert_eq!(bits >> 48, 0, "{mv:?} {score} {depth} {bound:?} age {age}");
        }
    }
}

/// What a race produces: one word of one write beside one word of another.
/// Every such pair must fail validation, for either key. And so must every
/// single-bit corruption of an intact pair, which is the same check at its
/// smallest.
#[test]
fn a_torn_read_is_rejected() {
    let mut rng = Rng::new(0xB01D_FACE);
    let samples = packable();
    let mut mixed = 0;
    let mut flipped = 0;
    for i in 0..400 {
        let (mv_a, sc_a, d_a, b_a) = samples[rng.below(samples.len())];
        let (mv_b, sc_b, d_b, b_b) = samples[rng.below(samples.len())];
        let (key_a, key_b) = (rng.next_u64(), rng.next_u64());
        let data_a = Entry::new(mv_a, sc_a, d_a, b_a, 3).to_bits();
        let data_b = Entry::new(mv_b, sc_b, d_b, b_b, 4).to_bits();
        let (word0_a, word0_b) = (key_a ^ data_a, key_b ^ data_b);

        // The premise: intact, both verify. Without this the rest of the
        // test passes against a `verify` that always says no.
        assert!(tt::verify(word0_a, data_a, key_a).is_some());
        assert!(tt::verify(word0_b, data_b, key_b).is_some());

        if key_a ^ data_a != key_b ^ data_b {
            assert!(
                tt::verify(word0_a, data_b, key_a).is_none(),
                "a torn read verified against key A"
            );
            assert!(
                tt::verify(word0_b, data_a, key_b).is_none(),
                "a torn read verified against key B"
            );
            mixed += 1;
        }

        // Every single-bit flip of either word, on the first few samples:
        // 128 corruptions each, all of which must miss.
        if i < 8 {
            for bit in 0..64 {
                assert!(
                    tt::verify(word0_a ^ (1 << bit), data_a, key_a).is_none(),
                    "bit {bit} of the key word survived"
                );
                assert!(
                    tt::verify(word0_a, data_a ^ (1 << bit), key_a).is_none(),
                    "bit {bit} of the data word survived"
                );
                flipped += 2;
            }
        }
    }
    assert_eq!(mixed, 400, "some pairs were not distinguishable");
    assert_eq!(flipped, 8 * 128);
}

/// An untouched slot is two zero words. It must not read as a result for
/// any key, the zero key included: the bound field is zero and zero is not
/// a bound.
#[test]
fn an_untouched_slot_is_never_a_hit() {
    let mut rng = Rng::new(0x0E11_0000);
    assert_eq!(Entry::EMPTY.to_bits(), 0);
    assert!(Entry::EMPTY.decode().is_none());
    assert!(
        tt::verify(0, 0, 0).is_none(),
        "the zero key hit an empty slot"
    );
    for _ in 0..1000 {
        assert!(tt::verify(0, 0, rng.next_u64()).is_none());
    }
    let empty = table(1);
    for _ in 0..1000 {
        assert!(empty.probe(rng.next_u64()).is_none());
    }
}

// ---------------------------------------------------------------------------
// The mate scale
// ---------------------------------------------------------------------------

/// A mate score is stored as the distance from the node that stored it, so
/// an entry read from a different ply names the same mate. Stored at the
/// root's scale it would report a mate that never arrives.
#[test]
fn a_mate_score_is_stored_relative_to_its_node() {
    let mut checked = 0;
    for ply in 0..16usize {
        for distance in 1..16usize {
            let winning = mate_in(ply + distance);
            let stored = score::to_tt(winning, ply);
            assert_eq!(
                Score::from(stored),
                mate_in(distance),
                "mate_in({}) at ply {ply}",
                ply + distance
            );
            for reader in 0..16usize {
                assert_eq!(score::from_tt(stored, reader), mate_in(reader + distance));
            }

            let losing = mated_in(ply + distance);
            let stored = score::to_tt(losing, ply);
            assert_eq!(Score::from(stored), mated_in(distance));
            for reader in 0..16usize {
                assert_eq!(score::from_tt(stored, reader), mated_in(reader + distance));
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 16 * 15);
}

#[test]
fn an_evaluation_passes_through_the_table_unchanged() {
    for score in [0, 1, -1, 25, -694, MAX_EVAL, -MAX_EVAL, MAX_EVAL - 1] {
        for ply in [0usize, 1, 7, 64, 255] {
            let stored = score::to_tt(score, ply);
            assert_eq!(Score::from(stored), score, "{score} at ply {ply}");
            assert_eq!(score::from_tt(stored, ply), score);
        }
    }
}

// ---------------------------------------------------------------------------
// Sizing, and the option that sets it
// ---------------------------------------------------------------------------

#[test]
fn the_table_is_the_size_it_was_asked_for() {
    let mut sizes = Vec::new();
    for mb in [1usize, 2, 16, 48, 64] {
        let t = table(mb);
        assert_eq!(t.bytes(), mb << 20, "{mb} MB");
        assert_eq!(t.buckets(), (mb << 20) / 64, "{mb} MB");
        sizes.push(t.bytes());
    }
    // Not rounded down to a power of two: 48 is 48, not 32.
    assert_eq!(sizes[3], 48 << 20);
    sizes.dedup();
    assert_eq!(sizes.len(), 5, "two sizes allocated the same table");
    assert_eq!(no_table().bytes(), 0);
}

/// The point of declaring the option: setting it must move the allocation.
/// An advertised option that is ignored is worse than an absent one,
/// because the preset that names a size then means nothing.
#[test]
fn the_hash_option_changes_the_allocation() {
    let mut s = Session::new();
    assert_eq!(
        s.tt().bytes(),
        tt::DEFAULT_HASH_MB << 20,
        "a fresh session is not at the declared default"
    );
    let mut seen = Vec::new();
    for mb in [1usize, 64, 16, 48] {
        s.handle_line(&format!("setoption name Hash value {mb}"));
        assert_eq!(s.tt().bytes(), mb << 20, "Hash {mb} did not take effect");
        seen.push(s.tt().bytes());
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 4, "the four sizes were not four allocations");

    // Out of range is clamped, not obeyed and not refused.
    s.handle_line("setoption name Hash value 0");
    assert_eq!(s.tt().bytes(), tt::MIN_HASH_MB << 20);
    s.handle_line(&format!(
        "setoption name Hash value {}",
        tt::MAX_HASH_MB + 1
    ));
    assert_eq!(s.tt().bytes(), tt::MAX_HASH_MB << 20);

    // Nonsense leaves the table alone.
    s.handle_line("setoption name Hash value 8");
    s.handle_line("setoption name Hash value banana");
    assert_eq!(s.tt().bytes(), 8 << 20);
    // The name is matched without regard to case, as UCI names are.
    s.handle_line("setoption name hash value 2");
    assert_eq!(s.tt().bytes(), 2 << 20);
}

/// What a runner reads. `fastchess` warns once a game for an option it was
/// told to set and the engine does not declare, and the `OpenBench` presets
/// set both of these.
#[test]
fn uci_declares_hash_and_threads() {
    let out = support::talk("uci\nquit\n");
    let expected = [
        format!(
            "option name Hash type spin default {} min {} max {}",
            tt::DEFAULT_HASH_MB,
            tt::MIN_HASH_MB,
            tt::MAX_HASH_MB
        ),
        "option name Threads type spin default 1 min 1 max 1".to_string(),
    ];
    let uciok = out.lines().position(|l| l == "uciok").expect("uciok");
    for line in &expected {
        let at = out
            .lines()
            .position(|l| l == line)
            .unwrap_or_else(|| panic!("no `{line}` in {out:?}"));
        assert!(at < uciok, "`{line}` came after uciok");
    }
    // The preset's values are the ones that must work.
    for mb in [16, 64] {
        let out = support::talk(&format!(
            "uci\nsetoption name Hash value {mb}\nsetoption name Threads value 1\nisready\nquit\n"
        ));
        assert!(out.contains("readyok"), "{out:?}");
    }
}

// ---------------------------------------------------------------------------
// Replacement and aging
// ---------------------------------------------------------------------------

/// Keys are not needed to be distinct in the bucket sense here: a table of
/// one bucket puts everything in the same four slots, which is what makes
/// the replacement scheme observable at all.
fn one_bucket() -> Table {
    Table::with_buckets(1).expect("a one-bucket table")
}

/// A probe answers for the key it was given or not at all. Without this
/// stated directly it is only tested by accident, and a table that returns
/// whatever is in the bucket is deterministic, so the search gates below
/// -- which are about repeatability -- do not notice it.
#[test]
fn a_probe_returns_only_what_was_stored_for_that_key() {
    // A small table and many keys, so buckets are contended and a probe
    // that trusted the bucket would answer constantly.
    let t = Table::with_buckets(64).expect("a 64-bucket table");
    t.new_search();
    let mut rng = Rng::new(0xC0FF_EE00);
    let keys: Vec<u64> = (0..2000).map(|_| rng.next_u64()).collect();
    // The depth is a function of the key, so a result found under the
    // wrong key names itself.
    let depth_of = |key: u64| -> u8 { (key % 200) as u8 + 1 };
    for &key in &keys {
        t.store(key, Move::NULL, 0, depth_of(key), Bound::Exact);
    }
    let mut hits = 0;
    for &key in &keys {
        if let Some(hit) = t.probe(key) {
            assert_eq!(
                hit.depth,
                depth_of(key),
                "a probe for one key returned another key's result"
            );
            hits += 1;
        }
    }
    assert!(
        hits >= 64,
        "only {hits} of 2000 keys were still there, so nothing was probed"
    );
    // And keys that were never stored must miss, whatever the buckets hold.
    let mut wrong = 0;
    for _ in 0..2000 {
        if t.probe(rng.next_u64()).is_some() {
            wrong += 1;
        }
    }
    assert_eq!(
        wrong, 0,
        "{wrong} keys that were never stored found a result"
    );
}

#[test]
fn a_bucket_holds_four_results_at_once() {
    let t = one_bucket();
    t.new_search();
    let keys: Vec<u64> = (1..=4).map(|i| i * 0x0123_4567_89AB_CDEF).collect();
    for (i, &key) in keys.iter().enumerate() {
        t.store(key, Move::NULL, 10, 4 + i as u8, Bound::Exact);
    }
    for (i, &key) in keys.iter().enumerate() {
        let hit = t
            .probe(key)
            .unwrap_or_else(|| panic!("key {i} was evicted from a bucket with room"));
        assert_eq!(hit.depth, 4 + i as u8);
    }
}

/// A fifth result takes the shallowest slot, not an arbitrary one: the
/// deep results are the expensive ones.
#[test]
fn the_shallowest_slot_is_the_one_replaced() {
    let t = one_bucket();
    t.new_search();
    let keys: Vec<u64> = (1..=4).map(|i| i * 0x0123_4567_89AB_CDEF).collect();
    let depths = [9u8, 3, 7, 5];
    for (&key, &depth) in keys.iter().zip(&depths) {
        t.store(key, Move::NULL, 10, depth, Bound::Exact);
    }
    let newcomer = 0xFEED_FACE_CAFE_BEEF;
    t.store(newcomer, Move::NULL, 20, 6, Bound::Exact);
    assert!(t.probe(newcomer).is_some(), "the new result was not stored");
    assert!(t.probe(keys[1]).is_none(), "the depth-3 slot survived");
    for i in [0usize, 2, 3] {
        assert!(
            t.probe(keys[i]).is_some(),
            "the depth-{} slot was taken instead",
            depths[i]
        );
    }
}

#[test]
fn a_deeper_result_for_the_same_position_is_kept() {
    let t = one_bucket();
    t.new_search();
    let key = 0x0BAD_C0DE_0BAD_C0DE;
    t.store(key, Move::NULL, 100, 8, Bound::Exact);
    // A shallower bound tells the next probe less than what is there.
    t.store(key, Move::NULL, 200, 3, Bound::Lower);
    assert_eq!(t.probe(key).expect("still there").depth, 8);
    assert_eq!(t.probe(key).expect("still there").score, 100);
    // A shallower *exact* score is worth having: it is a value, not a bound.
    t.store(key, Move::NULL, 300, 3, Bound::Exact);
    assert_eq!(t.probe(key).expect("still there").depth, 3);
    assert_eq!(t.probe(key).expect("still there").score, 300);
    // And a deeper result always replaces.
    t.store(key, Move::NULL, 400, 12, Bound::Upper);
    assert_eq!(t.probe(key).expect("still there").depth, 12);
    // One position occupies one slot however many times it is stored.
    let other = 0x1111_2222_3333_4444;
    t.store(other, Move::NULL, 1, 1, Bound::Exact);
    assert!(t.probe(other).is_some());
    assert!(t.probe(key).is_some());
}

/// The equilibrium the four-slot bucket is for. A store always takes the
/// least valuable slot, so a shallow result does displace a deep one --
/// once. After that the shallow slot is itself the cheapest thing in the
/// bucket and the next shallow store takes it back, so a bucket of deep
/// results loses one slot to churn and keeps the other three.
///
/// The alternative, refusing a store worth less than everything present,
/// was rejected: it makes a bucket sticky, and since the leaves are almost
/// all of the tree, almost all stores would be refused.
#[test]
fn a_shallow_result_takes_one_slot_of_a_deep_bucket_and_no_more() {
    let t = one_bucket();
    t.new_search();
    let keys: Vec<u64> = (1..=4).map(|i| i * 0x0123_4567_89AB_CDEF).collect();
    for &key in &keys {
        t.store(key, Move::NULL, 10, 20, Bound::Exact);
    }
    let mut shallow = Vec::new();
    for i in 0..4u64 {
        let key = 0xAAAA_BBBB_CCCC_DDDD ^ (i * 0x1_0000_0000);
        t.store(key, Move::NULL, 10, 1, Bound::Exact);
        shallow.push(key);
        let deep = keys.iter().filter(|&&k| t.probe(k).is_some()).count();
        assert_eq!(
            deep,
            3,
            "after {} shallow stores the bucket holds {deep} deep results, not 3",
            i + 1
        );
    }
    // Only the last shallow result is still there: each took the previous
    // one's slot.
    let kept = shallow.iter().filter(|&&k| t.probe(k).is_some()).count();
    assert_eq!(kept, 1, "{kept} of the four shallow results survived");
    assert!(
        t.probe(shallow[3]).is_some(),
        "the newest one is the one kept"
    );
}

/// Depth preference alone would let a bucket of deep results hold its
/// slots for the rest of the game. Aging is what stops that: a generation
/// is worth eight ply, so what a previous search left is cheap by
/// comparison and the bucket turns over.
#[test]
fn a_new_search_ages_what_is_already_there() {
    let deep_survivors = |generations: usize| -> usize {
        let t = one_bucket();
        t.new_search();
        let keys: Vec<u64> = (1..=4).map(|i| i * 0x0123_4567_89AB_CDEF).collect();
        for &key in &keys {
            t.store(key, Move::NULL, 10, 20, Bound::Exact);
        }
        for _ in 0..generations {
            t.new_search();
        }
        for i in 0..4u64 {
            t.store(
                0xAAAA_BBBB_CCCC_DDDD ^ (i * 0x1_0000_0000),
                Move::NULL,
                10,
                1,
                Bound::Exact,
            );
        }
        keys.iter().filter(|&&k| t.probe(k).is_some()).count()
    };
    // In the same search, four shallow stores share one slot (above).
    assert_eq!(deep_survivors(0), 3);
    // Three generations on, twenty ply less twenty-four is worth less than
    // one, and the same four stores take the bucket over.
    assert_eq!(
        deep_survivors(3),
        0,
        "aging never freed the bucket: a table that fills once stays filled"
    );
    let t = one_bucket();
    for i in 1..=5 {
        t.new_search();
        assert_eq!(t.generation(), i);
    }
}

#[test]
fn clearing_empties_the_table_and_resets_the_generation() {
    let t = table(1);
    for _ in 0..5 {
        t.new_search();
    }
    let keys: Vec<u64> = (1..=200u64)
        .map(|i| i.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .collect();
    for &key in &keys {
        t.store(key, Move::NULL, 7, 3, Bound::Exact);
    }
    let before = keys.iter().filter(|&&k| t.probe(k).is_some()).count();
    assert!(before > 150, "only {before} of 200 results were stored");
    t.clear();
    assert_eq!(t.generation(), 0, "the generation survived a clear");
    for &key in &keys {
        assert!(t.probe(key).is_none(), "a result survived a clear");
    }
}

// ---------------------------------------------------------------------------
// What the table does to a search
// ---------------------------------------------------------------------------

/// The premise for everything below: the table is reached, and it saves
/// work. A gate that compares a search against itself proves nothing if
/// the table is never consulted.
///
/// The baseline is a table of no buckets, which is the search with no
/// table at all: measured over the bench positions it reproduces the
/// previous engine's node count exactly, so the table is the only thing
/// this change does.
#[test]
fn the_table_saves_nodes() {
    let mut cheaper = 0;
    let fens = gate_fens();
    let (mut total_with, mut total_without) = (0u64, 0u64);
    for fen in &fens {
        let (_, _, without) = search_with(&mut board(fen), GATE_DEPTH, &no_table());
        let (_, _, with) = search_with(&mut board(fen), GATE_DEPTH, &table(tt::DEFAULT_HASH_MB));
        assert!(
            with <= without,
            "{fen}: {with} nodes with the table, {without} without"
        );
        total_with += with;
        total_without += without;
        if with < without {
            cheaper += 1;
        }
    }
    println!(
        "depth {GATE_DEPTH}, {} positions: {total_without} nodes without the table, \
         {total_with} with, cheaper in {cheaper}",
        fens.len()
    );
    assert_eq!(
        cheaper,
        fens.len(),
        "the table saved nothing in {} of {} positions",
        fens.len() - cheaper,
        fens.len()
    );
    assert!(
        total_with * 2 < total_without,
        "{total_with} against {total_without} is not a saving worth a table"
    );
}

/// An entry says how deep the result under it was searched, and a probe
/// cuts on that. One ply of overstatement is the classic off-by-one here
/// and it is invisible from outside: the search still finishes, still
/// reports a move, and is worth slightly less.
///
/// It is visible from inside. After a search to depth D nothing below the
/// root was searched deeper than D-1, so no entry may claim more, and the
/// root's own children must claim exactly that.
///
/// **Narrowed when the check extension landed, and it got sharper rather
/// than looser.** "D-1 below the root" was the same arithmetic the search
/// used to justify its stack safety with, and this is the third place it
/// was written down. A root move that gives check is now searched at D, so
/// its entry claims D. What replaces the old bound is not a weaker one: the
/// depth a child is searched at is exactly D-1 when the move gave no check
/// and exactly D when it did, both checked here against the child's own
/// `in_check`, so this gate now also says the extension is one ply and
/// fires on the checking moves and no others. Nothing anywhere may claim
/// more than D, because a ply costs one and an extension gives back at most
/// one.
#[test]
fn no_entry_claims_more_depth_than_was_searched() {
    let depth = 6u32;
    for fen in [
        support::standard_fen("startpos"),
        support::standard_fen("pos3"),
        support::ENDGAME_FENS[3].to_string(),
    ] {
        let tt = table(tt::DEFAULT_HASH_MB);
        let mut b = board(&fen);
        let _ = search_with(&mut b, depth, &tt);
        let mut at_full_depth = 0;
        let mut children = 0;
        for m in generate_legal(&b).iter() {
            b.make_move(m);
            // What this child was entitled to: one ply back if the move
            // gave check, and nothing otherwise.
            let gave_check = b.in_check();
            let entitled = if gave_check { depth } else { depth - 1 };
            if let Some(hit) = tt.probe(b.key()) {
                assert!(
                    u32::from(hit.depth) <= entitled,
                    "{fen}: a child of the root claims depth {} after a depth-{depth} \
                     search, and its move gave {}",
                    hit.depth,
                    if gave_check { "check" } else { "no check" }
                );
                if u32::from(hit.depth) == entitled {
                    at_full_depth += 1;
                }
                children += 1;
            }
            b.unmake_move(m);
        }
        assert!(
            at_full_depth * 2 >= children && at_full_depth > 0,
            "{fen}: only {at_full_depth} of {children} stored children were searched to \
             the depth their move entitled them to, so the claim above is not being \
             checked against anything"
        );
    }
}

/// The same position searched again, on the table the first search filled,
/// must give the same move. The table is the change most likely to break
/// that invariant.
///
/// The coverage assertion is the point. A table that is never written
/// passes this trivially, so the second search must also be cheaper than
/// the first in most positions; if it is not, the table was cold and the
/// stability observed is nothing.
#[test]
fn repeated_searches_on_a_warm_table_return_the_same_move() {
    let fens = gate_fens();
    let mut warmed = 0;
    for fen in &fens {
        let tt = table(tt::DEFAULT_HASH_MB);
        let (first, first_score, first_nodes) = search_with(&mut board(fen), GATE_DEPTH, &tt);
        let mut cheapest = first_nodes;
        for repeat in 1..6 {
            let (again, score, nodes) = search_with(&mut board(fen), GATE_DEPTH, &tt);
            assert_eq!(
                again, first,
                "{fen}: search {repeat} changed its move on a warm table"
            );
            assert_eq!(
                score, first_score,
                "{fen}: search {repeat} changed its score on a warm table"
            );
            cheapest = cheapest.min(nodes);
        }
        if cheapest < first_nodes {
            warmed += 1;
        }
    }
    assert_eq!(
        warmed,
        fens.len(),
        "the table was cold in {} of {} positions, so this gate proved nothing there",
        fens.len() - warmed,
        fens.len()
    );
}

/// A mate found through a warm table keeps its distance. Storing a mate
/// score at the root's scale rather than the node's is the classic table
/// bug, and it presents as an engine that announces mate in four, plays a
/// move, and announces mate in four again.
#[test]
fn mate_distances_survive_a_warm_table() {
    // Mate in two, three plies, found at depth four (the mated side is
    // found to have no moves only at a node that generates them).
    let positions = [
        ("8/7k/R7/8/8/8/8/1R4K1 w - - 0 1", mate_in(3)),
        ("7k/8/5K2/8/8/8/8/Q7 w - - 0 1", mate_in(3)),
        ("4r1k1/5ppp/8/3Q4/2B5/8/8/6K1 w - - 0 1", mate_in(3)),
        // Mate in one, and being mated in one.
        ("7k/8/6K1/8/8/8/8/1R6 w - - 0 1", mate_in(1)),
    ];
    for (fen, expected) in positions {
        let tt = table(tt::DEFAULT_HASH_MB);
        for repeat in 0..4 {
            let (_, score, _) = search_with(&mut board(fen), 4, &tt);
            assert_eq!(score, expected, "{fen}: search {repeat} scored {score}");
        }
        // The same table, now at other depths: an entry written at one
        // depth is read at another, which is where an unadjusted mate
        // score surfaces.
        for depth in [2, 3, 4, 5, 6] {
            let (_, score, _) = search_with(&mut board(fen), depth, &tt);
            if depth >= 4 {
                assert_eq!(score, expected, "{fen} at depth {depth}: scored {score}");
            }
        }
    }

    // Being mated keeps its distance too, at every depth past the mate.
    let fen = "7k/8/6K1/8/8/8/8/1R6 b - - 0 1";
    let tt = table(tt::DEFAULT_HASH_MB);
    for depth in [3, 4, 5, 6, 5, 4, 3] {
        let (_, score, _) = search_with(&mut board(fen), depth, &tt);
        assert_eq!(score, mated_in(2), "depth {depth}: scored {score}");
    }
}

/// Three searches of one position in one process, with `between` sent
/// before the third: their node counts, in order.
///
/// Interactive rather than piped. `talk` sends `quit` with everything
/// else, and `quit` -- correctly -- stops a search that is still running,
/// so a piped session completes no iteration and prints no `info` line to
/// read a node count off.
fn three_searches(setup: &str, between: &[&str]) -> Vec<u64> {
    let go = format!("go depth {GATE_DEPTH}");
    let mut engine = support::Engine::spawn();
    engine.sync();
    let mut counts = Vec::new();
    for round in 0..3 {
        if round == 2 {
            for line in between {
                engine.send(line);
            }
        }
        engine.send(setup);
        engine.send(&go);
        let lines = engine.read_until("bestmove ");
        let info = lines
            .iter()
            .rfind(|l| l.starts_with(&format!("info depth {GATE_DEPTH} ")))
            .unwrap_or_else(|| panic!("no completed iteration in {lines:?}"));
        let toks: Vec<&str> = info.split_whitespace().collect();
        let at = toks.iter().position(|t| *t == "nodes").expect("nodes");
        counts.push(toks[at + 1].parse().expect("a node count"));
    }
    engine.quit();
    counts
}

/// `ucinewgame` empties the table, so the search after it costs what a
/// first search costs. Driven through the binary, because that is where
/// the command is handled and where a GUI sends it.
#[test]
fn ucinewgame_makes_the_next_search_cold() {
    let setup = format!("position fen {}", support::standard_fen("startpos"));
    let counts = three_searches(&setup, &["ucinewgame"]);
    assert!(
        counts[1] < counts[0],
        "the second search was not cheaper, so the table is not being kept: {counts:?}"
    );
    assert_eq!(
        counts[2], counts[0],
        "the search after ucinewgame did not start cold: {counts:?}"
    );
}

/// Setting `Hash` replaces the table, so it starts cold too, even when the
/// size asked for is the size already in use. This is the other half of
/// the option taking effect: a new size is a new table, not a resized one.
#[test]
fn setting_hash_makes_the_next_search_cold() {
    let setup = format!("position fen {}", support::standard_fen("startpos"));
    let same = format!("setoption name Hash value {}", tt::DEFAULT_HASH_MB);
    let counts = three_searches(&setup, &[&same]);
    assert!(counts[1] < counts[0], "{counts:?}");
    assert_eq!(
        counts[2], counts[0],
        "the search after setting Hash did not start cold: {counts:?}"
    );
}

// ---------------------------------------------------------------------------
// The bench seam
// ---------------------------------------------------------------------------

/// The TT is cleared between positions. Without this the total depends on
/// position order and on whatever `go` ran before it.
///
/// Every position's count must therefore equal what that position costs on
/// its own. The second half of the test is the coverage assertion: run the
/// same positions without clearing and show that the total really does
/// move, so that the first half is testing something.
#[test]
fn the_bench_clears_the_table_between_positions() {
    let report = bench::bench();
    let fens = bench::positions();
    assert_eq!(report.lines.len(), fens.len());
    for line in &report.lines {
        let (best, _, nodes) =
            search_with(&mut board(&line.fen), bench::DEPTH, &table(bench::HASH_MB));
        assert_eq!(
            nodes, line.nodes,
            "{}: the bench counted {} nodes, a standalone search {nodes}",
            line.fen, line.nodes
        );
        assert_eq!(best, line.best, "{}", line.fen);
    }

    // The coverage assertion, and it is not the obvious one. Running the
    // list twice on one table without clearing gives the *same* total the
    // first time round, because no two bench positions transpose into each
    // other at depth three. What carries across a seam is what the table
    // learned about the position just searched, so the demonstration is a
    // second pass: it is cheaper, and a bench that did not clear would be
    // reporting that number for some of its positions.
    let shared = table(bench::HASH_MB);
    let pass = |shared: &Table| -> u64 {
        fens.iter()
            .map(|fen| search_with(&mut board(fen), bench::DEPTH, shared).2)
            .sum()
    };
    let first = pass(&shared);
    let second = pass(&shared);
    assert!(
        second < first,
        "a second pass over the same table cost {second} against {first}, so nothing \
         carries across a seam and this gate proves nothing"
    );
}

/// The table size the bench runs at is part of the determinism contract:
/// it is compiled in, not passed on the command line, and the summary line
/// says what it was.
#[test]
fn the_bench_records_the_table_size_it_ran_at() {
    const { assert!(bench::HASH_MB >= 1) };
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cadence"))
        .arg("bench")
        .output()
        .expect("run cadence bench");
    let out = String::from_utf8(out.stdout).expect("stdout is UTF-8");
    assert!(
        out.contains(&format!("hash {} MB", bench::HASH_MB)),
        "the summary does not name the table size: {out}"
    );
}
