// SPDX-License-Identifier: GPL-3.0-or-later

//! No search gate can say a move is good; these pin what needs no opponent: deepening, determinism
//! to the node, mate distances, draws, limits and the `info` lines. Mate keys are checked by a
//! brute-force verifier here, so a wrong expected key fails this file rather than the engine.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Colour, MAX_PLY, Move, START_FEN, generate_legal, parse_uci, to_uci};
use cadence_engine::eval;
use cadence_engine::position::Position;
use cadence_engine::score::{self, DRAW, MATE, Score, mate_in, mated_in};
use cadence_engine::search::{Limits, extension};
use cadence_engine::tt::Table;
use support::{Outcome, Rng, play_game, random_mover, table};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Result {
    best: Move,
    score: Score,
    nodes: u64,
    depth: u32,
    pv: Vec<Move>,
}

fn search(board: &mut Position, limits: Limits) -> Result {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut sink = Vec::new();
    let mut s = support::search(limits, &stop, &tt);
    let best = s.run(board, &mut sink);
    Result {
        best,
        score: s.score(),
        nodes: s.nodes(),
        depth: s.completed_depth(),
        pv: s.pv().to_vec(),
    }
}

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

fn mv(board: &Board, uci: &str) -> Move {
    parse_uci(&generate_legal(board), uci)
        .unwrap_or_else(|| panic!("{uci} is not legal in {board:?}"))
}

/// The standard suite, four DFRC arrays and four castling-legality positions.
fn sample() -> Vec<String> {
    let mut out = support::standard_fens();
    out.extend(
        support::dfrc_arrays()
            .into_iter()
            .take(4)
            .map(|(_, _, f)| f),
    );
    out.extend(support::castling_fens().into_iter().take(4));
    out
}

// ---------------------------------------------------------------------------
// Deepening and determinism
// ---------------------------------------------------------------------------

#[test]
fn deepening_searches_more_and_reports_the_depth_reached() {
    for fen in [
        START_FEN.to_string(),
        support::standard_fen("kiwipete"),
        support::dfrc_arrays()[3].2.clone(),
    ] {
        let mut prev = 0;
        for depth in 1..=4 {
            let r = search(&mut support::position(&fen), Limits::depth(depth));
            assert_eq!(r.depth, depth, "{fen} depth {depth}: reported {}", r.depth);
            assert!(
                r.nodes > prev,
                "{fen} depth {depth}: {} nodes after {prev}",
                r.nodes
            );
            assert!(!r.best.is_null(), "{fen} depth {depth}: no move");
            assert!(
                !r.pv.is_empty() && r.pv[0] == r.best,
                "{fen} depth {depth}: pv {:?}",
                r.pv
            );
            prev = r.nodes;
        }
    }
}

/// In a different order of positions the second time, so nothing carried between searches could
/// hide.
#[test]
fn the_same_position_and_depth_give_the_same_move_score_and_node_count() {
    let fens = sample();
    let first: Vec<Result> = fens
        .iter()
        .map(|f| search(&mut support::position(f), Limits::depth(3)))
        .collect();
    let mut again: Vec<Option<Result>> = vec![None; fens.len()];
    for (i, f) in fens.iter().enumerate().rev() {
        again[i] = Some(search(&mut support::position(f), Limits::depth(3)));
    }
    for (i, f) in fens.iter().enumerate() {
        assert_eq!(Some(&first[i]), again[i].as_ref(), "{f}");
    }
    // And the same board searched twice in a row.
    let mut b = support::position(&support::standard_fen("kiwipete"));
    let a = search(&mut b, Limits::depth(4));
    let c = search(&mut b, Limits::depth(4));
    assert_eq!(a, c);
    assert_eq!(b.ply(), 0, "the search left moves on the stack");
}

#[test]
fn the_binary_gives_the_same_answer_in_two_processes() {
    let fen = support::standard_fen("kiwipete");
    let position = format!("position fen {fen}");
    let a = support::Engine::go(&[&position], "go depth 4").join("\n");
    let b = support::Engine::go(&[&position], "go depth 4").join("\n");
    assert_eq!(support::bestmove(&a), support::bestmove(&b));
    let nodes = |out: &str| -> u64 {
        let line = out
            .lines()
            .rfind(|l| l.starts_with("info depth 4 "))
            .unwrap_or_else(|| panic!("no `info depth 4` in {out:?}"));
        let mut it = line.split_whitespace();
        while let Some(tok) = it.next() {
            if tok == "nodes" {
                return it.next().and_then(|n| n.parse().ok()).expect("nodes value");
            }
        }
        panic!("no nodes on {line}");
    };
    assert_eq!(nodes(&a), nodes(&b));
    assert!(
        nodes(&a) > 1000,
        "{} nodes at depth 4 is not a search",
        nodes(&a)
    );
}

// ---------------------------------------------------------------------------
// Mates
// ---------------------------------------------------------------------------

fn is_mated(b: &Board) -> bool {
    b.in_check() && generate_legal(b).is_empty()
}

fn mates_in_one(b: &mut Position) -> Vec<Move> {
    let mut out = Vec::new();
    for m in generate_legal(b).iter() {
        b.make_move(m);
        if is_mated(b) {
            out.push(m);
        }
        b.unmake_move(m);
    }
    out
}

/// Brute force from `generate_legal` alone, three plies, sharing nothing with the search.
fn mates_in_two(b: &mut Position) -> Vec<Move> {
    let mut out = Vec::new();
    for m in generate_legal(b).iter() {
        b.make_move(m);
        let replies = generate_legal(b);
        let forced = !replies.is_empty()
            && replies.iter().all(|r| {
                b.make_move(r);
                let mated = !mates_in_one(b).is_empty();
                b.unmake_move(r);
                mated
            });
        b.unmake_move(m);
        if forced {
            out.push(m);
        }
    }
    out
}

/// Each key is verified by the brute force above before the engine is asked: a mate in two, and no
/// mate in one.
const MATES_IN_TWO: &[(&str, &str)] = &[
    // Two rooks, the ladder: Rb7+ and Ra8#.
    ("8/7k/R7/8/8/8/8/1R4K1 w - - 0 1", "b1b7"),
    // Queen and king: the quiet Qa7, then Qg7#.
    ("7k/8/5K2/8/8/8/8/Q7 w - - 0 1", "a1a7"),
    // Queen and bishop against a castled king: Qxf7+ Kh8 Qxe8#. Without the rook on e8, Qd8# mates
    // in one through the vacated g8.
    ("4r1k1/5ppp/8/3Q4/2B5/8/8/6K1 w - - 0 1", "d5f7"),
    // The same three, for Black, by mirror.
    ("1r4k1/8/8/8/8/r7/7K/8 b - - 0 1", "b8b2"),
    ("q7/8/8/8/8/5k2/8/7K b - - 0 1", "a8a2"),
    ("6k1/8/8/2b5/3q4/8/5PPP/4R1K1 b - - 0 1", "d4f2"),
];

#[test]
fn the_mate_in_two_positions_are_what_they_claim() {
    for (fen, key) in MATES_IN_TWO {
        let mut b = support::position(fen);
        let key = mv(&b, key);
        assert!(mates_in_one(&mut b).is_empty(), "{fen} has a mate in one");
        let keys = mates_in_two(&mut b);
        assert!(
            keys.contains(&key),
            "{fen}: {key:?} is not a mate in two; these are: {keys:?}"
        );
    }
}

#[test]
fn mate_in_two_is_found_with_the_score_mate_2() {
    for (fen, _) in MATES_IN_TWO {
        let mut b = support::position(fen);
        let keys = mates_in_two(&mut b);
        // Depth 4: mate in two is three plies, and a leaf does not generate the mated side's moves.
        let r = search(&mut b, Limits::depth(4));
        assert!(
            keys.contains(&r.best),
            "{fen}: played {:?}, mates in two are {keys:?}",
            r.best
        );
        assert_eq!(r.score, mate_in(3), "{fen}: score {}", r.score);
        assert_eq!(score::uci(r.score), "mate 2", "{fen}");
        assert_eq!(r.pv.len(), 3, "{fen}: pv {:?}", r.pv);
    }
}

#[test]
fn mate_in_one_and_being_mated_in_one_are_scored_by_distance() {
    // Rb8# for White to move; Black to move can only walk into it.
    let mut w = support::position("7k/8/6K1/8/8/8/8/1R6 w - - 0 1");
    let mates = mates_in_one(&mut w);
    assert_eq!(mates, vec![mv(&w, "b1b8")]);
    let r = search(&mut w, Limits::depth(2));
    assert_eq!(r.best, mates[0]);
    assert_eq!(r.score, mate_in(1));
    assert_eq!(score::uci(r.score), "mate 1");

    let mut b = support::position("7k/8/6K1/8/8/8/8/1R6 b - - 0 1");
    assert_eq!(generate_legal(&b).len(), 1, "only Kg8");
    let r = search(&mut b, Limits::depth(3));
    assert_eq!(r.score, mated_in(2), "score {}", r.score);
    assert_eq!(score::uci(r.score), "mate -1");
    // Deeper does not change a forced mate's distance.
    let r = search(&mut b, Limits::depth(5));
    assert_eq!(r.score, mated_in(2), "score {}", r.score);
}

#[test]
fn a_mated_or_stalemated_root_returns_null_with_the_terminal_score() {
    let mut b = support::position("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1");
    assert!(is_mated(&b));
    let r = search(&mut b, Limits::depth(3));
    assert_eq!(r.best, Move::NULL);
    assert_eq!(r.score, mated_in(0));
    assert_eq!(r.score, -MATE);

    let mut b = support::position("7k/8/6Q1/8/8/8/8/7K b - - 0 1");
    assert!(!b.in_check() && generate_legal(&b).is_empty(), "stalemate");
    let r = search(&mut b, Limits::depth(3));
    assert_eq!(r.best, Move::NULL);
    assert_eq!(r.score, DRAW);
}

// ---------------------------------------------------------------------------
// Draws
// ---------------------------------------------------------------------------

/// Black's only save is Qe1+ Kg2, whose position is already twice in the game history. The search
/// must see the threefold two plies in; without the history it sees only the material.
#[test]
fn a_threefold_against_the_game_history_is_a_draw_in_the_tree() {
    // P_b, Black to move, with Qe1 and Kg2 -- then the cycle is played to
    // bring the root back to Qe3 / Kg1 with P_b twice in the history.
    let fen = "7k/RQ4p1/8/8/8/8/5PKP/4q3 b - - 10 40";
    let mut b = support::position(fen);
    for u in ["e1e3", "g2g1", "e3e1", "g1g2", "e1e3", "g2g1"] {
        let m = mv(&b, u);
        b.play(m);
    }
    assert_eq!(b.game_history().len(), 6);
    let root = b.to_fen(cadence_core::FenStyle::Shredder);
    // Kg2 is the only reply to Qe1+: the construction, checked.
    {
        let mut probe = b.duplicate();
        let check = mv(&probe, "e3e1");
        probe.make_move(check);
        let replies = generate_legal(&probe);
        assert_eq!(
            replies.len(),
            1,
            "replies to Qe1+: {:?}",
            replies.as_slice()
        );
        assert_eq!(replies.as_slice()[0], mv(&probe, "g1g2"));
    }
    for depth in 3..=4 {
        let r = search(&mut b, Limits::depth(depth));
        assert_eq!(
            r.best,
            mv(&b, "e3e1"),
            "depth {depth} from {root}: played {:?}",
            r.best
        );
        assert_eq!(r.score, DRAW, "depth {depth}: score {}", r.score);
    }
    // Without the history: the same position is simply lost.
    let mut fresh = support::position(&root);
    let r = search(&mut fresh, Limits::depth(3));
    assert!(r.score < -500, "without history, score {}", r.score);
}

/// Whatever Black plays, White's reply reaches a fifty-move draw: neither side has a capture, a
/// pawn move or a mate in one.
#[test]
fn the_fifty_move_rule_is_a_draw_in_the_tree() {
    let fen = "8/8/8/3k4/8/8/8/QQQ1K3 b - - 98 70";
    let mut b = support::position(fen);
    // The construction, checked: after every Black move, White has no mate
    // in one, and neither side has a capture or a pawn move.
    for m in generate_legal(&b).iter() {
        assert!(!m.is_capture());
        b.make_move(m);
        assert!(
            mates_in_one(&mut b).is_empty(),
            "after {m:?} White mates in one"
        );
        b.unmake_move(m);
    }
    let r = search(&mut b, Limits::depth(3));
    assert_eq!(r.score, DRAW, "score {}", r.score);
    // One ply earlier it is not a draw yet: at the leaf the clock reads 99.
    let mut earlier = support::position("8/8/8/3k4/8/8/8/QQQ1K3 b - - 97 70");
    let r = search(&mut earlier, Limits::depth(2));
    assert!(r.score < -1000, "score {}", r.score);
}

/// A stalemate is a draw, and a draw scores below a won position.
#[test]
fn a_stalemate_is_a_draw_and_is_not_chosen_when_winning() {
    // Black's queen on f4; Qf2 would leave White's king on h1, its three
    // squares covered by the queen and the pawns, with no move and not in
    // check.
    let fen = "7k/8/8/8/5q2/6pp/8/7K b - - 0 1";
    let mut b = support::position(fen);
    let qf2 = mv(&b, "f4f2");
    b.make_move(qf2);
    assert!(
        !b.in_check() && generate_legal(&b).is_empty(),
        "Qf2 is not stalemate"
    );
    b.unmake_move(qf2);
    for depth in 2..=4 {
        let r = search(&mut b, Limits::depth(depth));
        assert_ne!(r.best, qf2, "depth {depth} stalemated");
        assert!(r.score > 500, "depth {depth}: score {}", r.score);
    }
}

// ---------------------------------------------------------------------------
// Limits
// ---------------------------------------------------------------------------

#[test]
fn the_node_limit_stops_the_search() {
    let fen = support::standard_fen("kiwipete");
    // A limit under depth one's cost stops inside the first iteration, which reports no completed
    // depth and plays the best fully searched root move.
    let depth_one = search(&mut support::position(&fen), Limits::depth(1)).nodes;
    for n in [100u64, 1000, 5000, 20_000, 4 * depth_one] {
        let mut b = support::position(&fen);
        let limits = Limits {
            nodes: Some(n),
            ..Limits::default()
        };
        let r = search(&mut b, limits);
        assert!(r.nodes <= n, "nodes {n}: searched {}", r.nodes);
        assert!(
            generate_legal(&b).contains(r.best),
            "nodes {n}: {:?}",
            r.best
        );
        assert_eq!(
            r.depth >= 1,
            depth_one < n,
            "nodes {n}: depth {} with depth one costing {depth_one}",
            r.depth
        );
    }
    // A limit too small for one iteration still yields a legal move.
    let mut b = support::position(&fen);
    let limits = Limits {
        nodes: Some(1),
        ..Limits::default()
    };
    let r = search(&mut b, limits);
    assert!(generate_legal(&b).contains(r.best));
}

/// Stated here, not imported: a gate reading the constant it checks cannot see it change.
const EXTEND_WITHIN: usize = 2;

/// No child past `EXTEND_WITHIN` times the root depth is extended, so the pv runs at most to this.
/// Necessary, not sufficient: a line breaking the cap without becoming the pv is invisible here.
fn pv_bound(depth: u32) -> usize {
    (EXTEND_WITHIN + 1) * depth as usize - 1
}

#[test]
fn the_depth_limit_is_exact() {
    let mut b = support::position(START_FEN);
    for depth in [1, 2, 5] {
        let r = search(&mut b, Limits::depth(depth));
        assert_eq!(r.depth, depth);
        assert!(
            r.pv.len() <= pv_bound(depth),
            "pv {:?} longer than depth {depth} can extend to",
            r.pv
        );
    }
}

// ---------------------------------------------------------------------------
// The check extension
// ---------------------------------------------------------------------------

/// The only gate that sees the cap exactly, every case either side of it, including the root depth
/// of zero `Search::node` leaves. Whether it is wired into the child's depth is the next gate's
/// question.
#[test]
fn a_check_extends_by_one_ply_and_nothing_does_past_the_cap() {
    for root_depth in [0u32, 1, 2, 7, 20] {
        let cap = EXTEND_WITHIN * root_depth as usize;
        for ply in 0..cap + 4 {
            assert_eq!(
                extension(false, ply, root_depth),
                0,
                "root depth {root_depth}, ply {ply}: a move that gave no check extended"
            );
            assert_eq!(
                extension(true, ply, root_depth),
                u32::from(ply < cap),
                "root depth {root_depth}, ply {ply}: the cap is {cap}"
            );
        }
    }
}

/// A mate by quiet checks, which quiescence does not generate, found below its nominal depth shows
/// which lines were extended, as a node count cannot. No extension fails the first assertion and
/// extending by two or every move fails the second; before the extension these mates needed depths
/// 3 and 5.
#[test]
fn a_mate_by_quiet_checks_is_found_at_the_depth_the_extension_buys() {
    // The mate, in moves, and the nominal depth the extension finds it at.
    for (fen, mate, depth) in [
        // 1. Qd8+ Bxd8 2. Re8#.
        (
            "r1b2k1r/ppp1bppp/8/1B1Q4/5q2/2P5/PP1P1PPP/R3R1K1 w - - 0 1",
            2,
            2,
        ),
        // 1. Nh6+ Kh8 2. Qg8+ Rxg8 3. Nf7#.
        ("5rk1/5Npp/8/8/8/1Q6/8/6K1 w - - 0 1", 3, 3),
    ] {
        let mut b = support::position(fen);
        let r = search(&mut b, Limits::depth(depth));
        assert_eq!(
            r.score,
            mate_in(2 * mate - 1),
            "{fen}: depth {depth} scored {} and not mate {mate}",
            score::uci(r.score)
        );
        let r = search(&mut b, Limits::depth(depth - 1));
        assert!(
            !score::is_mate(r.score),
            "{fen}: depth {} already scored {}, so the extension is too large \
             or is not conditioned on the check",
            depth - 1,
            score::uci(r.score)
        );
    }
}

/// Every reported line stays inside `pv_bound`, in positions full of checks.
#[test]
fn no_line_runs_past_the_ply_the_extension_stops_at() {
    let checking = [
        // A queen against a bare king: almost every move is a check.
        "7k/8/8/8/8/8/8/Q6K w - - 0 1",
        // Two queens, two exposed kings, and checks for both sides.
        "1q5k/8/8/8/8/8/8/1Q5K w - - 0 1",
        // A rook and a queen loose around a king on an open board.
        "8/8/4k3/8/8/2Q5/8/4K2R w - - 0 1",
    ];
    for fen in checking.iter().map(|f| (*f).to_string()).chain(sample()) {
        let mut b = support::position(&fen);
        for depth in [1u32, 2, 4] {
            let r = search(&mut b, Limits::depth(depth));
            assert!(
                r.pv.len() <= pv_bound(depth),
                "{fen}: depth {depth} reported a {}-move line, past {}",
                r.pv.len(),
                pv_bound(depth)
            );
            assert!(
                r.best.is_null() || generate_legal(&b).contains(r.best),
                "{fen}: depth {depth} returned a move the position does not have"
            );
        }
    }
}

/// Ply `MAX_PLY` is always a quiescence node in a real search, so the boundary is called directly
/// with depth to go, the state an extension creates. Past it, `killers[ply]` and the state stack
/// are slice bounds checks that abort the process in release.
#[test]
fn an_interior_node_at_the_ply_bound_answers_instead_of_running_off_its_arrays() {
    let stop = AtomicBool::new(false);
    let tt = table();
    for fen in sample() {
        let mut b = support::position(&fen);
        // Past the bound as well as at it: an extension that gives back
        // more than one ply, or a bound written as an equality, both land
        // here.
        for ply in [MAX_PLY, MAX_PLY + 1, MAX_PLY + 64] {
            for depth in [1u32, 2, 8] {
                let mut s = support::search(Limits::default(), &stop, &tt);
                let score = s.node(&mut b, depth, ply);
                assert_eq!(
                    score,
                    eval::evaluate(&b),
                    "{fen}: ply {ply} depth {depth} did not stand on the evaluation"
                );
                assert_eq!(s.nodes(), 1, "{fen}: ply {ply} depth {depth} searched on");
                assert_eq!(b.ply(), 0, "{fen}: ply {ply} depth {depth} made a move");
            }
        }
    }
}

/// A guard one ply early would pass the gate above while truncating the deepest interior node, and
/// nothing at bench depth reaches it.
#[test]
fn the_deepest_ply_a_search_reaches_is_still_searched() {
    let stop = AtomicBool::new(false);
    let tt = table();
    for fen in sample() {
        // Cleared between positions as `bench` does: the sample holds one start array twice, and a
        // carried table would answer the second from the first.
        tt.clear();
        let mut b = support::position(&fen);
        let mut s = support::search(Limits::default(), &stop, &tt);
        let _ = s.node(&mut b, 1, MAX_PLY - 1);
        assert!(s.nodes() > 1, "{fen}: ply {} searched nothing", MAX_PLY - 1);
        assert_eq!(b.ply(), 0, "{fen}: board left off its root");
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// The bench's depth with no table, so the root's value is a function of the position and the depth
/// alone.
const WINDOW_DEPTH: u32 = 7;

/// Where the last unbroken run of window-independence ends on the pawn endgames, re-measured as
/// rules reading the window or the sort's rank land. At two it is one ply above the depth-one arm,
/// and the next rule to read the rank closes it.
const BRACKET_DEPTH: u32 = 2;

fn no_table() -> Table {
    Table::with_buckets(0).expect("a table of no buckets")
}

/// Fail-soft, so `(v - 1, v)` and `(v, v + 1)` around a full-window value `v` both return `v`; a
/// null window off by one, inverted or negated on the wrong side breaks it at depth one. Pruning
/// and ordering make the value window-dependent past depth one, so it is asserted at depth one
/// everywhere and to `BRACKET_DEPTH` on the pawn endgames, with no table.
#[test]
fn a_window_that_brackets_the_value_returns_the_value() {
    let stop = AtomicBool::new(false);
    for (fen, depths) in sample().into_iter().map(|f| (f, 1..=1)).chain(
        support::PAWN_ENDGAMES
            .into_iter()
            .map(|f| (f.to_string(), 1..=BRACKET_DEPTH)),
    ) {
        let mut b = support::position(&fen);
        for depth in depths {
            let tt = no_table();
            let full = support::search(Limits::default(), &stop, &tt).node(&mut b, depth, 0);
            let below = support::search(Limits::default(), &stop, &tt).node_window(
                &mut b,
                depth,
                0,
                full - 1,
                full,
            );
            let above = support::search(Limits::default(), &stop, &tt).node_window(
                &mut b,
                depth,
                0,
                full,
                full + 1,
            );
            assert_eq!(
                below,
                full,
                "{fen}: depth {depth}, the window ({}, {full}) did not agree with the full one",
                full - 1
            );
            assert_eq!(
                above,
                full,
                "{fen}: depth {depth}, the window ({full}, {}) did not agree with the full one",
                full + 1
            );
        }
    }
}

/// A narrower window may not change the root's move or score: it changes which parts of a fixed
/// tree are visited, nothing else. The fixture is re-measured only for changes to the tree, never
/// for a windowing change.
#[test]
fn a_narrower_window_returns_the_same_move_and_the_same_score() {
    let tt = no_table();
    let stop = AtomicBool::new(false);
    let got: Vec<(String, Score)> = sample()
        .iter()
        .map(|fen| {
            let mut b = support::position(fen);
            let mut s = support::search(Limits::depth(WINDOW_DEPTH), &stop, &tt);
            let best = s.run(&mut b, &mut Vec::new());
            (best.to_uci_chess960(), s.score())
        })
        .collect();
    println!("depth {WINDOW_DEPTH}, no table: {got:?}");
    let want: Vec<(String, Score)> = FULL_WINDOW_ANSWERS
        .iter()
        .map(|(m, s)| ((*m).to_string(), *s))
        .collect();
    assert_eq!(
        got, want,
        "a move or a score moved, so the window is not only a window"
    );
}

/// Measured on the shipped full-window build at `WINDOW_DEPTH` with no table. The promotions on c8
/// search to the same value, so `d7c8q` and `d7c8r` swapping is a tie-break, not a defect.
const FULL_WINDOW_ANSWERS: [(&str, Score); 14] = [
    ("d2d4", 38),
    ("d5e6", -147),
    ("b4f4", 59),
    ("c4c5", -390),
    ("d7c8q", 497),
    ("g5f6", 47),
    ("d2d4", 38),
    ("b2b3", 42),
    ("e1d3", 61),
    ("b2b3", 97),
    ("h1h7", 531),
    ("f1f4", 531),
    ("g1g4", 531),
    ("a1a4", 523),
];

/// With no table, the window is the only thing that can move the count. The ceiling is the
/// full-window count, which has no null move or reductions to lose.
#[test]
fn the_narrower_window_saves_nodes() {
    let tt = no_table();
    let stop = AtomicBool::new(false);
    let mut total = 0u64;
    for fen in sample() {
        let mut b = support::position(&fen);
        let mut s = support::search(Limits::depth(WINDOW_DEPTH), &stop, &tt);
        let _ = s.run(&mut b, &mut Vec::new());
        total += s.nodes();
    }
    println!(
        "depth {WINDOW_DEPTH}, {} positions, no table: {total} nodes",
        sample().len()
    );
    assert!(
        total < 17_000_000,
        "{total} nodes against the 17,337,259 the same search took with the full window everywhere"
    );
}

// ---------------------------------------------------------------------------
// What the GUI sees
// ---------------------------------------------------------------------------

#[test]
fn info_lines_report_each_iteration_and_agree_with_bestmove() {
    let fen = support::standard_fen("kiwipete");
    let out = support::Engine::go(&[&format!("position fen {fen}")], "go depth 3").join("\n");
    let infos: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("info depth "))
        .collect();
    assert_eq!(infos.len(), 3, "{out}");
    let mut last_nodes = 0u64;
    let mut last_pv: Vec<String> = Vec::new();
    for (i, line) in infos.iter().enumerate() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let field = |name: &str| -> Option<String> {
            toks.iter()
                .position(|t| *t == name)
                .and_then(|p| toks.get(p + 1))
                .map(|s| (*s).to_string())
        };
        assert_eq!(
            field("depth").as_deref(),
            Some((i + 1).to_string().as_str()),
            "{line}"
        );
        let kind = field("score").expect("score");
        assert!(kind == "cp" || kind == "mate", "{line}");
        let nodes: u64 = field("nodes").and_then(|n| n.parse().ok()).expect("nodes");
        assert!(nodes >= last_nodes, "{line}");
        last_nodes = nodes;
        let _: u64 = field("nps").and_then(|n| n.parse().ok()).expect("nps");
        let _: u64 = field("time").and_then(|n| n.parse().ok()).expect("time");
        let pv_at = toks.iter().position(|t| *t == "pv").expect("pv");
        last_pv = toks[pv_at + 1..].iter().map(|s| (*s).to_string()).collect();
        assert!(!last_pv.is_empty(), "{line}");
        // Every pv move is legal in sequence.
        let mut b = board(&fen);
        for u in &last_pv {
            let m = parse_uci(&generate_legal(&b), u)
                .unwrap_or_else(|| panic!("pv move {u} illegal: {line}"));
            b.make_move(m);
        }
    }
    assert_eq!(support::bestmove(&out), last_pv[0]);
}

#[test]
fn a_mate_is_reported_as_mate_and_spelled_per_the_option() {
    let out = support::Engine::go(
        &["position fen 7k/8/6K1/8/8/8/8/1R6 w - - 0 1"],
        "go depth 3",
    )
    .join("\n");
    assert!(
        out.lines()
            .any(|l| l.starts_with("info depth ") && l.contains("score mate 1")),
        "{out}"
    );
    assert_eq!(support::bestmove(&out), "b1b8");
    // A DFRC position whose best move is a castle, spelled both ways.
    let fen = "1k6/8/8/8/8/8/8/R3K1R1 w GA - 0 1";
    let b = board(fen);
    let legal = generate_legal(&b);
    for chess960 in [false, true] {
        let out = support::Engine::go(
            &[
                &format!("setoption name UCI_Chess960 value {chess960}"),
                &format!("position fen {fen}"),
            ],
            "go depth 2",
        )
        .join("\n");
        let bm = support::bestmove(&out);
        let m = parse_uci(&legal, &bm).unwrap_or_else(|| panic!("{bm} illegal"));
        assert_eq!(
            bm,
            to_uci(m, &legal, chess960),
            "spelling under chess960={chess960}"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance
// ---------------------------------------------------------------------------

fn engine_player(depth: u32, nodes: &mut u64) -> impl FnMut(&mut Position) -> Move + '_ {
    move |b: &mut Position| {
        let r = search(b, Limits::depth(depth));
        *nodes += r.nodes;
        r.best
    }
}

#[test]
fn a_legal_game_to_completion_against_itself_standard_and_dfrc() {
    let arrays = support::dfrc_arrays();
    for fen in [
        START_FEN.to_string(),
        arrays[0].2.clone(),
        arrays[7].2.clone(),
    ] {
        let mut nw = 0;
        let mut nb = 0;
        let (outcome, moves) = play_game(
            &fen,
            &mut engine_player(3, &mut nw),
            &mut engine_player(3, &mut nb),
            400,
        );
        println!(
            "{fen}: {outcome:?} after {} plies, {} nodes",
            moves.len(),
            nw + nb
        );
        assert!(moves.len() >= 20, "{fen}: over after {} plies", moves.len());
        assert_ne!(
            outcome,
            Outcome::Cap,
            "{fen}: the game did not end in 400 plies"
        );
    }
}

/// `beats_a_random_mover_at_depth_five` is the same nearer playing depth, ignored and run by hand
/// in release.
#[test]
fn beats_a_random_mover_a_hundred_times() {
    let (won, played, plies) = versus_random(100, 3);
    println!("{won}/{played} in {plies} plies");
    assert_eq!(won, played);
}

#[test]
#[ignore = "the acceptance run at depth five: run in release"]
fn beats_a_random_mover_at_depth_five() {
    let (won, played, plies) = versus_random(100, 5);
    println!("{won}/{played} in {plies} plies");
    assert_eq!(won, played);
}

/// Returns wins, games and total plies.
fn versus_random(games: usize, depth: u32) -> (usize, usize, usize) {
    let mut won = 0;
    let mut plies = 0;
    for g in 0..games {
        let mut rng = Rng::new(0x5EED_0000 + g as u64);
        let mut nodes = 0;
        let engine_is_white = g % 2 == 0;
        let (outcome, moves) = if engine_is_white {
            play_game(
                START_FEN,
                &mut engine_player(depth, &mut nodes),
                &mut random_mover(&mut rng),
                600,
            )
        } else {
            play_game(
                START_FEN,
                &mut random_mover(&mut rng),
                &mut engine_player(depth, &mut nodes),
                600,
            )
        };
        plies += moves.len();
        let us = if engine_is_white {
            Colour::White
        } else {
            Colour::Black
        };
        if outcome == Outcome::Mate(us) {
            won += 1;
        } else {
            println!("game {g}: {outcome:?} after {} plies", moves.len());
        }
    }
    (won, games, plies)
}
