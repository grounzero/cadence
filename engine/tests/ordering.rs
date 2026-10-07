// SPDX-License-Identifier: GPL-3.0-or-later

//! Ordering changes no result, so the gates pin node counts and what the ordering refuses. A table
//! or killer move the position does not have must never reach `make_move`, which panics in release;
//! `order_first` validates for free, because finding the move is the operation.

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::AtomicBool;

use cadence_core::fen::FenStyle;
use cadence_core::position::Board;
use cadence_core::{Move, Square, generate_legal, generate_noisy};
use cadence_engine::picker::{noisy_key, sort_from, sort_noisy};
use cadence_engine::position::Position;
use cadence_engine::score::Score;
use cadence_engine::search::{Limits, order_first, remember_killer};
use cadence_engine::see::see;
use cadence_engine::tt::{self, Bound, Table};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

fn search_with(board: &mut Position, depth: u32, tt: &Table) -> (Move, Score, u64) {
    let stop = AtomicBool::new(false);
    let mut sink = Vec::new();
    let mut s = support::search(Limits::depth(depth), &stop, tt);
    let best = s.run(board, &mut sink);
    (best, s.score(), s.nodes())
}

/// The same set as `tests/tt.rs`, so the node counts are comparable.
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

const DEEP: u32 = 7;

/// Dense move lists, and both ends of the DFRC range for the king-takes-rook encoding.
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

/// Stated as the generated list minus the move, not as a rotation, so the gate is not the
/// implementation written twice.
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

/// No bit pattern is reserved, so this is every move a collision could produce; only the legal ones
/// are accepted, and the rest leave the list as it was.
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

/// `Move::NULL` is what an empty slot holds, and no generator emits it.
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

/// The two shapes of bogus move `make_move` does not survive.
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

/// `make_move` reads the mover and victim with `expect`, so in release too an unchecked table move
/// kills the process.
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

/// At depth two the interior nodes are exactly the root's children, so this poisons every key the
/// search can probe; `depth = 0` means the entry is read for its move alone.
fn poison(b: &mut Position, tt: &Table, pick: impl Fn(&Board) -> Move) {
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

fn poisonable() -> Vec<String> {
    support::corpus_fens()
        .into_iter()
        .filter(|f| !generate_legal(&board(f)).is_empty())
        .collect()
}

const POISON_DEPTH: u32 = 2;

/// Against the same table poisoned with `Move::NULL`, so a moved count is the move field's doing
/// alone. An `order_first` never called passes every gate above and fails this.
#[test]
fn the_tables_move_is_read_at_every_interior_node() {
    let fens = poisonable();
    let mut moved = 0;
    for fen in &fens {
        let quiet = table();
        let mut b = support::position(fen);
        poison(&mut b, &quiet, |_| Move::NULL);
        let (_, _, null_nodes) = search_with(&mut b, POISON_DEPTH, &quiet);

        let loud = table();
        let mut b = support::position(fen);
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

/// Were the move played, `make_move` would panic and this test would abort rather than report.
#[test]
fn a_move_the_table_cannot_supply_is_ignored_by_the_search() {
    let fens = poisonable();
    let mut checked = 0;
    for fen in &fens {
        let quiet = table();
        let mut b = support::position(fen);
        poison(&mut b, &quiet, |_| Move::NULL);
        let clean = search_with(&mut b, POISON_DEPTH, &quiet);

        for which in 0..2 {
            let poisoned = table();
            let mut b = support::position(fen);
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

/// Against no table, so it mixes the score's saving with the move's and the sort's: the one gate
/// that fails if `negamax` never reads the move. The bound sits where it still fails for that
/// reason, since the killers overlap the table's move and take its share.
#[test]
fn the_tables_move_saves_nodes() {
    let fens = deep_fens();
    let (mut with, mut without) = (0u64, 0u64);
    for fen in &fens {
        let (_, _, w) = search_with(
            &mut support::position(fen),
            DEEP,
            &Table::new(tt::DEFAULT_HASH_MB).expect("a table"),
        );
        let (_, _, wo) = search_with(
            &mut support::position(fen),
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

/// What the quiescence search's sort is handed.
const NO_KILLERS: [Move; 2] = [Move::NULL; 2];

/// Far from the picker's own values, so the gates say below every noisy move rather than restate
/// its constants; `noisy_key` spans -64 to 101.
const KILLER_ONE: i32 = -1_000;
const KILLER_TWO: i32 = -1_001;
const LOSING_BASE: i32 = -10_000;
const QUIET_RANK: i32 = -100_000;

/// Written out rather than taken from the code under test: keeping noisy moves by MVV-LVA, killer
/// one, killer two, losing noisy moves in MVV-LVA order, then every other quiet move. `see` is
/// asked through the picker's own public function, since what is gated is where its answer puts the
/// move.
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

/// Each in the order the generator emitted them.
fn losing_and_rest(b: &Board) -> (Vec<Move>, Vec<Move>) {
    let noisy: Vec<Move> = generate_legal(b).iter().filter(|m| m.is_noisy()).collect();
    let losing = noisy.iter().copied().filter(|&m| see(b, m) < 0).collect();
    let rest = noisy.iter().copied().filter(|&m| see(b, m) >= 0).collect();
    (losing, rest)
}

/// The corpus holds too few losing captures to say a gate covered anything; its children are a few
/// thousand, reached deterministically.
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

/// A losing noisy move, a keeping one, and three quiet moves so a killer is not the only quiet
/// move.
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

/// `Move::NULL` matches no killer, so this is the rank before the killers existed.
fn rank(b: &Board, m: Move) -> i32 {
    rank_with(b, m, NO_KILLERS)
}

fn quiets(b: &Board) -> Vec<Move> {
    generate_legal(b).iter().filter(|m| !m.is_noisy()).collect()
}

fn sorted_with(fen: &str, start: usize, killers: [Move; 2]) -> (Board, Vec<Move>, Vec<Move>) {
    let b = board(fen);
    let generated: Vec<Move> = generate_legal(&b).iter().collect();
    let mut list = generate_legal(&b);
    sort_from(&b, &mut list, start, killers, &[]);
    let after: Vec<Move> = list.iter().collect();
    (b, generated, after)
}

fn sorted(fen: &str, start: usize) -> (Board, Vec<Move>, Vec<Move>) {
    sorted_with(fen, start, NO_KILLERS)
}

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

/// Counted from the generated list, so a sort that dropped a noisy move could not pass by having
/// fewer at the front.
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

/// As subsequence equality per rank, covering the quiet moves and the captures sharing a victim and
/// attacker.
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

/// Everything behind `start` is sorted as if the list began there.
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

/// The table's move stays at the head whatever it ranks: a quiet move in front of a queen capture
/// is the point of the stage order.
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
// A losing capture sits in a band below both killers and above every other quiet move. Its
// position, its membership by exchange rather than capture, and its internal order are separate
// claims.

/// The stage order, stated as indices into the sorted list.
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

/// Where the losing capture takes the more valuable piece, a rule reading the victim puts them the
/// wrong way round; the count of those cases is the coverage.
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

/// A flat band would also discard their MVV-LVA order, a second claim with a second number.
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

/// `sort_noisy` is the one sort that does not demote.
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

/// `quiesce` refuses every losing noisy move, so the moves it searches come out in the same order
/// either way, which is why `sort_noisy` pays for no `see` call.
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

/// Six dominates seven for both gates below: a 0.71% window against 0.023% for the demotion, at a
/// fifth of the nodes.
const SORT_DEPTH: u32 = 6;

/// With no table, `order_first` never fires and the sort is the only ordering, so the saving is
/// attributable to it. The counterfactual is re-measured at every pruning change, which can cut it
/// faster than the shipped tree.
#[test]
fn the_capture_sort_saves_nodes() {
    let fens = deep_fens();
    let mut total = 0u64;
    for fen in &fens {
        let (_, _, n) = search_with(
            &mut support::position(fen),
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
        total * 10 < 9 * 1_281_421,
        "{total} nodes against the 1,281,421 the same search took with no ordering at all"
    );
}

/// With no table, so the saving is the demotion's, a window of a fraction of a per cent of exact
/// counts. The sign is checked at each re-base, the promoted arm taken by flipping the sort's flag
/// for the measurement only.
#[test]
fn demoting_the_losing_captures_saves_nodes() {
    let fens = deep_fens();
    let mut total = 0u64;
    for fen in &fens {
        let (_, _, n) = search_with(
            &mut support::position(fen),
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
        total < 442_105,
        "{total} nodes against the 442,105 the same search took with every capture ahead of the killers"
    );
}

// ---------------------------------------------------------------------------
// The killers, on their own
// ---------------------------------------------------------------------------

/// Three quiet moves is the fewest that leaves one outside both slots.
fn killer_fens() -> Vec<String> {
    support::corpus_fens()
        .into_iter()
        .filter(|fen| quiets(&board(fen)).len() >= 3)
        .collect()
}

/// From the end, because the first quiet move generated is where an unsorted list already puts it.
fn late_killers(b: &Board) -> [Move; 2] {
    let q = quiets(b);
    [q[q.len() - 1], q[q.len() - 2]]
}

/// As two indices: a killer is behind the keeping noisy moves, not all of them, since the losing
/// band sits behind both killers.
#[test]
fn a_killer_is_tried_after_every_keeping_noisy_move_and_ahead_of_every_other_quiet() {
    // Five corpus positions have a losing capture, too few to exercise the narrowing.
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

/// The killers here are always the reverse of generation order, so a single shared rank behind a
/// stable sort fails on every position.
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

/// The noisy moves stay ordered among themselves, the two killers between them and the rest.
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

/// A killer cut at a sibling and may be illegal here; the comparison that finds it in the list is
/// the whole check, so no pseudo-legality checker is needed.
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

/// Nothing in the signature says a slot holds a quiet move; the branch order in `picker::move_key`
/// enforces it, and a capture ranked as a killer would sort below every other capture.
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

/// When the table's move is the killer it stays at the head, unranked, and no second copy appears.
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
                // A losing capture is behind both killers now, so it is part of the tail this index
                // skips.
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

/// Three quiet moves and a capture.
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

/// A move already in slot one is promoted rather than duplicated.
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

/// The shift would fill both slots with one move.
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

const REUSE_DEPTH: u32 = 5;

/// `bench` and the UCI layer build a fresh `Search` each time, so nothing reaches this today; it
/// pins that a node count is a function of the code and the table handed in. With no table, the
/// killers are the only state that could carry.
#[test]
fn a_reused_search_remembers_no_killers() {
    let fens = deep_fens();
    let stop = AtomicBool::new(false);
    let mut sink = Vec::new();
    let mut pairs = 0;
    for pair in fens.windows(2) {
        let tt = Table::with_buckets(0).expect("a table of no buckets");
        let mut reused = support::search(Limits::depth(REUSE_DEPTH), &stop, &tt);
        reused.run(&mut support::position(&pair[0]), &mut sink);
        sink.clear();
        let again = reused.run(&mut support::position(&pair[1]), &mut sink);
        let after = reused.nodes();

        let fresh_tt = Table::with_buckets(0).expect("a table of no buckets");
        let mut fresh = support::search(Limits::depth(REUSE_DEPTH), &stop, &fresh_tt);
        sink.clear();
        let alone = fresh.run(&mut support::position(&pair[1]), &mut sink);
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
