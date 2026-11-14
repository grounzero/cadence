// SPDX-License-Identifier: GPL-3.0-or-later

//! A cutoff writes the row of the move one or two plies up and the order follows, asserted as one
//! chain, as the butterfly's gate does; the rest hold the frames, the debits and the lifetime.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Move, MoveList, generate_legal, parse_uci};
use cadence_engine::conthist::{self, Frame, OFFSETS, PIECE_TO};
use cadence_engine::history::HISTORY_MAX;
use cadence_engine::picker::{Tables, sort_from};
use cadence_engine::search::{Limits, Search};
use cadence_engine::tt::Table;
use support::table;

/// The butterfly's gate's depth, deep enough that the rows are well written before the last
/// iteration orders a list.
const ORDER_DEPTH: u32 = 6;

/// The butterfly's debit gate's depth, where a node gives up quiet moves the debit must pass over.
const DEBIT_GATE_DEPTH: u32 = 9;

/// Quiet moves recur here, so quiet cutoffs write the rows; on Kiwipete at `ORDER_DEPTH` the
/// cutoffs are captures and the best move's rows stay empty.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The middlegame searched to `ORDER_DEPTH`, and its principal variation.
fn searched<'a>(stop: &'a AtomicBool, tt: &'a Table) -> (Search<'a>, Vec<Move>) {
    let mut b = support::position(MIDDLEGAME);
    let mut s = support::search(Limits::depth(ORDER_DEPTH), stop, tt);
    let best = s.run(&mut b, &mut Vec::new());
    assert!(!best.is_null(), "no move");
    assert_eq!(s.completed_depth(), ORDER_DEPTH, "depth not completed");
    let pv = s.pv().to_vec();
    assert!(pv.len() >= 2, "a principal variation of {} moves", pv.len());
    (s, pv)
}

/// The search writes the row, the row separates the side's quiet moves, and a picker handed it puts
/// the best-scoring first with the noisy prefix unchanged, where one handed none does not.
fn the_order_follows(b: &Board, row: &[i16], offset: usize) {
    let quiets: Vec<Move> = generate_legal(b).iter().filter(|m| !m.is_noisy()).collect();
    assert!(quiets.len() > 1, "offset {offset}: no quiet moves to rank");
    let scores: Vec<i16> = quiets
        .iter()
        .map(|&m| row[conthist::piece_to(b, m)])
        .collect();
    let high = *scores.iter().max().expect("a quiet move");
    let low = *scores.iter().min().expect("a quiet move");
    assert!(high > 0, "offset {offset}: no quiet move was ever credited");
    assert!(
        high > low,
        "offset {offset}: every quiet move scores {high}"
    );

    let mut continuation = [None; OFFSETS.len()];
    continuation[OFFSETS
        .iter()
        .position(|&o| o == offset)
        .expect("an offset")] = Some(row);
    let mut flat = generate_legal(b);
    let mut ranked = flat.clone();
    sort_from(b, &mut flat, 0, [Move::NULL; 2], &Tables::none());
    let view = Tables {
        butterfly: &[],
        continuation,
        capture: &[],
    };
    sort_from(b, &mut ranked, 0, [Move::NULL; 2], &view);
    let first_quiet = |l: &MoveList| l.iter().position(|m| !m.is_noisy()).expect("a quiet move");
    let cut = first_quiet(&flat);
    assert_eq!(cut, first_quiet(&ranked), "offset {offset}: a band moved");
    assert_eq!(
        flat.as_slice()[..cut],
        ranked.as_slice()[..cut],
        "offset {offset}: the noisy moves were reordered"
    );
    assert_ne!(
        flat.as_slice()[cut],
        ranked.as_slice()[cut],
        "offset {offset}: the row separates the quiet moves and the same one came first anyway"
    );
    assert_eq!(
        row[conthist::piece_to(b, ranked.as_slice()[cut])],
        high,
        "offset {offset}: the best-scoring quiet move is not first"
    );
}

#[test]
fn a_cutoff_writes_the_row_of_the_move_before_it_and_the_order_follows() {
    let (stop, tt) = (AtomicBool::new(false), table());
    let (s, pv) = searched(&stop, &tt);
    let mut b = board(MIDDLEGAME);
    let context = conthist::frame(&b, pv[0]).expect("a frame");
    b.make_move(pv[0]);
    the_order_follows(&b, s.continuation().row(context), 1);
}

#[test]
fn a_cutoff_writes_the_row_of_the_move_two_plies_up_and_the_order_follows() {
    let (stop, tt) = (AtomicBool::new(false), table());
    let (s, pv) = searched(&stop, &tt);
    let mut b = board(MIDDLEGAME);
    let context = conthist::frame(&b, pv[0]).expect("a frame");
    b.make_move(pv[0]);
    b.make_move(pv[1]);
    the_order_follows(&b, s.continuation().row(context), 2);
}

/// A null move at ply 1 leaves offset 1 empty at ply 2 and offset 2 the move at the root, and a
/// real search reads some offset with no frame.
#[test]
fn a_null_move_leaves_its_ply_absent_and_the_ply_above_it_present() {
    let b = board(&support::standard_fen("kiwipete"));
    let m = generate_legal(&b).iter().next().expect("a move");
    let root = conthist::frame(&b, m);
    let frames: [Frame; 3] = [root, None, None];
    assert_eq!(
        conthist::context(&frames, 2, 1),
        None,
        "the null move's ply"
    );
    assert_eq!(conthist::context(&frames, 2, 2), root, "the move above it");
    assert_eq!(conthist::context(&frames, 1, 2), None, "above the root");
    assert_eq!(conthist::context(&frames, 0, 1), None, "the root");

    let (stop, tt) = (AtomicBool::new(false), table());
    let (s, _) = searched(&stop, &tt);
    assert!(s.continuation_absent() > 0, "no node read an absent frame");
}

/// A search that gives moves up makes debits in every table; which moves they fall on is read in
/// the entries themselves beside `remember_history`, since the counters filter before they count.
#[test]
fn a_cutoff_debits_only_the_quiet_moves_its_node_searched_in_every_table() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut s = support::search(Limits::depth(DEBIT_GATE_DEPTH), &stop, &tt);
    let mut b = support::position(&support::standard_fen("kiwipete"));
    s.run(&mut b, &mut Vec::new());
    assert!(
        s.continuation_debits() > 100,
        "only {} debits",
        s.continuation_debits()
    );
    assert!(s.continuation_credits() > 0, "no credits");
}

/// With the transposition table cleared, identical runs of one `Search` show neither the table nor
/// the frames carried.
#[test]
fn the_tables_do_not_survive_a_second_search() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = support::position(&support::standard_fen("kiwipete"));
    let mut s = support::search(Limits::depth(ORDER_DEPTH), &stop, &tt);
    let rows = |s: &Search<'_>| -> Vec<i16> {
        (0..PIECE_TO)
            .flat_map(|c| s.continuation().row(c as u16).to_vec())
            .collect()
    };

    let _ = s.run(&mut b, &mut Vec::new());
    let nodes = s.nodes();
    let first = rows(&s);
    assert!(first.iter().any(|&v| v != 0), "the first run wrote nothing");

    tt.clear();
    let _ = s.run(&mut b, &mut Vec::new());
    assert_eq!(s.nodes(), nodes, "the second run searched a different tree");
    assert!(
        first == rows(&s),
        "the second run started from the first's table"
    );
}

/// Every coloured piece and destination the generator produces names a row and a column inside the
/// table, castling's own-rook destination included.
#[test]
fn every_frame_indexes_inside_the_table() {
    let mut seen = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        for m in generate_legal(&b).iter() {
            let i = conthist::piece_to(&b, m);
            assert!(i < PIECE_TO, "{fen}: {m:?} indexes past the table");
            assert_eq!(conthist::frame(&b, m), Some(i as u16), "{fen}: {m:?}");
            seen += 1;
        }
    }
    assert!(seen > 0, "the corpus produced no moves");
}

/// The piece numbered from white's pawn to black's king, times 64, plus the destination from a1: a
/// promotion is keyed by its pawn, and castling by its king onto its own rook in either variant.
#[test]
fn a_move_indexes_its_piece_and_its_destination() {
    let cases = [
        (
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            "g1f3",
            85,
        ),
        (
            "rnbqkbnr/pppppppp/8/8/4P3/8/PPPP1PPP/RNBQKBNR b KQkq - 0 1",
            "g8f6",
            493,
        ),
        ("8/4P3/8/8/8/8/k7/4K3 w - - 0 1", "e7e8q", 60),
        ("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1", "e1h1", 327),
        ("r3k2r/8/8/8/8/8/8/R3K2R b KQkq - 0 1", "e8a8", 760),
        ("rk5r/8/8/8/8/8/8/RK5R w AHah - 0 1", "b1a1", 320),
    ];
    for (fen, uci, index) in cases {
        let b = board(fen);
        let m = parse_uci(&generate_legal(&b), uci).unwrap_or_else(|| panic!("{fen}: no {uci}"));
        assert_eq!(conthist::piece_to(&b, m), index, "{fen}: {uci}");
    }
}

#[test]
fn a_real_search_leaves_every_continuation_entry_inside_the_bound() {
    let (stop, tt) = (AtomicBool::new(false), table());
    let (s, _) = searched(&stop, &tt);
    for c in 0..PIECE_TO {
        for (i, &v) in s.continuation().row(c as u16).iter().enumerate() {
            assert!(
                i32::from(v).abs() <= HISTORY_MAX,
                "row {c} column {i}: {v} is outside the bound"
            );
        }
    }
}
