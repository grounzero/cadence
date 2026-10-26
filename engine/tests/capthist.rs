// SPDX-License-Identifier: GPL-3.0-or-later

//! A cutoff writes a capture's score and the noisy order follows, asserted as one chain; the rest
//! hold the bands, the promotions that capture nothing, the indices, the counts and the lifetime.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::fen::FenStyle;
use cadence_core::position::Board;
use cadence_core::{Move, MoveList, generate_legal, parse_uci};
use cadence_engine::capthist::{self, ENTRIES};
use cadence_engine::history::HISTORY_MAX;
use cadence_engine::picker::{Tables, noisy_key, sort_from};
use cadence_engine::search::{Limits, Search};
use cadence_engine::see::see;
use cadence_engine::tt::Table;
use support::table;

/// The continuation history's gate's depth, deep enough that captures have cut before the last
/// iteration orders a list.
const ORDER_DEPTH: u32 = 6;

/// The butterfly's debit gate's depth.
const DEBIT_GATE_DEPTH: u32 = 9;

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The corpus and every position one move from it, so that promotions, en passant and long capture
/// lists all occur.
fn corpus_and_children() -> Vec<Board> {
    let mut out = Vec::new();
    for fen in support::corpus_fens() {
        let mut b = board(&fen);
        out.push(board(&fen));
        for m in generate_legal(&b).iter() {
            b.make_move(m);
            out.push(board(&b.to_fen(FenStyle::Shredder)));
            b.unmake_move(m);
        }
    }
    out
}

fn with(capture: &[i16]) -> Tables<'_> {
    Tables {
        capture,
        ..Tables::none()
    }
}

/// The search writes the table, the table separates the root's captures, and a picker handed it
/// yields the root's winning noisy moves by their key, in another order than one handed none.
#[test]
fn a_noisy_cutoff_raises_its_score_and_the_noisy_order_follows() {
    let (stop, tt) = (AtomicBool::new(false), table());
    let fen = support::standard_fen("kiwipete");
    let mut s = support::search(Limits::depth(ORDER_DEPTH), &stop, &tt);
    let _ = s.run(&mut support::position(&fen), &mut Vec::new());
    let b = board(&fen);
    let entries = s.capture_history().entries();
    let scores: Vec<i16> = generate_legal(&b)
        .iter()
        .filter_map(|m| capthist::index(&b, m))
        .map(|i| entries[i])
        .collect();
    let high = *scores.iter().max().expect("a capture");
    let low = *scores.iter().min().expect("a capture");
    assert!(high > 0, "no capture of the root's was ever credited");
    assert!(high > low, "every capture of the root's scores {high}");

    let view = with(entries);
    let (mut plain, mut ranked) = (generate_legal(&b), generate_legal(&b));
    sort_from(&b, &mut plain, 0, [Move::NULL; 2], &Tables::none());
    sort_from(&b, &mut ranked, 0, [Move::NULL; 2], &view);
    let winning = |list: &MoveList| -> Vec<Move> {
        list.iter()
            .filter(|&m| m.is_noisy() && see(&b, m) >= 0)
            .collect()
    };
    let (got, unranked) = (winning(&ranked), winning(&plain));
    assert!(got.len() > 1, "{} winning noisy moves to order", got.len());
    assert_ne!(
        got, unranked,
        "the table reordered none of the root's winning noisy moves"
    );
    let keys: Vec<i32> = got.iter().map(|&m| view.noisy_key(&b, m)).collect();
    assert!(
        keys.windows(2).all(|w| w[0] >= w[1]),
        "the winning noisy moves are not by key: {keys:?}"
    );
}

/// The compile-time assertions over real lists, the worst case taken: a queen promotion at the
/// floor still above every capture at the cap, and an underpromotion at the cap below every one at
/// the floor; and in the picker's order the bands hold at the cap and at the floor.
#[test]
fn a_score_moves_no_noisy_move_across_its_band() {
    let cap = vec![HISTORY_MAX as i16; ENTRIES];
    let floor = vec![-HISTORY_MAX as i16; ENTRIES];
    let (high, low) = (with(&cap), with(&floor));
    let mut promotions = 0;
    for b in corpus_and_children() {
        let legal = generate_legal(&b);
        let captures = || {
            legal
                .iter()
                .filter(|m| m.is_capture() && m.promotion_piece().is_none())
        };
        for m in legal.iter().filter(|m| m.promotion_piece().is_some()) {
            promotions += 1;
            for c in captures() {
                if noisy_key(&b, m) > 0 {
                    assert!(
                        low.noisy_key(&b, m) > high.noisy_key(&b, c),
                        "{m:?} under {c:?}"
                    );
                } else {
                    assert!(
                        high.noisy_key(&b, m) < low.noisy_key(&b, c),
                        "{m:?} over {c:?}"
                    );
                }
            }
        }
        let winning = |m: Move| m.is_noisy() && see(&b, m) >= 0;
        let losing = |m: Move| m.is_capture() && m.promotion_piece().is_none() && see(&b, m) < 0;
        for view in [&high, &low] {
            let mut list = legal.clone();
            sort_from(&b, &mut list, 0, [Move::NULL; 2], view);
            let moves = list.as_slice();
            let rest = moves
                .iter()
                .position(|&m| !winning(m))
                .unwrap_or(moves.len());
            assert!(
                !moves[rest..].iter().any(|&m| winning(m)),
                "a winning noisy move left behind"
            );
            if let Some(l) = moves.iter().position(|&m| losing(m)) {
                assert!(
                    moves[l..].iter().all(|&m| losing(m)),
                    "a move among the losing captures"
                );
            }
        }
    }
    assert!(promotions > 60, "only {promotions} promotions read");
}

/// With every entry at the cap and then at the floor, each promotion that captures nothing keys at
/// its MVV-LVA key on the noisy scale, one victim class to the history's cap, and nothing else.
#[test]
fn a_promotion_that_captures_nothing_keeps_its_key() {
    let cap = vec![HISTORY_MAX as i16; ENTRIES];
    let floor = vec![-HISTORY_MAX as i16; ENTRIES];
    let mut read = 0;
    for b in corpus_and_children() {
        for m in generate_legal(&b).iter() {
            if m.promotion_piece().is_none() || m.is_capture() {
                continue;
            }
            let want = noisy_key(&b, m) * (HISTORY_MAX / 8);
            for view in [with(&cap), with(&floor), Tables::none()] {
                assert_eq!(view.noisy_key(&b, m), want, "{m:?}");
            }
            read += 1;
        }
    }
    assert!(read > 30, "only {read} promotions that capture nothing");
}

/// The mover numbered from white's pawn to black's king, times 64, plus the destination from a1,
/// times six, plus the victim from the pawn: en passant takes a pawn and a promotion moves its pawn.
#[test]
fn a_capture_indexes_its_mover_its_destination_and_its_victim() {
    let cases = [
        ("4k3/8/8/4p3/8/5N2/8/4K3 w - - 0 1", "f3e5", Some(600)),
        ("q3k3/8/8/8/8/8/8/R3K3 b - - 0 1", "a8a1", Some(3843)),
        ("4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 1", "e5d6", Some(258)),
        ("4k3/8/8/8/8/8/1p6/R3K3 b - - 0 1", "b2a1q", Some(2307)),
        ("8/4P3/8/8/8/8/k7/4K3 w - - 0 1", "e7e8q", None),
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            "g1f3",
            None,
        ),
    ];
    for (fen, uci, index) in cases {
        let b = board(fen);
        let m = parse_uci(&generate_legal(&b), uci).unwrap_or_else(|| panic!("{fen}: no {uci}"));
        assert_eq!(capthist::index(&b, m), index, "{fen}: {uci}");
    }
}

/// Every capture the generator produces names an entry inside the table and every other move none,
/// en passant and promotions included.
#[test]
fn every_noisy_move_indexes_inside_the_table() {
    let (mut en_passant, mut promotions) = (0, 0);
    for b in corpus_and_children() {
        for m in generate_legal(&b).iter() {
            match capthist::index(&b, m) {
                Some(i) => assert!(i < ENTRIES, "{m:?} indexes past the table"),
                None => assert!(!m.is_capture(), "{m:?} captures and has no entry"),
            }
            en_passant += usize::from(m.is_en_passant());
            promotions += usize::from(m.is_capture() && m.promotion_piece().is_some());
        }
    }
    assert!(en_passant > 0, "no en passant capture read");
    assert!(promotions > 0, "no capturing promotion read");
}

/// Which captures the debits fall on is read in the entries beside `remember_history`; this holds
/// that a real search makes both kinds of write.
#[test]
fn a_search_credits_and_debits_the_capture_history() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut s = support::search(Limits::depth(DEBIT_GATE_DEPTH), &stop, &tt);
    let mut b = support::position(&support::standard_fen("kiwipete"));
    s.run(&mut b, &mut Vec::new());
    assert!(
        s.capture_debits() > 100,
        "only {} debits",
        s.capture_debits()
    );
    assert!(s.capture_credits() > 0, "no credits");
}

/// With the transposition table cleared, identical runs of one `Search` show the table not carried.
#[test]
fn the_capture_history_does_not_survive_a_second_search() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(&support::standard_fen("kiwipete"));
    let mut s = support::search(Limits::depth(ORDER_DEPTH), &stop, &tt);
    let entries = |s: &Search<'_>| s.capture_history().entries().to_vec();

    let _ = s.run(&mut b, &mut Vec::new());
    let nodes = s.nodes();
    let first = entries(&s);
    assert!(first.iter().any(|&v| v != 0), "the first run wrote nothing");

    tt.clear();
    let _ = s.run(&mut b, &mut Vec::new());
    assert_eq!(s.nodes(), nodes, "the second run searched a different tree");
    assert!(
        first == entries(&s),
        "the second run started from the first's table"
    );
}

#[test]
fn a_real_search_leaves_every_capture_entry_inside_the_bound() {
    let stop = AtomicBool::new(false);
    let tt: Table = table();
    let mut s = support::search(Limits::depth(DEBIT_GATE_DEPTH), &stop, &tt);
    let _ = s.run(
        &mut support::position(&support::standard_fen("kiwipete")),
        &mut Vec::new(),
    );
    for (i, &v) in s.capture_history().entries().iter().enumerate() {
        assert!(
            i32::from(v).abs() <= HISTORY_MAX,
            "entry {i}: {v} is outside the bound"
        );
    }
}
