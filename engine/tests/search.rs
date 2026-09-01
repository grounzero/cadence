// SPDX-License-Identifier: GPL-3.0-or-later

//! The search: it deepens, it is a function of the position and the depth,
//! it finds mates and scores them by distance, it scores draws as draws,
//! it honours its limits, and it reports what it did.
//!
//! What a search gate cannot do is say the move is *good*; that is SPRT's
//! job, later. What it can do is pin every property that does not need an
//! opponent to observe: that depth d+1 searches more than depth d and says
//! so; that two searches of one position agree to the node; that a mate in
//! two is found with the score `mate 2` and a mate in one with `mate 1`;
//! that a repetition the game history makes a threefold is a draw in the
//! tree, and a stalemate is not a win; that `nodes N` stops at N; and that
//! the `info` lines a GUI reads are there and consistent with the
//! `bestmove`. The mate positions are checked by a brute-force verifier
//! written here, so an expected key move that is wrong fails this file, not
//! the engine.
//!
//! Then the acceptance: a game to completion against itself, standard and
//! DFRC, and a clean sweep against a random mover. Samples here; the full
//! runs are ignored and recorded by hand.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{Colour, MAX_PLY, Move, START_FEN, generate_legal, parse_uci, to_uci};
use cadence_engine::eval;
use cadence_engine::score::{self, DRAW, MATE, Score, mate_in, mated_in};
use cadence_engine::search::{Limits, Search, extension};
use cadence_engine::tt::Table;
use support::{Outcome, Rng, play_game, random_mover, table};

/// The result of one search: move, score, nodes, completed depth, pv.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Result {
    best: Move,
    score: Score,
    nodes: u64,
    depth: u32,
    pv: Vec<Move>,
}

fn search(board: &mut Board, limits: Limits) -> Result {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut sink = Vec::new();
    let mut s = Search::new(limits, &stop, &tt);
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

/// Positions a search gate can be run over cheaply: the standard suite,
/// four DFRC arrays and four castling-legality positions.
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
            let r = search(&mut board(&fen), Limits::depth(depth));
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

/// Two fresh searches of one position at one depth agree on the move, the
/// score, the node count and the pv -- in a different order of positions
/// the second time, so that nothing carried between searches could hide.
#[test]
fn the_same_position_and_depth_give_the_same_move_score_and_node_count() {
    let fens = sample();
    let first: Vec<Result> = fens
        .iter()
        .map(|f| search(&mut board(f), Limits::depth(3)))
        .collect();
    let mut again: Vec<Option<Result>> = vec![None; fens.len()];
    for (i, f) in fens.iter().enumerate().rev() {
        again[i] = Some(search(&mut board(f), Limits::depth(3)));
    }
    for (i, f) in fens.iter().enumerate() {
        assert_eq!(Some(&first[i]), again[i].as_ref(), "{f}");
    }
    // And the same board searched twice in a row.
    let mut b = board(&support::standard_fen("kiwipete"));
    let a = search(&mut b, Limits::depth(4));
    let c = search(&mut b, Limits::depth(4));
    assert_eq!(a, c);
    assert_eq!(b.ply(), 0, "the search left moves on the stack");
}

/// The same, across processes: `go depth N` twice in two processes gives
/// the same bestmove and the same node count on the final info line.
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

/// Whether the side to move is mated.
fn is_mated(b: &Board) -> bool {
    b.in_check() && generate_legal(b).is_empty()
}

/// The side to move's moves that mate at once.
fn mates_in_one(b: &mut Board) -> Vec<Move> {
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

/// The side to move's moves after which every reply allows a mate in one:
/// the keys of a mate in two. Brute force, three plies, written from
/// `generate_legal` alone so it shares nothing with the search.
fn mates_in_two(b: &mut Board) -> Vec<Move> {
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

/// Mate-in-two positions with the expected key. Each is verified by the
/// brute-force search above before the engine is asked: the key is among
/// the mates in two, and there is no mate in one.
const MATES_IN_TWO: &[(&str, &str)] = &[
    // Two rooks, the ladder: Rb7+ and Ra8#.
    ("8/7k/R7/8/8/8/8/1R4K1 w - - 0 1", "b1b7"),
    // Queen and king: the quiet Qa7, then Qg7#.
    ("7k/8/5K2/8/8/8/8/Q7 w - - 0 1", "a1a7"),
    // Queen and bishop against a castled king: Qxf7+ Kh8 Qxe8#. (A first
    // draft without the rook on e8 had Qd8# in one -- h8 stays attacked
    // through the vacated g8 -- and the verifier below said so.)
    ("4r1k1/5ppp/8/3Q4/2B5/8/8/6K1 w - - 0 1", "d5f7"),
    // The same three, for Black, by mirror.
    ("1r4k1/8/8/8/8/r7/7K/8 b - - 0 1", "b8b2"),
    ("q7/8/8/8/8/5k2/8/7K b - - 0 1", "a8a2"),
    ("6k1/8/8/2b5/3q4/8/5PPP/4R1K1 b - - 0 1", "d4f2"),
];

#[test]
fn the_mate_in_two_positions_are_what_they_claim() {
    for (fen, key) in MATES_IN_TWO {
        let mut b = board(fen);
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
        let mut b = board(fen);
        let keys = mates_in_two(&mut b);
        // Mate in two is three plies, and the mated side is found to have
        // no moves only at a node that generates them, which a leaf does
        // not: depth 4.
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
    let mut w = board("7k/8/6K1/8/8/8/8/1R6 w - - 0 1");
    let mates = mates_in_one(&mut w);
    assert_eq!(mates, vec![mv(&w, "b1b8")]);
    let r = search(&mut w, Limits::depth(2));
    assert_eq!(r.best, mates[0]);
    assert_eq!(r.score, mate_in(1));
    assert_eq!(score::uci(r.score), "mate 1");

    let mut b = board("7k/8/6K1/8/8/8/8/1R6 b - - 0 1");
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
    let mut b = board("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1");
    assert!(is_mated(&b));
    let r = search(&mut b, Limits::depth(3));
    assert_eq!(r.best, Move::NULL);
    assert_eq!(r.score, mated_in(0));
    assert_eq!(r.score, -MATE);

    let mut b = board("7k/8/6Q1/8/8/8/8/7K b - - 0 1");
    assert!(!b.in_check() && generate_legal(&b).is_empty(), "stalemate");
    let r = search(&mut b, Limits::depth(3));
    assert_eq!(r.best, Move::NULL);
    assert_eq!(r.score, DRAW);
}

// ---------------------------------------------------------------------------
// Draws
// ---------------------------------------------------------------------------

/// Black, a queen and a rook down, has one saving move: Qe1+, to which Kg2
/// is the only reply, and the position after it has occurred twice in the
/// game already. The search must see the threefold two plies into the tree
/// and play for it; without the history the same search sees only the
/// material.
#[test]
fn a_threefold_against_the_game_history_is_a_draw_in_the_tree() {
    // P_b, Black to move, with Qe1 and Kg2 -- then the cycle is played to
    // bring the root back to Qe3 / Kg1 with P_b twice in the history.
    let fen = "7k/RQ4p1/8/8/8/8/5PKP/4q3 b - - 10 40";
    let mut b = board(fen);
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
    let mut fresh = board(&root);
    let r = search(&mut fresh, Limits::depth(3));
    assert!(r.score < -500, "without history, score {}", r.score);
}

/// Two plies before the fifty-move rule falls, with no capture or pawn move
/// available to either side and no mate in one for White: whatever Black
/// plays, the position after White's reply is a draw.
#[test]
fn the_fifty_move_rule_is_a_draw_in_the_tree() {
    let fen = "8/8/8/3k4/8/8/8/QQQ1K3 b - - 98 70";
    let mut b = board(fen);
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
    let mut earlier = board("8/8/8/3k4/8/8/8/QQQ1K3 b - - 97 70");
    let r = search(&mut earlier, Limits::depth(2));
    assert!(r.score < -1000, "score {}", r.score);
}

/// Black to move is winning and could stalemate White with Qf2. It must
/// not: a stalemate is a draw, and a draw scores below a won position.
#[test]
fn a_stalemate_is_a_draw_and_is_not_chosen_when_winning() {
    // Black's queen on f4; Qf2 would leave White's king on h1, its three
    // squares covered by the queen and the pawns, with no move and not in
    // check.
    let fen = "7k/8/8/8/5q2/6pp/8/7K b - - 0 1";
    let mut b = board(fen);
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
    // What depth one costs here, so that the limits read against it: a
    // limit under it stops the search inside its first iteration, which
    // then reports no completed depth and plays the best root move it had
    // fully searched; a limit over it completes depth one at least.
    let depth_one = search(&mut board(&fen), Limits::depth(1)).nodes;
    for n in [100u64, 1000, 5000, 20_000, 4 * depth_one] {
        let mut b = board(&fen);
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
    let mut b = board(&fen);
    let limits = Limits {
        nodes: Some(1),
        ..Limits::default()
    };
    let r = search(&mut b, limits);
    assert!(generate_legal(&b).contains(r.best));
}

/// The bound a principal variation is inside of, which is no longer the
/// depth asked for.
///
/// A check extension gives a ply back, so `ply + depth` is the depth asked
/// for plus the extensions granted above the node, and the line searched
/// runs past the iteration's number. What still bounds it is the ply the
/// extension stops being granted at: no child past `EXTEND_WITHIN` times
/// the root depth gets one, so from there depth falls by one per ply, the
/// deepest interior node is at ply `(EXTEND_WITHIN + 1) * root_depth - 2`
/// and the line from the root is one longer than that.
///
/// **This assertion used to read `pv.len() <= depth`**, which was the same
/// bound with nothing giving a ply back. It is narrowed rather than
/// dropped: a pv longer than the extension can pay for is still a fault,
/// and it is the only consequence of the ply cap that a whole search
/// exposes. It is necessary and not sufficient -- the pv is one line, and a
/// line that breaks the cap without becoming the pv is invisible here.
/// How many times the root depth the check extension is granted within.
///
/// Stated here rather than imported from the search: a gate that reads the
/// constant it is checking cannot see that constant change, and changing
/// the cap changes which lines are searched deeper, which is the thing this
/// file exists to notice.
const EXTEND_WITHIN: usize = 2;

fn pv_bound(depth: u32) -> usize {
    (EXTEND_WITHIN + 1) * depth as usize - 1
}

#[test]
fn the_depth_limit_is_exact() {
    let mut b = board(START_FEN);
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

/// The extension itself, as arithmetic: one ply for a check, nothing for
/// anything else, and nothing at all past the ply cap.
///
/// This is the only gate that sees the cap exactly. Everything below it
/// runs a search, and a search shows the cap only where a line happens to
/// reach it; here every case either side of the boundary is visited,
/// including the root depth of zero that [`Search::node`] leaves when it is
/// called at a boundary rather than run.
///
/// What it cannot see is whether the function is wired into the depth the
/// child is searched at. That is the next gate.
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

/// A forced mate delivered by quiet checks is found a full move earlier
/// than the depth it needs without the extension, and no earlier than that.
///
/// **This is the gate the bench cannot stand in for.** A node count is a
/// number the whole tree produces, and it moves for an extension applied to
/// the wrong nodes, at the wrong size, exactly as it moves for the right
/// one. What is wanted is evidence about *which* lines were searched
/// deeper, and a mate score is that: a mate at a distance the nominal depth
/// does not reach can only be reported if the line that delivers it was
/// given the plies back.
///
/// Both positions mate by **quiet** checks, which is what makes them
/// sensitive. The quiescence search below the horizon generates the noisy
/// moves out of check and every evasion in check, so a mating net built
/// from captures would be found there whatever the main search did. A quiet
/// check is generated by neither, so the ply has to come from the
/// extension.
///
/// **Each of the three ways to get this wrong fails one half of it.**
/// Extending by nothing fails the first assertion. Extending by two plies,
/// or extending every move rather than the checking ones, moves the mate a
/// further move earlier and fails the second. The depths below were read
/// off the tree before the extension existed: the first mate is found at
/// depth 3 there and the second at depth 5, one and two moves later than
/// here.
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
        let mut b = board(fen);
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

/// The ply cap, seen from a whole search: every line the search reports is
/// inside the bound the cap implies, in positions chosen because checks are
/// everywhere in them.
///
/// What this cannot see is stated on [`pv_bound`]: it is one line per
/// search. The cap itself is arithmetic and is gated as arithmetic, on the
/// function that computes it; the arrays it does not protect are held by
/// the ply bound in `negamax`, which is gated separately above.
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
        let mut b = board(&fen);
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

/// The ply bound, which is the limit the search cannot be asked for.
///
/// At `ply == MAX_PLY` the per-ply arrays end. The first thing an interior
/// node touches past their end is `killers[ply]`, read for the sort before
/// any move is made; the state stack `make_move` pushes is the failure
/// behind it, and `PvTable` guards itself so it is neither. Both are slice
/// bounds checks, which are present in release, where the `debug_assert`
/// that used to stand in for them is not, and where `panic = "abort"` makes
/// either one a process that stops playing mid-game.
///
/// **This cannot be driven through `run`, and a gate that tried would be
/// vacuous.** The root depth is capped at `MAX_PLY` and depth falls by one
/// per ply, so the deepest interior node any search can produce is at ply
/// `MAX_PLY - 1` and ply `MAX_PLY` is always a quiescence node, which has
/// carried its own bound all along. So the boundary is called at directly,
/// with depth still to go: the state an extension is what creates.
#[test]
fn an_interior_node_at_the_ply_bound_answers_instead_of_running_off_its_arrays() {
    let stop = AtomicBool::new(false);
    let tt = table();
    for fen in sample() {
        let mut b = board(&fen);
        // Past the bound as well as at it: an extension that gives back
        // more than one ply, or a bound written as an equality, both land
        // here.
        for ply in [MAX_PLY, MAX_PLY + 1, MAX_PLY + 64] {
            for depth in [1u32, 2, 8] {
                let mut s = Search::new(Limits::default(), &stop, &tt);
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

/// The other side of it: the bound is at `MAX_PLY` and not below. A guard
/// one ply early would pass the gate above while truncating the deepest
/// interior node a search can reach, and `bench` would not catch it,
/// because nothing at bench depth goes near ply `MAX_PLY - 1`.
#[test]
fn the_deepest_ply_a_search_reaches_is_still_searched() {
    let stop = AtomicBool::new(false);
    let tt = table();
    for fen in sample() {
        // Cleared between positions, as `bench` does: the sample holds one
        // start array twice over, once as a FEN and once as a DFRC one,
        // and a table carried across them answers the second from the
        // first without searching anything.
        tt.clear();
        let mut b = board(&fen);
        let mut s = Search::new(Limits::default(), &stop, &tt);
        let _ = s.node(&mut b, 1, MAX_PLY - 1);
        assert!(s.nodes() > 1, "{fen}: ply {} searched nothing", MAX_PLY - 1);
        assert_eq!(b.ply(), 0, "{fen}: board left off its root");
    }
}

// ---------------------------------------------------------------------------
// The window
// ---------------------------------------------------------------------------

/// The depth the two end-to-end window gates run at.
///
/// Seven, the bench's own depth, over `sample()` with a table of no
/// buckets, so that what they see is a tree of the size the regression
/// detector reads and the value at the root is a function of the position
/// and the depth alone.
const WINDOW_DEPTH: u32 = 7;

/// The depth the property gate runs to on the pawn endgames.
///
/// It is a different constant because it is answering a different
/// question. The property does not depend on the depth -- a window that
/// brackets the value returns the value at depth one -- so what depth buys
/// here is positions where the recursion has passed the window through
/// several plies.
///
/// **Four, unchanged when futility pruning landed, and the re-check is
/// worth recording because a coverage constant nobody re-measures is the
/// failure the counterfactual ceilings in `tests/ordering.rs` keep
/// finding.** The rule reads alpha and skips quiet moves, so it is
/// window-dependent like the two below and could have shortened the run
/// again. Measured on the tree it landed on, both endgames, depths one to
/// seven: the last unbroken run still ends at four, the second endgame
/// still diverges first at five (453 against 454 below), and the first
/// still diverges at seven. The constant stands on a new measurement
/// rather than on the old one.
///
/// **Re-checked again when reverse futility landed, and the run is
/// unmoved.** That rule reads beta and returns the node outright, so it is
/// window-dependent in the sharpest way anything here has been and could
/// have shortened the run a third time. Measured on the tree it landed on,
/// both endgames, depths one to seven: they agree through four, the second
/// endgame diverges first at five (452 against 454 below), and the first
/// diverges at seven. Two of the three numbers moved by a point or two and
/// the structure did not: the last unbroken run still ends at four.
///
/// **Four, down from five when the history heuristic landed, and what
/// shrank is the region rather than the property.** The pawn-endgame arm
/// rested on the null move being the only rule whose answer depends on the
/// window, so a position where the zugzwang guard refuses it at every node
/// was window-independent in full. That is no longer true and the history
/// table is why: it is written from beta cutoffs, which cutoff happens
/// depends on the window, the sort ranks quiet moves by what was written,
/// and the reduction reads both the rank and the score. So a narrow window
/// and a wide one now build different tables, order their quiet moves
/// differently and reduce differently, on a board the null move never
/// touches.
///
/// Measured on the tree this landed on, both endgames, depths one to
/// seven: they agree through four, the second diverges at five (454
/// against 452 below), both agree again at six, and both diverge at seven.
/// Four is where the last unbroken run ends. **A failure below four is
/// still window arithmetic**, which is the whole of what this gate is for
/// and is what the depth-one arm over the whole sample asserts anyway.
///
/// **Two, down from four when late move pruning landed, and it is the
/// history heuristic's mechanism arriving with a rule that deletes instead
/// of adjusting.** That entry recorded the chain: the table is written from
/// beta cutoffs, which cutoff happens depends on the window, the sort ranks
/// quiet moves by what was written, and something downstream reads the
/// rank. The reduction reads the rank and shortens a search by a ply; this
/// rule reads the rank and removes the move. So a one-place difference in
/// rank between a narrow window and a wide one is now the difference
/// between a move being searched and not existing, and the killers carry
/// the same dependence a step earlier, since a killer is exempt here and a
/// killer is written from a cutoff.
///
/// Measured on the tree this landed on, both endgames, depths one to
/// eight: they agree through two, the first diverges at three (444 against
/// 448 full) and at every depth above it, and the second agrees through
/// four and diverges at five. Two is where the last unbroken run ends.
/// **The region has now gone five, four, two over three changes**, and what
/// is left is one ply above the depth-one arm that holds by construction.
/// The next rule to read the sort's rank closes it, and the honest reading
/// at that point is that the pawn-endgame arm has become a second copy of
/// the depth-one arm rather than a gate of its own.
const BRACKET_DEPTH: u32 = 2;

fn no_table() -> Table {
    Table::with_buckets(0).expect("a table of no buckets")
}

/// A window that brackets the value returns that value, whichever side of
/// it the window sits on -- wherever the search is still a function of the
/// position and the depth alone.
///
/// **This is the property every null-window search rests on.** Alpha-beta
/// is fail-soft here, so a search that fails high returns a lower bound on
/// the value and one that fails low returns an upper bound. Ask a window of
/// one point either side of a value `v` obtained from the full window and
/// both bounds are pinned to it: `(v - 1, v)` fails high, so what it
/// returns is at least `v` and at most the value, which is `v`; `(v, v + 1)`
/// fails low, so what it returns is at most `v` and at least the value.
/// Both come back equal to `v`, and neither had to search what a full
/// window searches to say so.
///
/// A null window that is off by one, inverted, or negated on the wrong
/// side of the recursion breaks this, and it breaks it at depth one on
/// positions with nothing interesting in them. What it cannot see is
/// **which** window a search chooses to ask: that is the coverage gate at
/// the end of this section.
///
/// **Two rules have shrunk where this holds, and the gate is scoped to
/// where it does rather than retired.** Null-move pruning reads beta, so
/// past depth one a node's value depends on the window it was asked in
/// and the two sides of this assertion legitimately diverge (the first
/// divergence observed was two points, at depth four, on a quiet
/// middlegame). The history heuristic then took the exemption that
/// survived that: a table written from cutoffs is written differently
/// under a different window, the sort reads it and the reduction reads
/// both the sort's rank and the score, so a pawn endgame the null move
/// never touches is window-dependent through the ordering instead. What
/// is left exact is exactly what is asserted: every position at depth
/// one, where no node below the root exists for either rule to fire at,
/// which is the depth the arithmetic faults break at anyway; and the
/// pawn-and-king positions to [`BRACKET_DEPTH`], which is where the last
/// unbroken run ends and which that constant records. A failure here is
/// window arithmetic, not pruning and not ordering.
///
/// With no table, deliberately. An entry stored by one of these searches
/// and read by the next would let a narrow window answer from a wide one's
/// work, which is a legitimate thing for a table to do and would make this
/// gate agree with itself for the wrong reason.
#[test]
fn a_window_that_brackets_the_value_returns_the_value() {
    let stop = AtomicBool::new(false);
    for (fen, depths) in sample().into_iter().map(|f| (f, 1..=1)).chain(
        support::PAWN_ENDGAMES
            .into_iter()
            .map(|f| (f.to_string(), 1..=BRACKET_DEPTH)),
    ) {
        let mut b = board(&fen);
        for depth in depths {
            let tt = no_table();
            let full = Search::new(Limits::default(), &stop, &tt).node(&mut b, depth, 0);
            let below = Search::new(Limits::default(), &stop, &tt).node_window(
                &mut b,
                depth,
                0,
                full - 1,
                full,
            );
            let above = Search::new(Limits::default(), &stop, &tt).node_window(
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

/// The move and the score a full-window search returns, position by
/// position, as a fixture.
///
/// **A narrower window may not change the answer.** The window a search
/// asks in is an argument about what it can afford to ignore, and the value
/// at the root is not one of the things it may ignore: a search that comes
/// back with a different move or a different score is not a cheaper search,
/// it is a wrong one. `the_sort_changes_no_score` in `tests/ordering.rs`
/// makes the same argument for an ordering change over its own set; this is
/// the move as well as the score, and it is here because the window is the
/// search's own arithmetic rather than the picker's.
///
/// The fixture was measured over the positions of `sample()` at
/// `WINDOW_DEPTH` with a table of no buckets, first on the tree that
/// searched every move with the full window, and re-measured when
/// null-move pruning landed, and again when late move reductions did.
/// **It is not re-baselined by a windowing change.** An extension changes
/// the tree the value is of and may move a score (`tests/ordering.rs`
/// records the one time that was the right answer); a pruning rule that
/// reads beta does the same, which is what moved two scores here by two
/// points each with every move unchanged; a reduction searches parts of
/// the tree shallower and moved scores and moves both; a window changes
/// which parts of a fixed tree have to be visited to establish the same
/// value, and nothing else. If this array has to move, what moved is not
/// a window.
#[test]
fn a_narrower_window_returns_the_same_move_and_the_same_score() {
    let tt = no_table();
    let stop = AtomicBool::new(false);
    let got: Vec<(String, Score)> = sample()
        .iter()
        .map(|fen| {
            let mut b = board(fen);
            let mut s = Search::new(Limits::depth(WINDOW_DEPTH), &stop, &tt);
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

/// What `sample()` answers at `WINDOW_DEPTH` with no table, measured on
/// the tree the history heuristic left. Against the full-window-everywhere
/// tree it was first measured on, the null-move landing moved two scores
/// by two points with every move unchanged; the reductions moved five
/// scores by two to nine points and three moves, which is what a rule
/// that searches parts of the tree shallower does to the value the tree
/// has -- the extension's re-baseline in `tests/ordering.rs` is the same
/// event in the other direction. The root's own moves are never reduced,
/// so a move that changed did so because a subtree's value moved, not
/// because it was searched at less than its depth.
///
/// **The history heuristic moved four scores by two to twelve points and
/// one move, and it is the first re-baseline here caused by an ordering
/// change.** Ordering used to be the thing this fixture was safe against:
/// a different order over the same tree finds the same value. It stopped
/// being safe when the reduction started reading the index the sort
/// assigns, which is what `tests/ordering.rs` records at
/// `the_sort_changes_no_score`, and this is that consequence arriving in
/// a second gate. The one move that moved (`e1d3` for `d1e3`) is a
/// sibling at the root scoring within two points of it.
///
/// **Futility pruning moved two scores and one move, and it is the
/// smallest re-baseline this fixture has had.** The rule reads alpha, so
/// it is window-dependent in the way the null move is beta-dependent, and
/// a node that skips a quiet move returns a value the full move list
/// would not have: `d1c3` at 6 replaces `d1e3` at 8, again a root sibling
/// within two points, and `e1d3` rose from 8 to 10 two positions later,
/// which is the same effect one ply further up with the sign turned over.
/// Eleven of the fourteen entries did not move at all.
///
/// **Reverse futility moved two scores and one move, and the move is an
/// underpromotion, which is worth the paragraph it takes to say why that
/// is not a defect.** `d7c8q` at 504 becomes `d7c8r` at 516 in pos5, where
/// a pawn on d7 promotes by capturing on c8. That square is defended twice,
/// by the queen on d8 and the knight on b8, so the promoted piece is taken
/// at once whichever piece it is and **the two promotions search to the
/// same value**: the choice between them is a tie-break, not a material
/// judgement. Measured across depths one to twelve, the base build already
/// prefers the rook at depths one to four and switches to the queen at
/// five; the shipped build switches at seven **with a table** and not at
/// all without one, and this fixture is the no-table configuration. So the
/// rule delays a tie-break in a configuration the engine never plays in,
/// and in the one it does play in the two builds agree from depth seven.
/// Read a promotion piece moving here as a tie until the two moves are
/// shown to search to different values.
///
/// The other entry is `e1d3` falling from 10 back to 8, which is the value
/// it carried before futility pruning raised it, in the same position.
/// Twelve of the fourteen entries did not move.
///
/// **Late move pruning moved every one of the fourteen scores and five of
/// the moves, which is by far the largest re-baseline this fixture has
/// had** -- against the reductions' five scores and three moves, and the
/// three re-baselines since that moved two entries each. It is the right
/// size for what the rule is: the reduction searched a late quiet move at
/// less than its depth and this one does not search it at all, so the
/// value of nearly every subtree in the sample changes rather than a few.
/// A fixture that moved a little would be the thing to look at here.
///
/// **The scores move away from zero as often as toward it and by a great
/// deal more than before**: `d5e6` from -18 to -109, `c4c5` from -504 to
/// -613, `b4f4` from 37 to 116. Those are not tie-breaks and no claim is
/// made that they are better answers; they are the values a smaller tree
/// has, and whether a smaller tree is worth its lost accuracy is the SPRT's
/// question and not this gate's. What this gate still refuses is a *window*
/// moving them, which is unchanged: the array is re-measured on the shipped
/// full-window build, as every re-baseline above was.
const FULL_WINDOW_ANSWERS: [(&str, Score); 14] = [
    ("d2d4", 10),
    ("d5e6", -109),
    ("b4f4", 116),
    ("c4c5", -613),
    ("d7c8r", 491),
    ("g5f6", 108),
    ("d2d4", 10),
    ("e1d3", 10),
    ("e1f3", 0),
    ("e1d3", 10),
    ("h1h7", 527),
    ("f1h1", 522),
    ("f1e2", 527),
    ("a1a7", 529),
];

/// End to end: the narrower window has to be worth nodes.
///
/// The same positions at the same depth with no table, so the window is the
/// only thing that can move the count: with no table there is no probe to
/// answer a re-search from, and the ordering, the killers and the extension
/// are what they were. A windowing that is correct at every node and is
/// never asked for passes both gates above and fails this one.
///
/// Measured when this gate was written: 17,337,259 nodes with every move
/// searched in the full window. That figure survived null-move pruning,
/// because the pruning fires only inside a null window: a build that asks
/// every question with the full window has no null move either, so the
/// ceiling still separates the tree from its counterfactual rather than
/// from a counterfactual that no longer exists. Late move reductions sit
/// inside the same null-window question, so the same argument carries
/// them: the full-window counterfactual has no reductions to lose.
#[test]
fn the_narrower_window_saves_nodes() {
    let tt = no_table();
    let stop = AtomicBool::new(false);
    let mut total = 0u64;
    for fen in sample() {
        let mut b = board(&fen);
        let mut s = Search::new(Limits::depth(WINDOW_DEPTH), &stop, &tt);
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

/// An engine player at fixed depth, with the nodes it searched.
fn engine_player(depth: u32, nodes: &mut u64) -> impl FnMut(&mut Board) -> Move + '_ {
    move |b: &mut Board| {
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

/// A hundred games, fifty as each colour, at depth three: a few seconds in
/// the dev profile. `beats_a_random_mover_at_depth_five` is the same at a
/// depth closer to how the engine plays, ignored, run by hand in release
/// and recorded.
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

/// `games` games against a random mover, alternating colours, the engine at
/// `depth`. Returns wins, games, total plies.
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
