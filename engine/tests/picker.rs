// SPDX-License-Identifier: GPL-3.0-or-later

//! The main search's picker against the sort it replaces: the same moves in the same order, ties
//! included, and an exchange read only for a noisy move about to be tried.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::fen::FenStyle;
use cadence_core::position::Board;
use cadence_core::{Colour, MAX_MOVES, Move, MoveList, generate_legal};
use cadence_engine::history::SPAN;
use cadence_engine::picker::{Picker, Slot, noisy_key};
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
    let mut picker = Picker::new(&list, start, killers, history, &mut slots);
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

/// The exchanges read by each yield are those of the noisy moves the picker has tested to reach
/// it: none for the table's move, every noisy move ranked at or above a winning one when it is
/// yielded, and each noisy move once by the end.
#[test]
fn an_exchange_is_read_only_for_a_noisy_move_about_to_be_tried() {
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
