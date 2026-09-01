// SPDX-License-Identifier: GPL-3.0-or-later

//! Aspiration windows: an iteration opens around the last one's score
//! rather than at the full window, and runs again wider where the answer
//! falls outside it.
//!
//! **What these gates demonstrate is a narrow window used and a re-search
//! triggered by a fail.** That is two claims and neither of them is "the
//! code runs". The first is behavioural and does not read a counter at
//! all: one position, one depth, the root asked the same question twice,
//! once at the full window and once at a window that brackets the answer,
//! and the narrow one comes back with the same move and the same score
//! over a smaller tree. The second is a search of a real position whose
//! score moves further between two iterations than the width allows, so a
//! window **must** be wrong in both directions before the search finishes,
//! and the gate asserts both directions fired.
//!
//! **The correctness argument decomposes into two halves and each has its
//! own gate**, which is why no gate here asserts that an aspirated search
//! agrees with a full-window one. It does not, and it may not be made to:
//! a narrower window is a different question, null-move pruning and the
//! history heuristic both read the answer, and `tests/search.rs` already
//! records where window agreement survives and where it stopped. What
//! holds is that a window bracketing the value agrees about the value
//! (`a_root_window_that_brackets_the_value_returns_the_value`) and that the
//! widening reaches a bracketing window from any answer a search can return
//! (`the_widening_terminates_from_any_score`). Together those say nothing
//! is trusted that was not searched in a window with room for it.
//!
//! **Three of these gates fail on the seam commit**, where the window
//! arithmetic and the counters exist and nothing calls them: the three at
//! the end, which are the three that watch the search choose its own
//! window. The two root gates above them pass there, because they hand the
//! root a window themselves through the seam rather than waiting for the
//! search to choose one, and that difference is the seam's whole purpose.
//!
//! The counters are written wherever the rule runs and read on no decision
//! path, so a depth-limited search here reads no clock and every assertion
//! is exact rather than statistical.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{START_FEN, generate_legal};
use cadence_engine::score::{INFINITE, MATE, MATE_IN_MAX_PLY, MAX_EVAL, Score, mate_in, mated_in};
use cadence_engine::search::{
    ASPIRATION_DELTA, ASPIRATION_DEPTH, Limits, Search, aspiration_rewiden, aspiration_window,
};
use cadence_engine::tt::Table;
use support::table;

/// Kiwipete, whose score moves further between two iterations than
/// [`ASPIRATION_DELTA`] allows, in both directions and inside one search.
///
/// **Measured on the champion rather than chosen.** Over the thirty-nine
/// bench positions at depth twelve it is the only one whose score both
/// falls and rises by more than the width inside one search: `-22` at depth
/// nine to `-86` at ten, which no window of twenty-four centipawns centred
/// on `-22` contains, and back to `-24` at eleven, which none centred on
/// `-86` contains either. That is what makes one search of one position
/// exercise both branches of [`aspiration_rewiden`], and it is why
/// [`FAIL_DEPTH`] is eleven and not less.
const FAIL_POSITION: &str = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

/// The depth [`FAIL_POSITION`] is searched to, and the first depth at which
/// both directions have fired.
///
/// The fall lands at depth ten and the rise at eleven, so ten is not
/// enough. It is not taken one ply past the first depth that works, the way
/// `tests/reverse_futility.rs` takes its coverage depth: this gate asserts
/// that a specific pair of movements produced a specific pair of
/// re-searches, so a depth past the second one is a depth at which a third
/// movement can arrive and change what is being asserted.
const FAIL_DEPTH: u32 = 11;

/// A position the search scores as a mate well before the depth these gates
/// run to: king and queen against king and rook, mate in three.
///
/// It is a bench position, and the mate is what makes it one here: from the
/// iteration that finds it, the score the next window would be centred on
/// is on the mate scale, and [`aspiration_window`] has to refuse. A
/// position that never scores a mate cannot tell a rule that refuses one
/// from a rule that never met one.
const MATE_POSITION: &str = "3k4/8/3K4/8/8/8/8/3Q1r2 w - - 0 1";

/// The depth [`MATE_POSITION`] is searched to. The mate is found at eight;
/// nine is taken so that at least one iteration is refused a window on the
/// strength of it.
const MATE_DEPTH: u32 = 9;

fn board(fen: &str) -> Board {
    Board::from_fen(fen).expect("a legal position")
}

fn no_table() -> Table {
    Table::with_buckets(0).expect("a table of no buckets")
}

// ---------------------------------------------------------------------------
// The window, as arithmetic
// ---------------------------------------------------------------------------

/// The window is the width either side of the score it is centred on, and
/// nothing else decides it.
#[test]
fn the_window_is_the_delta_either_side_of_the_previous_score() {
    for prev in [-2_000, -301, -24, -1, 0, 1, 24, 301, 2_000] {
        let (alpha, beta) =
            aspiration_window(ASPIRATION_DEPTH, Some(prev)).expect("a window at the floor");
        assert_eq!(alpha, prev - ASPIRATION_DELTA, "alpha at prev {prev}");
        assert_eq!(beta, prev + ASPIRATION_DELTA, "beta at prev {prev}");
        assert_eq!(beta - alpha, 2 * ASPIRATION_DELTA, "width at prev {prev}");
    }
}

/// The first iteration has no previous score, and the absence is what
/// refuses it rather than a condition anybody has to remember.
#[test]
fn no_previous_score_leaves_the_window_full() {
    for depth in 1..=32 {
        assert_eq!(
            aspiration_window(depth, None),
            None,
            "depth {depth} narrowed a window with no score to centre it on"
        );
    }
}

/// The floor decides, and it is the first depth that narrows.
#[test]
fn the_depth_floor_decides_and_is_the_first_depth_that_narrows() {
    for depth in 1..ASPIRATION_DEPTH {
        assert_eq!(
            aspiration_window(depth, Some(0)),
            None,
            "depth {depth} is below the floor and narrowed anyway"
        );
    }
    for depth in ASPIRATION_DEPTH..=64 {
        assert!(
            aspiration_window(depth, Some(0)).is_some(),
            "depth {depth} is at or above the floor and did not narrow"
        );
    }
}

/// A mate score is not a quantity a centipawn width is commensurable with,
/// at any distance the scale allows and in either direction.
#[test]
fn a_mate_scale_previous_score_leaves_the_window_full() {
    for ply in 0..=256 {
        for prev in [mate_in(ply), mated_in(ply)] {
            assert_eq!(
                aspiration_window(ASPIRATION_DEPTH, Some(prev)),
                None,
                "a window was opened around the mate score {prev}"
            );
        }
    }
    // And the boundary is the scale's own, not a number written twice.
    assert_eq!(
        aspiration_window(ASPIRATION_DEPTH, Some(MATE_IN_MAX_PLY)),
        None
    );
    assert!(aspiration_window(ASPIRATION_DEPTH, Some(MATE_IN_MAX_PLY - 1)).is_some());
}

// ---------------------------------------------------------------------------
// The widening, as arithmetic
// ---------------------------------------------------------------------------

/// A score the window contains asks for nothing.
#[test]
fn a_score_inside_the_window_asks_for_no_re_search() {
    let window = (-24, 24);
    for score in -23..=23 {
        assert_eq!(
            aspiration_rewiden(window, score, ASPIRATION_DELTA),
            None,
            "score {score} inside {window:?} asked for a re-search"
        );
    }
}

/// A fail low moves alpha down and leaves beta where it was: the bound that
/// was not contradicted is still a bound.
#[test]
fn a_fail_low_widens_downward_and_keeps_beta() {
    let (alpha, beta) = (-24, 24);
    for score in [-24, -30, -100, -500] {
        let ((a, b), wider) = aspiration_rewiden((alpha, beta), score, ASPIRATION_DELTA)
            .expect("a score at or below alpha fails low");
        assert_eq!(b, beta, "beta moved on a fail low at score {score}");
        assert_eq!(wider, 2 * ASPIRATION_DELTA);
        assert_eq!(a, score - wider, "alpha is placed against the score");
        assert!(a < alpha, "the window did not widen at score {score}");
    }
}

/// A fail high moves beta up and leaves alpha where it was.
#[test]
fn a_fail_high_widens_upward_and_keeps_alpha() {
    let (alpha, beta) = (-24, 24);
    for score in [24, 30, 100, 500] {
        let ((a, b), wider) = aspiration_rewiden((alpha, beta), score, ASPIRATION_DELTA)
            .expect("a score at or above beta fails high");
        assert_eq!(a, alpha, "alpha moved on a fail high at score {score}");
        assert_eq!(wider, 2 * ASPIRATION_DELTA);
        assert_eq!(b, score + wider, "beta is placed against the score");
        assert!(b > beta, "the window did not widen at score {score}");
    }
}

/// A mate returned inside a window abandons the failing bound outright, and
/// only that one: the width is not a distance on the mate scale, and the
/// other bound is still a bound.
#[test]
fn a_mate_returned_inside_a_window_abandons_the_failing_bound_outright() {
    let window = (-24, 24);
    for ply in 0..=256 {
        let ((a, b), _) = aspiration_rewiden(window, mate_in(ply), ASPIRATION_DELTA)
            .expect("a mate above beta fails high");
        assert_eq!(b, INFINITE, "a mate in {ply} widened beta by a margin");
        assert_eq!(a, window.0, "a mate in {ply} moved alpha as well");

        let ((a, b), _) = aspiration_rewiden(window, mated_in(ply), ASPIRATION_DELTA)
            .expect("a mate below alpha fails low");
        assert_eq!(
            a, -INFINITE,
            "being mated in {ply} widened alpha by a margin"
        );
        assert_eq!(b, window.1, "being mated in {ply} moved beta as well");
    }
}

/// The widening doubles, moves only outward, and stops -- which is what
/// bounds how many times one iteration can be searched.
///
/// **And where it stops is the whole of the correctness claim.** For every
/// score a search can return it stops with the score strictly inside the
/// window, so nothing is ever taken as a value from a window with no room
/// for it. `INFINITE` is not one of those scores: it is the sentinel the
/// window itself is built from and the value an aborted root reports, and
/// there the widening stops with the failing bound at the full one, which
/// is as far out as a bound goes. Both endings are asserted, because a
/// widening that stopped early would look like the second while being the
/// first.
#[test]
fn the_widening_terminates_from_any_score() {
    // Across the evaluation's own range, at both ends of the mate scale,
    // and at the sentinel, so a widening that only works in one direction
    // fails here.
    let scores: Vec<Score> = (-MAX_EVAL..=MAX_EVAL)
        .step_by(97)
        .chain([-INFINITE, INFINITE, mate_in(0), mated_in(0), MATE, -MATE, 0])
        .collect();
    for &score in &scores {
        for centre in [-1_000, 0, 1_000] {
            let mut window =
                aspiration_window(ASPIRATION_DEPTH, Some(centre)).expect("a window at the floor");
            let mut delta = ASPIRATION_DELTA;
            let mut passes = 0;
            // The same score every time is the adversarial case: a search
            // that keeps insisting on an answer the window keeps refusing.
            while let Some((next, wider)) = aspiration_rewiden(window, score, delta) {
                assert!(
                    next.0 <= window.0 && next.1 >= window.1,
                    "the window narrowed: {window:?} -> {next:?}"
                );
                assert!(next != window, "the window did not move: {window:?}");
                assert_eq!(wider, delta * 2, "the width did not double");
                window = next;
                delta = wider;
                passes += 1;
                assert!(
                    passes < 32,
                    "score {score} against centre {centre} did not resolve"
                );
            }
            if score.abs() <= MATE {
                assert!(
                    score > window.0 && score < window.1,
                    "score {score} is not inside the window it resolved to, {window:?}"
                );
            } else if score < 0 {
                assert_eq!(
                    window.0, -INFINITE,
                    "{score} stopped short of the full bound"
                );
            } else {
                assert_eq!(
                    window.1, INFINITE,
                    "{score} stopped short of the full bound"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The root, searched
// ---------------------------------------------------------------------------

/// A root move list, in the order the root would take them.
fn root_moves(board: &Board) -> Vec<cadence_core::Move> {
    generate_legal(board).iter().collect()
}

/// A window that brackets the value returns that value, and the move that
/// holds it.
///
/// **This is the property a narrowed root window rests on**, and it is the
/// root's half of `tests/search.rs`'s gate of the same name. It is scoped
/// the way that one is and for the same reasons: past depth one, null-move
/// pruning reads beta and the history heuristic is written from cutoffs, so
/// what a node returns depends on the window it was asked in and the two
/// sides legitimately diverge. Depth one is where no node below the root
/// exists for either to fire at.
///
/// With no table, deliberately: an entry one of these searches stored and
/// the next read would let the narrow window answer from the wide one's
/// work, which is a legitimate thing for a table to do and would make this
/// gate agree with itself for the wrong reason.
#[test]
fn a_root_window_that_brackets_the_value_returns_the_value() {
    let stop = AtomicBool::new(false);
    for fen in [START_FEN, FAIL_POSITION, MATE_POSITION] {
        let mut b = board(fen);
        let moves = root_moves(&b);
        let tt = no_table();
        let (full_move, full) = Search::new(Limits::default(), &stop, &tt)
            .root_window(&mut b, &moves, 1, -INFINITE, INFINITE);
        for window in [(full - 1, full + 1), (full - 1, full), (full, full + 1)] {
            let (m, score) = Search::new(Limits::default(), &stop, &tt)
                .root_window(&mut b, &moves, 1, window.0, window.1);
            assert_eq!(
                score, full,
                "{fen}: the window {window:?} did not agree with the full one about {full}"
            );
            assert_eq!(
                m, full_move,
                "{fen}: the window {window:?} changed the move"
            );
        }
    }
}

/// A window that does not contain the value comes back as a bound on the
/// side it failed, which is what the widening is handed.
#[test]
fn a_root_window_off_the_value_fails_toward_it() {
    let stop = AtomicBool::new(false);
    for fen in [START_FEN, FAIL_POSITION] {
        let mut b = board(fen);
        let moves = root_moves(&b);
        let tt = no_table();
        let (_, full) = Search::new(Limits::default(), &stop, &tt)
            .root_window(&mut b, &moves, 1, -INFINITE, INFINITE);

        let below = (full - 200, full - 100);
        let (_, high) = Search::new(Limits::default(), &stop, &tt)
            .root_window(&mut b, &moves, 1, below.0, below.1);
        assert!(
            high >= below.1,
            "{fen}: the window {below:?} sits below {full} and did not fail high"
        );
        assert_eq!(
            aspiration_rewiden(below, high, ASPIRATION_DELTA).map(|(w, _)| w.0),
            Some(below.0),
            "{fen}: the fail high moved alpha"
        );

        let above = (full + 100, full + 200);
        let (_, low) = Search::new(Limits::default(), &stop, &tt)
            .root_window(&mut b, &moves, 1, above.0, above.1);
        assert!(
            low <= above.0,
            "{fen}: the window {above:?} sits above {full} and did not fail low"
        );
        assert_eq!(
            aspiration_rewiden(above, low, ASPIRATION_DELTA).map(|(w, _)| w.1),
            Some(above.1),
            "{fen}: the fail low moved beta"
        );
    }
}

/// The narrow window is not merely accepted, it is cheaper: the root asked
/// the same question inside a window that brackets the answer searches
/// fewer nodes than the root asked at the full one.
///
/// **This is the gate that says a narrow window is used rather than
/// tolerated**, and it reads no counter: a build that computed a window and
/// then searched at the full one passes every arithmetic gate above and
/// fails this.
#[test]
fn the_narrow_root_window_saves_nodes() {
    let stop = AtomicBool::new(false);
    let mut b = board(FAIL_POSITION);
    let moves = root_moves(&b);
    let depth = 8;

    let tt = table();
    let mut wide = Search::new(Limits::default(), &stop, &tt);
    let (_, full) = wide.root_window(&mut b, &moves, depth, -INFINITE, INFINITE);
    let wide_nodes = wide.nodes();

    let tt = table();
    let mut narrow = Search::new(Limits::default(), &stop, &tt);
    let (_, score) = narrow.root_window(
        &mut b,
        &moves,
        depth,
        full - ASPIRATION_DELTA,
        full + ASPIRATION_DELTA,
    );
    let narrow_nodes = narrow.nodes();

    assert!(
        score > full - ASPIRATION_DELTA && score < full + ASPIRATION_DELTA,
        "the window did not bracket the answer: {score} against {full}"
    );
    assert!(
        narrow_nodes < wide_nodes,
        "the narrow window searched {narrow_nodes} nodes against the full window's {wide_nodes}"
    );
}

// ---------------------------------------------------------------------------
// The search, choosing its own windows
// ---------------------------------------------------------------------------

fn search_to(fen: &str, depth: u32) -> Search<'static> {
    // Leaked so the search outlives this frame and the gate can read its
    // counters; a test binary is the one place that costs nothing.
    let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let tt: &'static Table = Box::leak(Box::new(table()));
    let mut b = board(fen);
    let mut s = Search::new(Limits::depth(depth), stop, tt);
    let _ = s.run(&mut b, &mut std::io::sink());
    s
}

/// The search narrows from the floor and not before, and the count is
/// exact: one window per iteration from [`ASPIRATION_DEPTH`] up.
#[test]
fn the_search_narrows_from_the_depth_floor_and_not_before() {
    for depth in 1..ASPIRATION_DEPTH {
        let s = search_to(START_FEN, depth);
        assert_eq!(
            s.aspiration_windows(),
            0,
            "a search to depth {depth} opened a window below the floor"
        );
    }
    for depth in ASPIRATION_DEPTH..=8 {
        let s = search_to(START_FEN, depth);
        assert_eq!(
            s.aspiration_windows(),
            u64::from(depth - ASPIRATION_DEPTH + 1),
            "a search to depth {depth} did not open one window per iteration from the floor"
        );
    }
}

/// A fail triggers a re-search, in both directions, inside one search of
/// one position.
///
/// **This is the gate the item was asked for.** [`FAIL_POSITION`]'s score
/// falls by 64 centipawns at one iteration and rises by 62 at the next,
/// both further than [`ASPIRATION_DELTA`], so a window centred on the
/// previous score is wrong first below and then above. A build that opens
/// a window and never re-searches returns a bound as a value here; a build
/// that widens in one direction only fails one of the two assertions.
#[test]
fn a_fail_triggers_a_re_search_in_both_directions() {
    let s = search_to(FAIL_POSITION, FAIL_DEPTH);
    assert!(
        s.aspiration_fail_low() > 0,
        "no window was wrong below, over {} opened",
        s.aspiration_windows()
    );
    assert!(
        s.aspiration_fail_high() > 0,
        "no window was wrong above, over {} opened",
        s.aspiration_windows()
    );
}

/// The mate refusal decides, rather than never coming up.
#[test]
fn the_mate_refusal_decides() {
    let s = search_to(MATE_POSITION, MATE_DEPTH);
    assert!(
        s.aspiration_refused_by_mate() > 0,
        "no iteration was refused a window by a mate score, over {} opened",
        s.aspiration_windows()
    );
    // And the refusal is the only reason a window was not opened here: the
    // iterations from the floor up are the windows plus the refusals.
    assert_eq!(
        s.aspiration_windows() + s.aspiration_refused_by_mate(),
        u64::from(MATE_DEPTH - ASPIRATION_DEPTH + 1),
        "the iterations from the floor are not the windows plus the refusals"
    );
}
