// SPDX-License-Identifier: GPL-3.0-or-later

//! The main search's picker against the sort it replaces: the same moves in the same order, ties
//! included, and SEE computed only for noisy candidates examined in rank order.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::fen::FenStyle;
use cadence_core::position::Board;
use cadence_core::{Colour, MAX_MOVES, Move, MoveList, generate_legal};
use cadence_engine::history::SPAN;
use cadence_engine::picker::{Picker, Slot, noisy_key, sort_from};
use cadence_engine::search::{Limits, order_first};
use cadence_engine::see::see;
use cadence_engine::tt::{self, Table};

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The corpus and every position one move from it, about eleven hundred, so that losing captures,
/// promotions and long quiet lists all occur.
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

/// Both sides' tables after a real search, so the quiet moves are ranked as a search ranks them.
fn searched_history() -> [Vec<i32>; 2] {
    let tt = Table::new(tt::DEFAULT_HASH_MB).expect("a default-sized transposition table");
    let stop = AtomicBool::new(false);
    let mut s = support::search(Limits::depth(9), &stop, &tt);
    let mut b = support::position(&support::standard_fen("kiwipete"));
    s.run(&mut b, &mut Vec::new());
    let h = s.history();
    [
        h.side(Colour::White).to_vec(),
        h.side(Colour::Black).to_vec(),
    ]
}

/// The two quiet moves generated last, where an unordered list would try them last.
fn late_killers(b: &Board) -> [Move; 2] {
    let q: Vec<Move> = generate_legal(b).iter().filter(|m| !m.is_noisy()).collect();
    match q.len() {
        0 => [Move::NULL; 2],
        1 => [q[0], Move::NULL],
        n => [q[n - 1], q[n - 2]],
    }
}

/// The node's list as the search holds it after the table's move is put in front.
fn node_list(b: &Board, tt_move: Move) -> (MoveList, usize) {
    let mut list = generate_legal(b);
    let start = usize::from(order_first(&mut list, tt_move));
    (list, start)
}

/// The sequence the picker yields, and the exchanges it had read when it yielded each move.
fn picked(
    b: &Board,
    mut list: MoveList,
    start: usize,
    killers: [Move; 2],
    history: &[i32],
) -> (Vec<Move>, Vec<u32>) {
    let mut slots = [Slot::default(); MAX_MOVES];
    let mut picker = Picker::new(b, &mut list, start, killers, history, &mut slots);
    let (mut moves, mut exchanges) = (Vec::new(), Vec::new());
    while let Some((m, i, _)) = picker.next(b, &mut list, &mut slots) {
        assert_eq!(i, moves.len(), "the index is the yield count");
        moves.push(m);
        exchanges.push(picker.exchanges());
    }
    (moves, exchanges)
}

fn sorted(
    b: &Board,
    mut list: MoveList,
    start: usize,
    killers: [Move; 2],
    history: &[i32],
) -> Vec<Move> {
    support::sort_from(b, &mut list, start, killers, history);
    list.iter().collect()
}

/// Every position with no table move and with its last legal move planted as the table's, with no
/// killers and with two planted late, ranked by `history`.
fn each_node(
    history: &[Vec<i32>; 2],
    mut check: impl FnMut(&str, &Board, MoveList, usize, [Move; 2], &[i32]),
) -> usize {
    let mut nodes = 0;
    for fen in corpus_and_children() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        let Some(last) = legal.iter().last() else {
            continue;
        };
        let side = &history[b.side_to_move().index()];
        for tt_move in [Move::NULL, last] {
            for killers in [[Move::NULL; 2], late_killers(&b)] {
                let (list, start) = node_list(&b, tt_move);
                check(&fen, &b, list, start, killers, side);
                nodes += 1;
            }
        }
    }
    nodes
}

#[test]
fn the_picker_yields_the_sorted_order_in_every_position() {
    let history = searched_history();
    let nodes = each_node(&history, |fen, b, list, start, killers, side| {
        let want = sorted(b, list.clone(), start, killers, side);
        let (got, _) = picked(b, list, start, killers, side);
        assert_eq!(got, want, "{fen}: start {start}, killers {killers:?}");
    });
    assert!(nodes > 4_000, "only {nodes} nodes checked");
}

/// Every quiet move at one history score, so the quiet band is a single tie, beside the noisy
/// moves of equal MVV-LVA key the corpus already holds.
#[test]
fn ties_fall_in_generation_order() {
    let flat = [vec![300; SPAN], vec![300; SPAN]];
    let (mut quiet_ties, mut noisy_ties) = (0, 0);
    each_node(&flat, |fen, b, list, start, killers, side| {
        let rest = &list.as_slice()[start..];
        quiet_ties += usize::from(rest.iter().filter(|m| !m.is_noisy()).count() >= 3);
        let mut keys: Vec<i32> = rest
            .iter()
            .filter(|m| m.is_noisy())
            .map(|&m| noisy_key(b, m))
            .collect();
        keys.sort_unstable();
        noisy_ties += usize::from(keys.windows(2).any(|w| w[0] == w[1]));
        let want = sorted(b, list.clone(), start, killers, side);
        let (got, _) = picked(b, list, start, killers, side);
        assert_eq!(got, want, "{fen}: start {start}, killers {killers:?}");
    });
    assert!(
        quiet_ties > 1_000,
        "only {quiet_ties} lists with three tied quiet moves"
    );
    assert!(
        noisy_ties > 100,
        "only {noisy_ties} lists with two noisy moves of one key"
    );
}

/// SEE is computed only for noisy candidates examined while selecting the next move, in rank order,
/// and never for lower-ranked, unvisited ones: none for the table's move, every noisy move ranked at
/// or above a winning one when it is yielded, and each noisy move once by the end.
#[test]
fn see_is_computed_only_for_noisy_candidates_examined_in_rank_order() {
    let history = searched_history();
    let mut tested = 0;
    each_node(&history, |fen, b, list, start, killers, side| {
        let mut ranked: Vec<(i32, usize, Move)> = list.as_slice()[start..]
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_noisy())
            .map(|(generated, &m)| (noisy_key(b, m), generated, m))
            .collect();
        ranked.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.cmp(&y.1)));
        let total = ranked.len() as u32;
        let (moves, exchanges) = picked(b, list, start, killers, side);
        for (i, (&m, &read)) in moves.iter().zip(&exchanges).enumerate() {
            let want = if i < start {
                0
            } else if m.is_noisy() && see(b, m) >= 0 {
                ranked
                    .iter()
                    .position(|r| r.2 == m)
                    .expect("a ranked noisy move") as u32
                    + 1
            } else {
                total
            };
            assert_eq!(read, want, "{fen}: {read} exchanges read at move {i}");
        }
        tested += usize::from(total > 1);
    });
    assert!(
        tested > 500,
        "only {tested} lists with two noisy moves to choose between"
    );
}

fn fresh() -> [Slot; MAX_MOVES] {
    [Slot::default(); MAX_MOVES]
}

/// The picker's sequence over `slots`, held to each move of the list exactly once and to the sort's
/// order.
fn checked(
    b: &Board,
    list: &MoveList,
    start: usize,
    killers: [Move; 2],
    history: &[i32],
    slots: &mut [Slot],
) -> Vec<Move> {
    let mut work = list.clone();
    let mut picker = Picker::new(b, &mut work, start, killers, history, slots);
    let mut got = Vec::new();
    while let Some((m, _, _)) = picker.next(b, &mut work, slots) {
        got.push(m);
    }
    let mut given: Vec<u16> = list.iter().map(Move::to_bits).collect();
    let mut yielded: Vec<u16> = got.iter().map(|m| m.to_bits()).collect();
    given.sort_unstable();
    yielded.sort_unstable();
    assert_eq!(yielded, given, "each move of the list exactly once");
    assert_eq!(
        got,
        sorted(b, list.clone(), start, killers, history),
        "the sort's order"
    );
    got
}

fn one(m: Move) -> MoveList {
    let mut list = MoveList::new();
    list.push(m);
    list
}

#[test]
fn a_table_move_equal_to_either_killer_appears_once() {
    let history = searched_history();
    let mut cases = 0;
    for fen in corpus_and_children() {
        let b = board(&fen);
        let killers = late_killers(&b);
        let side = &history[b.side_to_move().index()];
        for k in killers.into_iter().filter(|k| !k.is_null()) {
            let (list, start) = node_list(&b, k);
            let got = checked(&b, &list, start, killers, side, &mut fresh());
            assert_eq!(got[0], k, "{fen}: the table's move first");
            cases += 1;
        }
    }
    assert!(cases > 1_000, "only {cases} cases");
}

#[test]
fn identical_killers_appear_once() {
    let mut cases = 0;
    for fen in corpus_and_children() {
        let b = board(&fen);
        let legal = generate_legal(&b);
        let Some(q) = legal.iter().find(|m| !m.is_noisy()) else {
            continue;
        };
        let last = legal.iter().last().expect("a legal move");
        for tt_move in [Move::NULL, q, last] {
            let (list, start) = node_list(&b, tt_move);
            checked(&b, &list, start, [q, q], &[], &mut fresh());
            cases += 1;
        }
    }
    assert!(cases > 1_000, "only {cases} cases");
}

#[test]
fn a_table_capture_or_promotion_appears_once() {
    let (mut captures, mut promotions) = (0, 0);
    for fen in corpus_and_children() {
        let b = board(&fen);
        for m in generate_legal(&b).iter().filter(|m| m.is_noisy()) {
            let (list, start) = node_list(&b, m);
            let got = checked(&b, &list, start, late_killers(&b), &[], &mut fresh());
            assert_eq!(got[0], m, "{fen}: the table's move first");
            captures += usize::from(m.is_capture());
            promotions += usize::from(m.promotion_piece().is_some());
        }
    }
    assert!(captures > 1_000, "only {captures} table captures");
    assert!(promotions > 10, "only {promotions} table promotions");
}

#[test]
fn an_empty_list_yields_nothing() {
    let b = board(&support::standard_fen("startpos"));
    let mut list = MoveList::new();
    let mut slots = fresh();
    let mut picker = Picker::new(&b, &mut list, 0, [Move::NULL; 2], &[], &mut slots);
    assert!(picker.next(&b, &mut list, &mut slots).is_none());
    assert_eq!(picker.exchanges(), 0);
}

/// A quiet move and a capture, each alone, with and without being the table's move.
#[test]
fn a_single_move_list_yields_it_once() {
    let b = board(&support::standard_fen("kiwipete"));
    let legal = generate_legal(&b);
    let quiet = legal.iter().find(|m| !m.is_noisy()).expect("a quiet move");
    let capture = legal.iter().find(|m| m.is_capture()).expect("a capture");
    for m in [quiet, capture] {
        for start in [0, 1] {
            assert_eq!(
                checked(&b, &one(m), start, [Move::NULL; 2], &[], &mut fresh()),
                vec![m]
            );
        }
    }
}

#[test]
fn an_all_quiet_list_is_the_sorted_order() {
    let history = searched_history();
    let b = board(&support::standard_fen("startpos"));
    assert!(generate_legal(&b).iter().all(|m| !m.is_noisy()));
    let side = &history[b.side_to_move().index()];
    let last = generate_legal(&b).iter().last().expect("a legal move");
    for tt_move in [Move::NULL, last] {
        let (list, start) = node_list(&b, tt_move);
        checked(&b, &list, start, late_killers(&b), side, &mut fresh());
    }
}

/// Only the losing captures of positions holding two or more, so the list is one band.
#[test]
fn an_all_losing_capture_list_is_the_sorted_order() {
    let mut cases = 0;
    for fen in corpus_and_children() {
        let b = board(&fen);
        let losing: Vec<Move> = generate_legal(&b)
            .iter()
            .filter(|&m| m.is_capture() && m.promotion_piece().is_none() && see(&b, m) < 0)
            .collect();
        if losing.len() < 2 {
            continue;
        }
        let mut list = MoveList::new();
        for &m in &losing {
            list.push(m);
        }
        checked(&b, &list, 0, [Move::NULL; 2], &[], &mut fresh());
        cases += 1;
    }
    assert!(cases > 20, "only {cases} lists of losing captures");
}

/// Every promotion loses to the rook, so all four fall in the losing promotions' band.
#[test]
fn losing_underpromotions_keep_the_sorted_order() {
    let b = board("r6k/4P3/8/8/8/8/8/4K3 w - - 0 1");
    let promotions: Vec<Move> = generate_legal(&b)
        .iter()
        .filter(|m| m.promotion_piece().is_some())
        .collect();
    assert_eq!(promotions.len(), 4);
    assert!(
        promotions.iter().all(|&m| see(&b, m) < 0),
        "every promotion loses"
    );
    for tt_move in [Move::NULL, promotions[3]] {
        let (list, start) = node_list(&b, tt_move);
        checked(&b, &list, start, [Move::NULL; 2], &[], &mut fresh());
    }
}

#[test]
fn quiet_evasions_alone_are_the_sorted_order() {
    let b = board("4k3/8/8/8/8/8/8/r3K3 w - - 0 1");
    assert!(b.in_check());
    let legal = generate_legal(&b);
    assert!(legal.len() > 1 && legal.iter().all(|m| !m.is_noisy()));
    let last = legal.iter().last().expect("a legal move");
    for tt_move in [Move::NULL, last] {
        let (list, start) = node_list(&b, tt_move);
        checked(&b, &list, start, late_killers(&b), &[], &mut fresh());
    }
}

#[test]
fn a_single_legal_evasion_is_yielded_once() {
    let b = board("6k1/8/8/8/8/3N4/5PPP/3r2K1 w - - 0 1");
    assert!(b.in_check());
    let legal = generate_legal(&b);
    assert_eq!(legal.len(), 1);
    let only = legal.iter().next().expect("one evasion");
    for tt_move in [Move::NULL, only] {
        let (list, start) = node_list(&b, tt_move);
        assert_eq!(
            checked(&b, &list, start, [Move::NULL; 2], &[], &mut fresh()),
            vec![only]
        );
    }
}

const NEAR_MAX_MOVES: &str = "R6R/3Q4/1Q4Q1/4Q3/2Q4Q/Q4Q2/pp1Q4/kBNN1KB1 w - - 0 1";

#[test]
fn a_list_near_max_moves_is_the_sorted_order() {
    let history = searched_history();
    let b = board(NEAR_MAX_MOVES);
    let legal = generate_legal(&b);
    assert!(legal.len() >= 200, "{} moves", legal.len());
    let side = &history[b.side_to_move().index()];
    let last = legal.iter().last().expect("a legal move");
    for tt_move in [Move::NULL, last] {
        let (list, start) = node_list(&b, tt_move);
        checked(&b, &list, start, late_killers(&b), side, &mut fresh());
    }
}

/// One row of slots, left holding a longer list's entries, then reused for a shorter list, with and
/// without the table's move at its head.
#[test]
fn a_reused_row_poisoned_by_a_longer_list_keeps_the_sorted_order() {
    let history = searched_history();
    let long = board(NEAR_MAX_MOVES);
    let short = board(&support::standard_fen("kiwipete"));
    let mut slots = fresh();
    let long_last = generate_legal(&long).iter().last().expect("a legal move");
    let short_last = generate_legal(&short).iter().last().expect("a legal move");
    for (long_tt, short_tt) in [
        (Move::NULL, Move::NULL),
        (long_last, short_last),
        (Move::NULL, short_last),
        (long_last, Move::NULL),
    ] {
        let (list, start) = node_list(&long, long_tt);
        let side = &history[long.side_to_move().index()];
        checked(&long, &list, start, late_killers(&long), side, &mut slots);
        let (list, start) = node_list(&short, short_tt);
        let side = &history[short.side_to_move().index()];
        checked(&short, &list, start, late_killers(&short), side, &mut slots);
    }
}

/// The capture ranked first loses and the one ranked second wins, so both are examined before the
/// first noisy move is yielded, and nothing more by the end.
#[test]
fn a_high_ranked_losing_capture_before_a_winning_one_costs_two_see_calls() {
    let b = board("4k3/8/2p5/3r4/4n3/6N1/8/K2Q4 w - - 0 1");
    let noisy: Vec<Move> = generate_legal(&b).iter().filter(|m| m.is_noisy()).collect();
    assert_eq!(noisy.len(), 2, "{noisy:?}");
    let (high, low) = if noisy_key(&b, noisy[0]) > noisy_key(&b, noisy[1]) {
        (noisy[0], noisy[1])
    } else {
        (noisy[1], noisy[0])
    };
    assert!(
        see(&b, high) < 0 && see(&b, low) >= 0,
        "{high:?} loses and {low:?} wins"
    );
    let mut list = generate_legal(&b);
    let mut slots = fresh();
    let mut picker = Picker::new(&b, &mut list, 0, [Move::NULL; 2], &[], &mut slots);
    let (first, _, _) = picker.next(&b, &mut list, &mut slots).expect("a move");
    assert_eq!((first, picker.exchanges()), (low, 2));
    while picker.next(&b, &mut list, &mut slots).is_some() {}
    assert_eq!(picker.exchanges(), 2, "each noisy move once");
}

/// A start at or past the list's end leaves the list as it was, as the sort did.
#[test]
fn sort_from_leaves_a_list_alone_from_a_start_at_or_past_its_end() {
    let b = board(&support::standard_fen("kiwipete"));
    let legal = generate_legal(&b);
    for start in [legal.len(), legal.len() + 1, legal.len() + 40] {
        let mut list = legal.clone();
        sort_from(&b, &mut list, start, late_killers(&b), &[]);
        assert_eq!(list.as_slice(), legal.as_slice(), "start {start}");
    }
    let mut empty = MoveList::new();
    sort_from(&b, &mut empty, 1, [Move::NULL; 2], &[]);
    assert!(empty.is_empty());
}
