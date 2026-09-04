// SPDX-License-Identifier: GPL-3.0-or-later

//! The history heuristic: what a quiet move has been worth elsewhere, and
//! what reads it.
//!
//! What these gates demonstrate is the mechanism and not that the code
//! runs. The load-bearing one is
//! [`a_cutoff_raises_a_score_and_the_order_follows`]: a real search plays
//! real cutoffs, the table it leaves behind holds a higher score for the
//! move that cut than for the moves it beat, and sorting the same list
//! through that table puts a different move in front of the search. Score
//! and order are asserted as one chain, because either alone passes on a
//! mechanism that does nothing -- a table nobody reads raises scores
//! forever, and a sort keyed on a table nobody writes reorders nothing.
//!
//! The malus has a gate of its own, since it is the half that produces a
//! negative score and a negative score is the whole of what the reduction
//! reads on the downside; a table that only ever credits would pass every
//! gate here that does not look for one.
//!
//! The debit's own gate is
//! [`a_debit_reaches_only_the_moves_the_node_searched`], and it is the
//! verification that loop had none of: the pruning rules give quiet moves
//! up inside the slice a cutoff debits, and nothing said whether the table
//! was learning from a move that failed or from one nobody played.
//!
//! The rest pin the arithmetic directly, without a search: the bonus, the
//! ageing update's bound and its diminishing return, the shift, and the
//! rule that history adjusts a reduction and never creates one.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Colour, MAX_MOVES, Move, MoveList, START_FEN, generate_legal};
use cadence_engine::history::{self, HISTORY_MAX, History, SHIFT_MAX, SPAN};
use cadence_engine::picker::sort_from;
use cadence_engine::search::{Limits, Search, Searched, history_reduction, lmr_reduction};
use support::table;

/// The depth the ordering gate runs at. Six, like the reduction gates:
/// deep enough that quiet cutoffs are plentiful and the table has been
/// written to well before the last iteration sorts a list.
const ORDER_DEPTH: u32 = 6;

/// The depth the modulation gate runs at, and it is deeper for a reason
/// that is a property of the mechanism rather than of the position.
///
/// The reduction reads the *tails* of the score distribution, deliberately
/// (`history::HISTORY_PLY`), and a tail needs a table that has been written
/// to enough times for one to exist. Measured over the bench positions:
/// at depth six no position in the list moves a reduction in both
/// directions, at depth seven none does either, and at depth eight five do.
/// A gate on a rule that fires on the outer few per cent has to search
/// deep enough to have an outer few per cent.
///
/// **Late move pruning moved it from eight to ten, and what moved is the
/// malus's reader rather than the table.** The direction only the debit can
/// produce -- a reduction the score *lengthens* -- falls from 949,732 to
/// 293,638 over the bench positions at depth twelve, a 69% fall, while the
/// direction a credit produces rises from 110,829 to 166,213. The ratio
/// between the two narrows from 8.6 to 1 down to 1.8 to 1. The mechanism is
/// that a move carrying negative history is a late quiet move that has been
/// refuted, and that is precisely the population this rule gives up before
/// a reduction site is reached, so the malus keeps its writer and loses
/// most of its reader. Measured on this position: no reduction is
/// lengthened at depth eight, and some are at nine. Ten is taken, one past
/// the first depth that works, for `GATE_DEPTH`'s reason in
/// `tests/futility.rs`.
const MODULATION_DEPTH: u32 = 10;

/// A quiet middlegame with both sides developed and no capture worth
/// making, which is where a history table has something to learn: the same
/// quiet moves come up at node after node and the score separates them.
/// Kiwipete is the opposite kind of position and is used above for the
/// opposite reason, that its noisy prefix is long enough for the "the sort
/// did not disturb the other bands" half to mean something.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// The chain the whole change rests on, end to end through a real search:
/// a cutoff raises a score, and the order changes as a result.
///
/// Three links, each asserted, and the first two are what stop the third
/// passing for the wrong reason. The search writes: some quiet move ends
/// the search with a score above zero, so a cutoff credited it. The write
/// is discriminating: the highest-scoring quiet move in the root's own list
/// outscores the lowest, so the table separates quiet moves rather than
/// lifting them all together. And the sort follows: ordering the root's
/// legal list through that table puts a different move at the head of the
/// quiet block than ordering it through no table at all, with the noisy
/// prefix identical between the two, which is what says the score moved a
/// quiet move and did not disturb a band it has no business in.
#[test]
fn a_cutoff_raises_a_score_and_the_order_follows() {
    let fen = support::standard_fen("kiwipete");
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(&fen);
    let mut s = Search::new(Limits::depth(ORDER_DEPTH), &stop, &tt);
    let best = s.run(&mut b, &mut Vec::new());
    assert!(!best.is_null(), "{fen}: no move");
    assert_eq!(
        s.completed_depth(),
        ORDER_DEPTH,
        "the search did not complete depth {ORDER_DEPTH}"
    );

    let us = b.side_to_move();
    let hist = s.history();
    let quiets: Vec<Move> = generate_legal(&b)
        .iter()
        .filter(|m| !m.is_noisy())
        .collect();
    assert!(quiets.len() > 1, "the position needs quiet moves to rank");

    let scores: Vec<i32> = quiets.iter().map(|&m| hist.get(us, m)).collect();
    let high = *scores.iter().max().expect("a quiet move");
    let low = *scores.iter().min().expect("a quiet move");
    assert!(
        high > 0,
        "depth {ORDER_DEPTH} on Kiwipete and no quiet move of the root's own list was ever credited"
    );
    assert!(
        high > low,
        "every quiet move of the root's list scores {high}: the table does not separate them"
    );

    let mut flat = generate_legal(&b);
    let mut ranked = flat.clone();
    sort_from(&b, &mut flat, 0, [Move::NULL; 2], &[]);
    sort_from(&b, &mut ranked, 0, [Move::NULL; 2], hist.side(us));
    let first_quiet = |l: &MoveList| {
        l.iter()
            .position(|m| !m.is_noisy())
            .expect("the list has a quiet move")
    };
    let cut = first_quiet(&flat);
    assert_eq!(
        cut,
        first_quiet(&ranked),
        "the history score moved a move out of the quiet band"
    );
    assert_eq!(
        flat.as_slice()[..cut],
        ranked.as_slice()[..cut],
        "the history score reordered the noisy moves"
    );
    assert_ne!(
        flat.as_slice()[cut],
        ranked.as_slice()[cut],
        "the table separates the quiet moves and the sort put the same one first anyway"
    );
    assert_eq!(
        hist.get(us, ranked.as_slice()[cut]),
        high,
        "the sort did not put the best-scoring quiet move first"
    );
}

/// The malus fires, and both directions reach a reduction that was going
/// to happen.
///
/// The counters are the instrument: one counts a reduction the score
/// shortened, the other one it lengthened, and the second cannot move
/// unless some move at a reduction site carries a negative score, which
/// only the debit on the refuted quiets can produce. A table that credited
/// the cutter and left its siblings alone would pass the first assertion
/// and fail this one, which is why the two are separate counters and not a
/// sum.
#[test]
fn the_malus_reaches_a_reduction_and_so_does_the_bonus() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(MIDDLEGAME);
    let mut s = Search::new(Limits::depth(MODULATION_DEPTH), &stop, &tt);
    let best = s.run(&mut b, &mut Vec::new());
    assert!(!best.is_null(), "no move");
    assert_eq!(
        s.completed_depth(),
        MODULATION_DEPTH,
        "the search did not complete depth {MODULATION_DEPTH}"
    );
    assert!(
        s.lmr_reductions() > 0,
        "nothing was reduced, so nothing could be modulated"
    );
    assert!(
        s.history_reduced_less() > 0,
        "no history score ever shortened a reduction"
    );
    assert!(
        s.history_reduced_more() > 0,
        "no history score ever lengthened a reduction"
    );
}

/// The depth the debit gate runs at, and it is chosen against a
/// measurement rather than from the depths the two pruning rules name.
///
/// Both rules give quiet moves up from depth four on this position, and a
/// debit falls on one of them only from depth eight: measured over root
/// depths four to eleven, the count of debits landing on an unsearched move
/// runs 0, 0, 0, 0, 34, 69, 144, 355. A gate at seven or below would assert
/// the property of a search that never violated it, so eight is the first
/// depth that works and nine is taken, one past it, for `GATE_DEPTH`'s
/// reason in `tests/futility.rs`. At nine the margin gives up 51,228 moves,
/// the count 41,858, and the cutoffs debit 318 quiet moves of which 69 were
/// never searched.
const DEBIT_DEPTH: u32 = 9;

/// **The debit reaches the moves the node searched and no others.**
///
/// A move that was given up before `make_move` produced no evidence, and
/// debiting it is the search learning from a game it did not play. The
/// hazard is that the circuit closes: a debited move sorts lower, a lower
/// rank is a higher index, and a higher index is nearer the count the
/// pruning gives up at, so a move given up once is likelier to be given up
/// again on evidence its own skipping created.
///
/// Three coverage assertions come first, because the property is trivially
/// true of a search that pruned nothing: both rules must have given a quiet
/// move up somewhere, and some cutoff must have debited one, or the fourth
/// assertion passes without having asked anything.
#[test]
fn a_debit_reaches_only_the_moves_the_node_searched() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(MIDDLEGAME);
    let mut s = Search::new(Limits::depth(DEBIT_DEPTH), &stop, &tt);
    let best = s.run(&mut b, &mut Vec::new());
    assert!(!best.is_null(), "no move");

    assert!(
        s.futility_skipped() > 0,
        "the margin gave up no move, so the slice holds nothing it skipped"
    );
    assert!(
        s.lmp_skipped() > 0,
        "the count gave up no move, so the slice holds nothing it skipped"
    );
    assert!(
        s.history_debits() > 0,
        "no quiet cutoff debited anything at depth {DEBIT_DEPTH}"
    );
    assert_eq!(
        s.history_debits_unsearched(),
        0,
        "{} of {} debits fell on a move the node never searched",
        s.history_debits_unsearched(),
        s.history_debits()
    );
}

/// The record is empty until something is written to it, reads back what
/// was written, and keeps its indices apart.
///
/// Idempotence is asserted because the loop visits an index once and a gate
/// should not rest on that, and the two ends are asserted because a word
/// index and a bit index are the two things an implementation gets the
/// wrong way round while every middle case still passes.
#[test]
fn the_record_reads_back_what_was_set() {
    let empty = Searched::new();
    for i in 0..MAX_MOVES {
        assert!(!empty.contains(i), "a fresh record holds {i}");
    }
    assert_eq!(
        empty,
        Searched::default(),
        "default is not the empty record"
    );

    for &i in &[0usize, 1, 63, 64, 65, 127, 128, 191, 192, MAX_MOVES - 1] {
        let mut r = Searched::new();
        r.set(i);
        assert!(r.contains(i), "{i} was set and does not read back");
        r.set(i);
        assert!(r.contains(i), "{i} did not survive being set twice");
        for j in 0..MAX_MOVES {
            assert_eq!(r.contains(j), i == j, "setting {i} moved {j}");
        }
    }

    // Every index at once, which is the state a node that searched its
    // whole list leaves behind.
    let mut all = Searched::new();
    for i in 0..MAX_MOVES {
        all.set(i);
    }
    for i in 0..MAX_MOVES {
        assert!(all.contains(i), "{i} is missing from a full record");
    }
}

/// Every move list a generator can produce is indexable in the record.
///
/// The record is sized for [`MAX_MOVES`] and indexes rather than saturates,
/// so this is `every_generated_move_indexes_inside_the_table`'s argument
/// applied to the other array a cutoff reads: the bound is asserted against
/// real lists rather than hidden behind a clamp that would pass on the day
/// it stopped holding.
#[test]
fn every_generated_list_fits_the_record() {
    let mut widest = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let n = generate_legal(&b).len();
        assert!(n <= MAX_MOVES, "{fen}: {n} moves");
        let mut r = Searched::new();
        for i in 0..n {
            r.set(i);
        }
        widest = widest.max(n);
    }
    assert!(widest > 0, "the corpus produced no moves");
}

/// Every entry the search writes stays inside the table's stated range,
/// measured over a real search rather than over the update function alone:
/// the bound is what `picker`'s band width and the shift's divisor are both
/// sized against, so it is worth checking against the thing that actually
/// writes.
#[test]
fn a_real_search_leaves_every_entry_inside_the_bound() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(&support::standard_fen("kiwipete"));
    let mut s = Search::new(Limits::depth(ORDER_DEPTH), &stop, &tt);
    let _ = s.run(&mut b, &mut Vec::new());
    for side in Colour::ALL {
        let row = s.history().side(side);
        assert_eq!(row.len(), SPAN, "{side:?}: the row is not the whole span");
        for (i, &v) in row.iter().enumerate() {
            assert!(
                v.abs() <= HISTORY_MAX,
                "{side:?} index {i}: {v} is outside the bound"
            );
        }
    }
}

/// The table is one search's and never two, which is the variant that was
/// chosen rather than an accident of the UCI layer.
///
/// Two runs of one `Search` over one position, with the transposition table
/// cleared between them so that it carries nothing either. What a search
/// sees is then the code, the position and an empty table, all three the
/// same both times, and the killers are cleared at the head of `run`. So
/// the history is the only state that could carry, and two identical runs
/// are what says it did not: had the second started from the first's table
/// it would have sorted its quiet moves differently and searched a
/// different tree.
///
/// `bench` cannot make this distinction -- it builds a fresh `Search` per
/// position -- so a gate through one `Search` is the only thing in the
/// repository that can say which lifetime is in force.
#[test]
fn the_table_does_not_survive_a_second_search() {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut b = board(&support::standard_fen("kiwipete"));
    let mut s = Search::new(Limits::depth(ORDER_DEPTH), &stop, &tt);

    let _ = s.run(&mut b, &mut Vec::new());
    let nodes = s.nodes();
    let first: Vec<i32> = Colour::ALL
        .iter()
        .flat_map(|&c| s.history().side(c).to_vec())
        .collect();
    assert!(
        first.iter().any(|&v| v != 0),
        "the first run wrote nothing to compare against"
    );

    tt.clear();
    let _ = s.run(&mut b, &mut Vec::new());
    assert_eq!(
        s.nodes(),
        nodes,
        "the second run of the same search searched a different tree"
    );
    let second: Vec<i32> = Colour::ALL
        .iter()
        .flat_map(|&c| s.history().side(c).to_vec())
        .collect();
    assert_eq!(
        first, second,
        "the second run started from the first's table"
    );
}

/// The bonus is the scaled square of the depth until the cap, and the cap
/// is where that square would leave the table's range.
///
/// The scale is pinned here as well as the shape, because it is the one
/// constant in this module chosen against a measurement rather than argued
/// from the mechanism: at a scale of one the ageing term never engages and
/// the cap is decoration, which is a thing the gate should notice being
/// undone.
#[test]
fn the_bonus_is_the_scaled_square_until_the_cap() {
    assert_eq!(history::bonus(0), 0);
    assert_eq!(history::bonus(1), 16, "the scale moved");
    for depth in 0..=32u32 {
        assert_eq!(
            history::bonus(depth),
            i32::try_from(16 * depth * depth).expect("inside the cap"),
            "depth {depth}"
        );
    }
    assert_eq!(history::bonus(32), HISTORY_MAX, "the cap is not at 32");
    for depth in [33u32, 128, 1_000, 65_535, u32::MAX] {
        assert_eq!(history::bonus(depth), HISTORY_MAX, "depth {depth}");
    }
}

/// The ageing update: bounded for every input, credit raises, debit lowers,
/// and the return diminishes as the entry approaches the cap.
///
/// The last is the property that makes it ageing rather than accumulation,
/// and it is the one an implementation gets wrong by leaving out a term
/// while every other assertion here still passes.
#[test]
fn the_update_is_bounded_and_its_return_diminishes() {
    let extremes = [
        i32::MIN,
        -HISTORY_MAX - 1,
        -HISTORY_MAX,
        -1,
        0,
        1,
        HISTORY_MAX,
        HISTORY_MAX + 1,
        i32::MAX,
    ];
    for e in extremes {
        for b in extremes {
            let v = history::apply(e, b);
            assert!(v.abs() <= HISTORY_MAX, "apply({e}, {b}) = {v}");
        }
    }
    for e in (-HISTORY_MAX..=HISTORY_MAX).step_by(257) {
        assert!(history::apply(e, 64) >= e, "a credit lowered {e}");
        assert!(history::apply(e, -64) <= e, "a debit raised {e}");
    }
    // The same bonus moves an empty entry further than a half-full one.
    let bonus = history::bonus(8);
    let from_empty = history::apply(0, bonus);
    let from_half = history::apply(HISTORY_MAX / 2, bonus) - HISTORY_MAX / 2;
    assert!(
        from_empty > from_half,
        "the credit did not shrink as the entry grew: {from_empty} against {from_half}"
    );
    assert_eq!(
        history::apply(HISTORY_MAX, HISTORY_MAX),
        HISTORY_MAX,
        "an entry at the cap left it"
    );
    assert_eq!(
        history::apply(-HISTORY_MAX, -HISTORY_MAX),
        -HISTORY_MAX,
        "an entry at the floor left it"
    );
    // Repeated credits converge on the cap and never pass it.
    let mut e = 0;
    for _ in 0..10_000 {
        e = history::apply(e, bonus);
    }
    assert!(e > HISTORY_MAX / 2, "credits did not accumulate: {e}");
    assert!(e <= HISTORY_MAX, "credits passed the cap: {e}");
}

/// The shift is odd, bounded and monotone: symmetric about a score of
/// zero, never more than [`SHIFT_MAX`] plies, and never smaller for a
/// larger score.
#[test]
fn the_shift_is_bounded_and_monotone() {
    assert_eq!(history::shift(0), 0, "a move with no score was moved");
    let mut previous = -SHIFT_MAX - 1;
    for h in (-HISTORY_MAX..=HISTORY_MAX).step_by(97) {
        let s = history::shift(h);
        assert!(s.abs() <= SHIFT_MAX, "history {h}: shift {s}");
        assert!(s >= previous, "history {h}: the shift fell to {s}");
        assert_eq!(history::shift(-h), -s, "history {h}: not symmetric");
        previous = s;
    }
    assert_eq!(history::shift(HISTORY_MAX), SHIFT_MAX, "the cap is the top");
    assert_eq!(
        history::shift(-HISTORY_MAX),
        -SHIFT_MAX,
        "the floor is the bottom"
    );
}

/// History adjusts a reduction and never creates one: a base of zero comes
/// back zero for every score there is, so every exemption `reduction` holds
/// survives the table.
#[test]
fn history_never_creates_a_reduction() {
    for h in (-HISTORY_MAX..=HISTORY_MAX).step_by(31) {
        assert_eq!(
            history_reduction(0, h),
            0,
            "history {h} created a reduction"
        );
    }
}

/// Where a reduction was going to happen, the score moves it by the shift
/// and by no more, in both directions, and it cannot drive one below zero.
#[test]
fn history_moves_a_reduction_by_the_shift() {
    for depth in [3u32, 4, 8, 16, 31] {
        for index in [3usize, 4, 8, 16, 32, 64] {
            let base = lmr_reduction(depth, index);
            assert!(base > 0, "depth {depth} index {index}: no base to move");
            for h in (-HISTORY_MAX..=HISTORY_MAX).step_by(97) {
                let r = history_reduction(base, h);
                let want = base.saturating_add_signed(-history::shift(h));
                assert_eq!(r, want, "depth {depth} index {index} history {h}");
                assert!(
                    i64::from(r) <= i64::from(base) + i64::from(SHIFT_MAX),
                    "depth {depth} index {index} history {h}: {r} against {base}"
                );
            }
            assert!(
                history_reduction(base, HISTORY_MAX) <= base,
                "a credited move was reduced more"
            );
            assert!(
                history_reduction(base, -HISTORY_MAX) >= base,
                "a refuted move was reduced less"
            );
        }
    }
}

/// Every move a generator emits indexes inside the table.
///
/// The butterfly index is twelve bits of a sixteen-bit move, and the table
/// is sized for that and not for anything the encoding might grow. A
/// promotion carries its piece in the bits above the index, so the check is
/// worth making against real lists rather than against the mask.
#[test]
fn every_generated_move_indexes_inside_the_table() {
    let mut seen = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        for m in generate_legal(&b).iter() {
            assert!(m.from_to() < SPAN, "{fen}: {m:?} indexes past the table");
            seen += 1;
        }
    }
    assert!(seen > 0, "the corpus produced no moves");
}

/// A table nobody has written to reads zero for every move, which is what
/// makes the empty slice `picker` takes from a caller with none the same
/// order as no table at all.
#[test]
fn a_fresh_table_reads_zero() {
    let h = History::new();
    let b = board(START_FEN);
    for side in Colour::ALL {
        assert!(h.side(side).iter().all(|&v| v == 0));
        for m in generate_legal(&b).iter() {
            assert_eq!(h.get(side, m), 0, "{m:?}");
        }
    }
}
