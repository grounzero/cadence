// SPDX-License-Identifier: GPL-3.0-or-later

//! The quiescence search: what the horizon looks like once it is quiet.
//!
//! A search that stops at a fixed depth and calls the static evaluation
//! evaluates positions in the middle of capture sequences, and plays
//! accordingly: it takes a defended pawn with the queen at the last ply
//! because the recapture is one ply beyond the horizon. Quiescence resolves
//! the captures, promotions and checks at the horizon before evaluating, so
//! that what the evaluation sees is a position nobody is about to win
//! material in.
//!
//! None of this proves the search is *right*. Quiescence has no perft: a
//! stand-pat rule that is slightly off, a noisy set that is slightly wrong,
//! an in-check node that is allowed to stand pat, all play legal chess,
//! pass every test that does not know the answer, and gain less than they
//! should. What a gate can do is pin the behaviours that define the thing,
//! each observable at depth one, where the old search and the new one
//! differ in a way that needs no opponent to see: a capture refuted by an
//! immediate recapture is not played; a piece attacked at the root is not
//! left to be taken; a losing capture is never forced on the side to move,
//! so a quiet horizon scores its static evaluation and costs one node; a
//! check at the horizon is answered, not ignored, so a mate in one is found
//! at depth one and a skewer through a check is seen; a promotion at the
//! horizon is seen; and the tree below depth one is bounded. Every
//! constructed position is checked for what it claims before the engine is
//! asked, and every one is asked in both colours.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::types::PromoPiece;
use cadence_core::{Colour, Move, PieceType, START_FEN, generate_legal, generate_noisy, parse_uci};
use cadence_engine::eval;
use cadence_engine::picker::{capture_key, noisy_key, sort_noisy};
use cadence_engine::score::{self, Score, mate_in};
use cadence_engine::search::{Limits, Search};
use cadence_engine::see::see;
use support::table;

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

/// A UCI move under the colour mirror: the ranks flip, the files stay.
fn mirror_uci(uci: &str) -> String {
    uci.chars()
        .map(|c| match c {
            '1'..='8' => char::from(b'9' - (c as u8 - b'0')),
            other => other,
        })
        .collect()
}

/// A position and a move of interest, in both colours.
fn both_colours(fen: &str, uci: &str) -> [(String, String); 2] {
    [
        (fen.to_string(), uci.to_string()),
        (support::mirror_fen(fen), mirror_uci(uci)),
    ]
}

/// Classical piece values, for the in-test material arithmetic only.
fn value(pt: PieceType) -> i32 {
    match pt {
        PieceType::Pawn => 1,
        PieceType::Knight | PieceType::Bishop => 3,
        PieceType::Rook => 5,
        PieceType::Queen => 9,
        PieceType::King => 0,
    }
}

/// `c`'s material less the other side's.
fn balance(b: &Board, c: Colour) -> i32 {
    let mut total = 0;
    for pt in PieceType::ALL {
        let v = value(pt);
        total += v * i32::try_from(b.pieces(c, pt).count()).expect("fits");
        total -= v * i32::try_from(b.pieces(c.flip(), pt).count()).expect("fits");
    }
    total
}

/// The side to move's legal captures.
fn captures(b: &Board) -> Vec<Move> {
    generate_legal(b)
        .iter()
        .filter(|m| m.is_capture())
        .collect()
}

/// The side to move's legal captures of a piece of type `pt`.
fn captures_of(b: &Board, pt: PieceType) -> Vec<Move> {
    captures(b)
        .into_iter()
        .filter(|m| {
            !m.is_en_passant() && b.piece_at(m.to_sq()).is_some_and(|p| p.piece_type() == pt)
        })
        .collect()
}

/// Whether the side to move's piece of type `pt` could be captured if it
/// passed: the opponent's captures of it after a null move.
fn en_prise(b: &mut Board, pt: PieceType) -> bool {
    b.make_null_move();
    let hit = !captures_of(b, pt).is_empty();
    b.unmake_null_move();
    hit
}

/// Whether `bad`, a capture, loses material to the worst recapture on its
/// destination square: the mover's balance after the recapture is below
/// its balance before the capture.
fn loses_material_to_a_recapture(b: &mut Board, bad: Move) -> bool {
    assert!(bad.is_capture(), "{bad:?} is not a capture");
    let us = b.side_to_move();
    let before = balance(b, us);
    b.make_move(bad);
    let mut worst = i32::MAX;
    for r in captures(b) {
        if r.to_sq() == bad.to_sq() {
            b.make_move(r);
            worst = worst.min(balance(b, us));
            b.unmake_move(r);
        }
    }
    b.unmake_move(bad);
    worst < before
}

/// Whether the side to move has a move after which the opponent has no
/// capture at all.
fn has_a_move_allowing_no_capture(b: &mut Board) -> bool {
    let legal = generate_legal(b);
    legal.iter().any(|m| {
        b.make_move(m);
        let none = captures(b).is_empty();
        b.unmake_move(m);
        none
    })
}

/// The side to move's best static score after one move: the depth-one
/// score of a search whose horizon is quiet.
fn best_static_reply(b: &mut Board) -> Score {
    let mut best = Score::MIN;
    for m in generate_legal(b).iter() {
        b.make_move(m);
        best = best.max(-eval::evaluate(b));
        b.unmake_move(m);
    }
    best
}

/// How many root moves the null window costs a second search.
///
/// Every root move behind the first is searched in a window with no room
/// in it for an answer better than the move in hand, and a move that
/// beats it comes back with a bound rather than a value and is searched
/// again with the full window. So a leaf under such a move is visited
/// twice, and the count is the number of root moves that are strictly
/// better than everything tried before them.
///
/// Only sound where the value of a root move is minus the static
/// evaluation of the position it leads to, which is what a quiet horizon
/// means: the callers below each establish that before using this.
fn root_re_searches(b: &mut Board) -> u64 {
    let mut best = Score::MIN;
    let mut re_searched = 0;
    for (i, m) in generate_legal(b).iter().enumerate() {
        b.make_move(m);
        let v = -eval::evaluate(b);
        b.unmake_move(m);
        if i > 0 && v > best {
            re_searched += 1;
        }
        best = best.max(v);
    }
    re_searched
}

// ---------------------------------------------------------------------------
// Captures at the horizon
// ---------------------------------------------------------------------------

/// A capture that is refuted by an immediate recapture -- the capturer is
/// worth more than what it takes -- with a quiet alternative available.
/// Each is a pawn or piece defended once, taken by something bigger.
const LOSING_CAPTURES: &[(&str, &str)] = &[
    // Qxd5, a pawn defended by a pawn.
    ("6k1/8/4p3/3p4/8/8/8/3Q2K1 w - - 0 1", "d1d5"),
    // Rxe5, a knight defended by a bishop.
    ("6k1/2b5/8/4n3/8/8/8/4R1K1 w - - 0 1", "e1e5"),
    // Bxe5, a pawn defended by a knight.
    ("6k1/8/2n5/4p3/8/8/1B6/6K1 w - - 0 1", "b2e5"),
    // Nxe5, a pawn defended by a pawn.
    ("6k1/8/3p4/4p3/8/5N2/8/6K1 w - - 0 1", "f3e5"),
    // Qxf7+, a pawn defended by the king: the refutation is an evasion.
    ("6k1/5p2/8/7Q/8/8/8/6K1 w - - 0 1", "h5f7"),
];

#[test]
fn the_losing_captures_are_what_they_claim() {
    for (fen, bad) in LOSING_CAPTURES {
        for (fen, bad) in both_colours(fen, bad) {
            let mut b = board(&fen);
            let bad = mv(&b, &bad);
            assert!(
                loses_material_to_a_recapture(&mut b, bad),
                "{fen}: {bad:?} is not refuted by a recapture"
            );
            assert!(
                has_a_move_allowing_no_capture(&mut b),
                "{fen}: no quiet alternative"
            );
        }
    }
}

/// At depth one the old search takes the material and stops; with the
/// captures resolved at the horizon the recapture is seen and the capture
/// is not played. Deeper searches see it either way, and must still not.
#[test]
fn a_capture_refuted_by_an_immediate_recapture_is_not_played() {
    for (fen, bad) in LOSING_CAPTURES {
        for (fen, bad) in both_colours(fen, bad) {
            let mut b = board(&fen);
            let bad = mv(&b, &bad);
            // Standing pat is a lower bound for the side to move at the
            // horizon, so at depth one no reply scores above its static
            // worth and the root never scores above its best static reply:
            // the capture is not counted as having won anything.
            let ceiling = best_static_reply(&mut b);
            for depth in 1..=3 {
                let r = search(&mut b, Limits::depth(depth));
                assert_ne!(r.best, bad, "{fen}: depth {depth} played {bad:?}");
                if depth == 1 {
                    assert!(
                        r.score <= ceiling,
                        "{fen}: depth 1 scores {} above the best static reply {ceiling}",
                        r.score
                    );
                }
            }
        }
    }
}

/// A piece attacked at the root, with a safe square to go to, and a decoy:
/// a knight on the rim whose centralising move is the best thing the
/// piece-square tables can see, which is what the old search plays.
const ATTACKED_PIECES: &[(&str, PieceType)] = &[
    // The queen on d4 is attacked by the knight; Nb1-c3 is the decoy.
    ("6k1/8/2n5/8/3Q4/8/8/1N4K1 w - - 0 1", PieceType::Queen),
    // The rook on d5 is attacked by the bishop; the same decoy.
    ("6k1/1b6/8/3R4/8/8/8/1N4K1 w - - 0 1", PieceType::Rook),
];

#[test]
fn the_attacked_pieces_are_what_they_claim() {
    for (fen, pt) in ATTACKED_PIECES {
        for (fen, _) in both_colours(fen, "a1a1") {
            let mut b = board(&fen);
            assert!(en_prise(&mut b, *pt), "{fen}: the {pt:?} is not attacked");
            // A move after which it is safe exists, and so does one after
            // which it is not: the choice is real.
            let mut safe = 0;
            let mut unsafe_ = 0;
            for m in generate_legal(&b).iter() {
                b.make_move(m);
                if captures_of(&b, *pt).is_empty() {
                    safe += 1;
                } else {
                    unsafe_ += 1;
                }
                b.unmake_move(m);
            }
            assert!(
                safe > 0 && unsafe_ > 0,
                "{fen}: {safe} safe, {unsafe_} unsafe"
            );
        }
    }
}

#[test]
fn a_piece_attacked_at_the_root_is_not_left_to_be_taken() {
    for (fen, pt) in ATTACKED_PIECES {
        for (fen, _) in both_colours(fen, "a1a1") {
            let mut b = board(&fen);
            for depth in 1..=3 {
                let r = search(&mut b, Limits::depth(depth));
                b.make_move(r.best);
                let taken = captures_of(&b, *pt);
                b.unmake_move(r.best);
                assert!(
                    taken.is_empty(),
                    "{fen}: depth {depth} played {:?} and the {pt:?} is taken by {taken:?}",
                    r.best
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Standing pat
// ---------------------------------------------------------------------------

/// White has three king moves and nothing else; after each of them Black's
/// only capture is Qxb3, a pawn defended by a pawn, which loses the queen.
/// The side to move at the horizon is never obliged to capture: the
/// position stands on its static evaluation, so the depth-one score is the
/// best static reply -- exactly what a search with a quiet horizon gives --
/// while the node count shows the losing capture was looked at.
const STAND_PAT: &str = "7k/5q2/8/8/1p6/pP6/P7/7K w - - 0 1";

#[test]
fn the_stand_pat_position_is_what_it_claims() {
    for (fen, _) in both_colours(STAND_PAT, "a1a1") {
        let mut b = board(&fen);
        let legal = generate_legal(&b);
        assert!(legal.len() > 1, "{fen}: {} moves", legal.len());
        for m in legal.iter() {
            assert!(!m.is_capture(), "{fen}: {m:?} is a capture");
            b.make_move(m);
            let caps = captures(&b);
            assert_eq!(caps.len(), 1, "{fen}: after {m:?}, captures {caps:?}");
            assert!(
                loses_material_to_a_recapture(&mut b, caps[0]),
                "{fen}: after {m:?}, {:?} does not lose material",
                caps[0]
            );
            b.unmake_move(m);
        }
    }
}

#[test]
fn a_losing_capture_is_never_forced_on_the_side_to_move_at_the_horizon() {
    for (fen, _) in both_colours(STAND_PAT, "a1a1") {
        let mut b = board(&fen);
        let expected = best_static_reply(&mut b);
        let roots = generate_legal(&b).len() as u64;
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.score, expected, "{fen}: score {}", r.score);
        // This gate used to require more than `1 + roots` nodes as well, to
        // show the losing capture had been looked at and refused. The
        // exchange evaluation now refuses it without looking, which is the
        // gate in the section on losing captures below; the property here
        // is the score, and it is unchanged.
        let _ = roots;
    }
}

/// Where no root move allows a capture or a promotion, the horizon is
/// already quiet: the search costs one node per leaf and scores the best
/// static reply.
///
/// **One node per leaf, and one more for each leaf the null window has to
/// visit twice.** The root searches its first move in the full window and
/// the rest in a window with no room in it for a better answer, so a root
/// move that turns out to be better comes back with a bound and is
/// searched again. Here every leaf is a single node -- there is nothing
/// noisy to resolve under any of them -- so the whole of the count is
/// arithmetic: the root, one node per root move, and one more for each
/// root move that beat everything before it. `root_re_searches` computes
/// the last term from the static evaluations, which is what those moves
/// are worth at this depth, so this gate pins the re-search rule as well
/// as the cost of a quiet horizon. It read `1 + legal.len()` before the
/// null window existed.
#[test]
fn a_quiet_horizon_costs_one_node_per_leaf_and_scores_the_static_evaluation() {
    let mut fens = vec![START_FEN.to_string()];
    fens.extend(
        support::dfrc_arrays()
            .into_iter()
            .take(6)
            .map(|(_, _, f)| f),
    );
    for fen in fens {
        let mut b = board(&fen);
        let legal = generate_legal(&b);
        // The premise, checked: nothing noisy is available after any move.
        for m in legal.iter() {
            b.make_move(m);
            let noisy: Vec<Move> = generate_legal(&b).iter().filter(|m| m.is_noisy()).collect();
            assert!(noisy.is_empty(), "{fen}: after {m:?}, noisy {noisy:?}");
            b.unmake_move(m);
        }
        let expected = best_static_reply(&mut b);
        let re_searched = root_re_searches(&mut b);
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.nodes, 1 + legal.len() as u64 + re_searched, "{fen}");
        assert_eq!(r.score, expected, "{fen}");
    }
}

/// Kiwipete has eight captures at the root and plenty below: a search that
/// resolves them visits more than the root and its children at depth one.
#[test]
fn a_noisy_horizon_is_searched_below_depth_one() {
    let fen = support::standard_fen("kiwipete");
    let mut b = board(&fen);
    let roots = generate_legal(&b).len() as u64;
    let r = search(&mut b, Limits::depth(1));
    assert_eq!(r.depth, 1);
    assert!(
        r.nodes > 1 + roots,
        "{} nodes for {roots} root moves",
        r.nodes
    );
}

// ---------------------------------------------------------------------------
// Checks at the horizon
// ---------------------------------------------------------------------------

/// A side in check at the horizon may not stand pat -- it has to get out of
/// check, and if it cannot it is mated. So a mate in one is found at depth
/// one: the mating move is the root move, and the horizon below it is a
/// position with no evasion.
#[test]
fn mate_in_one_is_found_at_depth_one() {
    for (fen, key) in both_colours("7k/8/6K1/8/8/8/8/1R6 w - - 0 1", "b1b8") {
        let mut b = board(&fen);
        let key = mv(&b, &key);
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.best, key, "{fen}: played {:?}", r.best);
        assert_eq!(r.score, mate_in(1), "{fen}: score {}", r.score);
        assert_eq!(score::uci(r.score), "mate 1", "{fen}");
        assert_eq!(r.pv, vec![key], "{fen}: pv {:?}", r.pv);
    }
}

/// Ra8+ skewers the king and the queen: every evasion is a quiet king move,
/// and after each the rook takes the queen. A horizon that lets the side
/// in check stand pat, or answers a check with captures only, scores Ra8+
/// as nothing; one that answers it with the evasions sees the queen go.
/// The old search prefers Ra7, for the seventh rank.
const SKEWER: &str = "4k2q/8/8/8/8/8/8/R5K1 w - - 0 1";

#[test]
fn the_skewer_is_what_it_claims() {
    for (fen, key) in both_colours(SKEWER, "a1a8") {
        let mut b = board(&fen);
        let key = mv(&b, &key);
        b.make_move(key);
        assert!(b.in_check(), "{fen}: {key:?} is not check");
        let evasions = generate_legal(&b);
        assert_eq!(
            evasions.len(),
            3,
            "{fen}: evasions {:?}",
            evasions.as_slice()
        );
        for e in evasions.iter() {
            assert!(!e.is_capture(), "{fen}: evasion {e:?} is a capture");
            b.make_move(e);
            assert_eq!(
                captures_of(&b, PieceType::Queen).len(),
                1,
                "{fen}: after {e:?}, no capture of the queen"
            );
            b.unmake_move(e);
        }
        b.unmake_move(key);
    }
}

#[test]
fn a_check_at_the_horizon_is_answered_with_every_evasion() {
    for (fen, key) in both_colours(SKEWER, "a1a8") {
        let mut b = board(&fen);
        let key = mv(&b, &key);
        for depth in 1..=2 {
            let r = search(&mut b, Limits::depth(depth));
            assert_eq!(r.best, key, "{fen}: depth {depth} played {:?}", r.best);
            assert!(r.score > 500, "{fen}: depth {depth} scores {}", r.score);
        }
    }
}

// ---------------------------------------------------------------------------
// Promotions at the horizon
// ---------------------------------------------------------------------------

/// Black's pawn on a2 promotes next move unless White covers a1. A
/// horizon that does not see promotions plays Rh7 for the seventh rank and
/// meets a queen; one that does plays a rook move after which every
/// promotion is captured. The one check available, Rh8+, hangs the rook to
/// the king, so a horizon that sees the promotion cannot push it out of
/// sight with a check either.
const PROMOTION: &str = "6k1/6p1/8/8/2b5/7R/p7/6K1 w - - 0 1";

/// Whether every promotion the opponent has is met by a capture of the
/// promoted piece.
fn every_promotion_is_captured(b: &mut Board) -> bool {
    let promotions: Vec<Move> = generate_legal(b)
        .iter()
        .filter(|m| m.is_promotion())
        .collect();
    !promotions.is_empty()
        && promotions.iter().all(|&p| {
            b.make_move(p);
            let met = captures(b).iter().any(|c| c.to_sq() == p.to_sq());
            b.unmake_move(p);
            met
        })
}

#[test]
fn the_promotion_position_is_what_it_claims() {
    for (fen, _) in both_colours(PROMOTION, "a1a1") {
        let mut b = board(&fen);
        let mut covered = 0;
        let mut open = 0;
        let mut checks = 0;
        for m in generate_legal(&b).iter() {
            b.make_move(m);
            if b.in_check() {
                // Answering the check comes before promoting; the only
                // check there is loses the rook to the king.
                checks += 1;
                assert_eq!(
                    captures_of(&b, PieceType::Rook).len(),
                    1,
                    "{fen}: the check {m:?} does not hang the rook"
                );
            } else {
                let promotions = generate_legal(&b)
                    .iter()
                    .filter(|m| m.is_promotion())
                    .count();
                assert!(promotions > 0, "{fen}: after {m:?} no promotion");
                if every_promotion_is_captured(&mut b) {
                    covered += 1;
                } else {
                    open += 1;
                }
            }
            b.unmake_move(m);
        }
        assert_eq!(checks, 1, "{fen}: {checks} checks");
        assert!(
            covered > 0 && open > 0,
            "{fen}: {covered} covered, {open} open"
        );
    }
}

#[test]
fn a_promotion_at_the_horizon_is_seen() {
    for (fen, _) in both_colours(PROMOTION, "a1a1") {
        let mut b = board(&fen);
        for depth in 1..=2 {
            let r = search(&mut b, Limits::depth(depth));
            b.make_move(r.best);
            let met = !b.in_check() && every_promotion_is_captured(&mut b);
            b.unmake_move(r.best);
            assert!(met, "{fen}: depth {depth} played {:?}", r.best);
        }
    }
}

// ---------------------------------------------------------------------------
// The order the noisy moves are tried in
// ---------------------------------------------------------------------------

/// Most valuable victim first, least valuable attacker among equal
/// victims, both ranked pawn, knight, bishop, rook, queen, with the king an
/// attacker only. The key is what the quiescence search sorts by, so its
/// order is part of what the bench number depends on.
#[test]
fn the_capture_key_ranks_the_victim_first_and_the_attacker_second() {
    let victims = [
        PieceType::Pawn,
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
    ];
    let attackers = [
        PieceType::Pawn,
        PieceType::Knight,
        PieceType::Bishop,
        PieceType::Rook,
        PieceType::Queen,
        PieceType::King,
    ];
    // A more valuable victim beats a less valuable one whoever takes it.
    for (i, &v) in victims.iter().enumerate() {
        for &w in &victims[..i] {
            for &a in &attackers {
                for &b in &attackers {
                    assert!(
                        capture_key(a, v) > capture_key(b, w),
                        "{a:?}x{v:?} should come before {b:?}x{w:?}"
                    );
                }
            }
        }
    }
    // Among equal victims, the cheaper attacker first.
    for &v in &victims {
        for (i, &a) in attackers.iter().enumerate() {
            for &b in &attackers[..i] {
                assert!(
                    capture_key(b, v) > capture_key(a, v),
                    "{b:?}x{v:?} should come before {a:?}x{v:?}"
                );
            }
        }
    }
}

/// The key of a move in a position: a capture reads its attacker and victim
/// off the board, en passant is a pawn taking a pawn, a queen promotion
/// outranks every capture and an underpromotion ranks below every one, and
/// a promotion that captures keeps the capture's order within its class.
#[test]
fn the_noisy_key_reads_the_board() {
    // Kiwipete's eight captures, each named.
    let b = board(&support::standard_fen("kiwipete"));
    for (uci, attacker, victim) in [
        ("e5g6", PieceType::Knight, PieceType::Pawn),
        ("e5d7", PieceType::Knight, PieceType::Pawn),
        ("e5f7", PieceType::Knight, PieceType::Pawn),
        ("e2a6", PieceType::Bishop, PieceType::Bishop),
        ("f3h3", PieceType::Queen, PieceType::Pawn),
        ("f3f6", PieceType::Queen, PieceType::Knight),
        ("g2h3", PieceType::Pawn, PieceType::Pawn),
        ("d5e6", PieceType::Pawn, PieceType::Pawn),
    ] {
        let m = mv(&b, uci);
        assert!(m.is_capture(), "{uci}");
        assert_eq!(noisy_key(&b, m), capture_key(attacker, victim), "{uci}");
    }
    // Promotions with and without capture, and en passant.
    let b = board("r3k3/1P6/8/3pP3/8/8/6K1/8 w - d6 0 1");
    let ep = mv(&b, "e5d6");
    assert!(ep.is_en_passant());
    assert_eq!(
        noisy_key(&b, ep),
        capture_key(PieceType::Pawn, PieceType::Pawn)
    );
    let best_capture = capture_key(PieceType::Pawn, PieceType::Queen);
    let worst_capture = capture_key(PieceType::King, PieceType::Pawn);
    for p in PromoPiece::ALL {
        let push = mv(&b, &format!("b7b8{}", p.to_char()));
        let take = mv(&b, &format!("b7a8{}", p.to_char()));
        assert!(take.is_capture() && push.is_promotion() && take.is_promotion());
        if p == PromoPiece::Queen {
            assert!(noisy_key(&b, push) > best_capture, "{push:?}");
            assert!(noisy_key(&b, take) > noisy_key(&b, push), "{take:?}");
        } else {
            assert!(noisy_key(&b, push) < worst_capture, "{push:?}");
            assert!(noisy_key(&b, take) > noisy_key(&b, push), "{take:?}");
            assert!(noisy_key(&b, take) < worst_capture, "{take:?}");
        }
    }
}

/// The noisy list sorted: a permutation of the generated one, keys
/// non-increasing, ties in generation order -- checked in every corpus
/// position, and in Kiwipete the sorted order is written out, because there
/// the generated order is not it.
#[test]
fn the_noisy_moves_are_sorted_by_key_stably() {
    let mut differs = 0;
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let generated: Vec<Move> = generate_legal(&b).iter().filter(|m| m.is_noisy()).collect();
        let mut list = cadence_core::MoveList::new();
        for &m in &generated {
            list.push(m);
        }
        sort_noisy(&b, &mut list);
        let sorted = list.as_slice().to_vec();
        let mut generated_bits = generated.iter().map(|m| m.to_bits()).collect::<Vec<_>>();
        let mut sorted_bits = sorted.iter().map(|m| m.to_bits()).collect::<Vec<_>>();
        generated_bits.sort_unstable();
        sorted_bits.sort_unstable();
        assert_eq!(generated_bits, sorted_bits, "{fen}: not a permutation");
        for w in sorted.windows(2) {
            let (first, second) = (noisy_key(&b, w[0]), noisy_key(&b, w[1]));
            assert!(
                first >= second,
                "{fen}: {:?} ({first}) before {:?} ({second})",
                w[0],
                w[1]
            );
            if first == second {
                let i = generated.iter().position(|&m| m == w[0]).expect("in list");
                let j = generated.iter().position(|&m| m == w[1]).expect("in list");
                assert!(
                    i < j,
                    "{fen}: tie {:?} {:?} out of generation order",
                    w[0],
                    w[1]
                );
            }
        }
        if sorted != generated {
            differs += 1;
        }
    }
    assert!(differs > 0, "no corpus position is reordered");

    let b = board(&support::standard_fen("kiwipete"));
    let mut list = cadence_core::MoveList::new();
    for m in generate_legal(&b).iter().filter(|m| m.is_noisy()) {
        list.push(m);
    }
    sort_noisy(&b, &mut list);
    let order: Vec<String> = list.iter().map(Move::to_uci_chess960).collect();
    assert_eq!(
        order,
        [
            "e2a6", "f3f6", "g2h3", "d5e6", "e5g6", "e5d7", "e5f7", "f3h3"
        ]
    );
}

// ---------------------------------------------------------------------------
// The order the check evasions are tried in
// ---------------------------------------------------------------------------

/// Runs `f` at every in-check position that is one legal move from a corpus
/// position: the move that gave the check, the board it reached, and the
/// evasions generated there, in generation order.
///
/// This is the population the ordering acts on, and it is taken from the
/// corpus rather than constructed, because what the sort is worth depends
/// on how often a check at the horizon has a noisy answer at all. A check
/// that is mate generates nothing and is not one of these positions: the
/// search returns before it reaches the order.
fn for_each_in_check_child(mut f: impl FnMut(&str, Move, &Board, &[Move])) {
    for fen in support::corpus_fens() {
        let mut b = board(&fen);
        for m in generate_legal(&b).iter() {
            b.make_move(m);
            if b.in_check() {
                let evasions = generate_legal(&b);
                if !evasions.is_empty() {
                    f(&fen, m, &b, evasions.as_slice());
                }
            }
            b.unmake_move(m);
        }
    }
}

/// The rank an evasion is required to sort by: its capture key if it is
/// noisy, and one rank below every noisy one if it is quiet. `i32::MIN`
/// rather than `picker::QUIET`, so the assertion says "below all of them"
/// and not "the number the implementation uses".
fn evasion_rank(b: &Board, m: Move) -> i32 {
    if m.is_noisy() {
        noisy_key(b, m)
    } else {
        i32::MIN
    }
}

/// The premise of the change, read off the generator rather than assumed:
/// the evasions come out king moves first, so the capture of the piece
/// giving check is tried after every retreat, and a retreat from a check
/// is what opens the next one.
#[test]
fn the_check_evasions_are_generated_king_first() {
    let (mut lists, mut with_noisy, mut king_first, mut noisy_behind_a_king_move) = (0, 0, 0, 0);
    for_each_in_check_child(|fen, m, b, evasions| {
        lists += 1;
        let king = b
            .pieces(b.side_to_move(), PieceType::King)
            .lsb()
            .expect("a side to move has a king");
        assert_eq!(
            evasions[0].from_sq(),
            king,
            "{fen}: after {}, the first evasion is {:?}",
            m.to_uci_chess960(),
            evasions[0]
        );
        king_first += 1;
        let noisy = evasions.iter().position(|e| e.is_noisy());
        if let Some(i) = noisy {
            with_noisy += 1;
            if i > 0 && evasions[..i].iter().any(|e| e.from_sq() == king) {
                noisy_behind_a_king_move += 1;
            }
        }
    });
    println!(
        "{lists} in-check positions, {king_first} led by a king move, \
         {with_noisy} with a noisy evasion, {noisy_behind_a_king_move} of those behind one"
    );
    assert_eq!(king_first, lists);
    assert!(
        noisy_behind_a_king_move > 0,
        "no corpus check answers a retreat before a capture"
    );
}

/// The order the sort is required to produce over an evasion list, which is
/// the one place the quiescence search sorts a list holding quiet moves:
/// every noisy evasion first by capture key, the quiet ones behind them all
/// in the order the generator emitted them, and nothing gained or lost.
#[test]
fn the_check_evasions_sort_noisy_first_and_keep_generation_order() {
    let mut reordered = 0;
    for_each_in_check_child(|fen, m, b, evasions| {
        let mut list = cadence_core::MoveList::new();
        for &e in evasions {
            list.push(e);
        }
        cadence_engine::picker::sort_from(b, &mut list, 0, [Move::NULL; 2], &[]);
        let sorted = list.as_slice().to_vec();
        let mut before = evasions.iter().map(|e| e.to_bits()).collect::<Vec<_>>();
        let mut after = sorted.iter().map(|e| e.to_bits()).collect::<Vec<_>>();
        before.sort_unstable();
        after.sort_unstable();
        assert_eq!(
            before,
            after,
            "{fen}: {} lost an evasion",
            m.to_uci_chess960()
        );
        for w in sorted.windows(2) {
            let (first, second) = (evasion_rank(b, w[0]), evasion_rank(b, w[1]));
            assert!(
                first >= second,
                "{fen}: {:?} ({first}) before {:?} ({second})",
                w[0],
                w[1]
            );
            if first == second {
                let i = evasions.iter().position(|e| *e == w[0]).expect("in list");
                let j = evasions.iter().position(|e| *e == w[1]).expect("in list");
                assert!(
                    i < j,
                    "{fen}: tie {:?} {:?} out of generation order",
                    w[0],
                    w[1]
                );
            }
        }
        if sorted != evasions {
            reordered += 1;
        }
    });
    println!("{reordered} evasion lists come out in a different order");
    assert!(reordered > 0, "no corpus evasion list is reordered");
}

/// White is in check from the queen on a1 and has exactly one legal move,
/// Ng1, which blocks the check and gives one: the knight lands defended by
/// the king, so the capture that answers it, Qxg1, loses the queen for a
/// knight, and the seven king moves that answer it keep the queen. The
/// generator emits the king moves first and the sort puts the capture in
/// front of them, so this is a position where the sort tries the losing
/// move first.
const DEFENDED_BLOCKER: &str = "8/8/8/8/8/7N/4k1PP/q6K w - - 0 1";

#[test]
fn the_defended_blocker_is_what_it_claims() {
    for (fen, key) in both_colours(DEFENDED_BLOCKER, "h3g1") {
        let mut b = board(&fen);
        assert!(b.in_check(), "{fen}: not in check");
        let legal = generate_legal(&b);
        assert_eq!(legal.len(), 1, "{fen}: {:?}", legal.as_slice());
        let key = mv(&b, &key);
        assert_eq!(legal.as_slice()[0], key, "{fen}");
        b.make_move(key);
        assert!(b.in_check(), "{fen}: {key:?} is not check");
        let evasions = generate_legal(&b);
        let noisy: Vec<Move> = evasions.iter().filter(|e| e.is_noisy()).collect();
        assert_eq!(noisy.len(), 1, "{fen}: noisy evasions {noisy:?}");
        assert_eq!(
            evasions.len(),
            8,
            "{fen}: evasions {:?}",
            evasions.as_slice()
        );
        assert!(
            !evasions.as_slice()[0].is_noisy(),
            "{fen}: the capture is generated first"
        );
        let take = noisy[0];
        assert!(
            take.is_capture() && take.to_sq() == key.to_sq(),
            "{fen}: {take:?}"
        );
        assert!(
            loses_material_to_a_recapture(&mut b, take),
            "{fen}: {take:?} is not refuted"
        );
        b.unmake_move(key);
    }
}

/// The sort reorders the evasions; it does not shorten the list. The one
/// noisy evasion here is tried first and loses a queen, and the value of
/// the position is what the quiet ones are worth, so a search that stopped
/// at the head of the sorted list would score this the other way round.
///
/// It costs the same eleven nodes at depth one in either order, because
/// every evasion is searched in either order and none of the eight has a
/// capture under it. That is the point: this is a claim about what the
/// node is worth, not about what it costs.
#[test]
fn a_noisy_evasion_that_loses_is_not_the_answer() {
    for (fen, key) in both_colours(DEFENDED_BLOCKER, "h3g1") {
        let mut b = board(&fen);
        let key = mv(&b, &key);
        for depth in 1..=3 {
            let r = search(&mut b, Limits::depth(depth));
            assert_eq!(r.best, key, "{fen}: depth {depth} played {:?}", r.best);
            assert!(
                r.score < -300,
                "{fen}: depth {depth} scores {}, so the queen was taken",
                r.score
            );
        }
    }
}

/// The end-to-end gate: the same corpus positions the depth-one bound below
/// uses, at depth two, where a check at the horizon has a tree under it.
/// This is the one that fails if the sort is never called.
///
/// Depth two rather than depth one, which the bound below already runs:
/// the two answer the same way (249,995 nodes against 54,702 at depth one)
/// and the deeper one puts a tree under more of the checks.
///
/// Measured over the corpus at depth two: 306,698 nodes with the evasions
/// in generation order and 68,845 with them ordered, the worst position
/// being Kiwipete either way (294,587 and 59,825). The ceiling is half the
/// first, which is more than twice the second: it is a coverage assertion,
/// not a size claim, and anything that reorders the evasions clears it by
/// a wide margin while anything that does not fails it by a wide one.
const CORPUS_DEPTH_TWO_NODE_CEILING: u64 = 150_000;

#[test]
fn ordering_the_check_evasions_saves_nodes() {
    let mut total = 0;
    let mut worst = (0u64, String::new());
    for fen in support::corpus_fens() {
        let mut b = board(&fen);
        if generate_legal(&b).is_empty() {
            continue;
        }
        let r = search(&mut b, Limits::depth(2));
        total += r.nodes;
        if r.nodes > worst.0 {
            worst = (r.nodes, fen.clone());
        }
    }
    println!(
        "corpus at depth two: {total} nodes, worst {} in {}",
        worst.0, worst.1
    );
    assert!(
        total < CORPUS_DEPTH_TWO_NODE_CEILING,
        "{total} nodes over the corpus at depth two"
    );
}

// ---------------------------------------------------------------------------
// Losing captures are not searched
// ---------------------------------------------------------------------------
//
// Out of check, the quiescence search skips a noisy move whose static
// exchange value (`see::see`) is negative: a capture that loses material
// once every recapture has been answered is refused without being searched.
// In check nothing is skipped, because an evasion is a legal answer to the
// check and skipping one is skipping an answer.
//
// What a gate can see: a node count, and the sign the rule reads. A depth-one
// search from a position whose root moves are all quiet costs one node per
// root move plus the root when every reply below the horizon is refused,
// and more when one is searched. So the gates are three positions of one
// shape, the only noisy reply losing, winning and even (exactly `1 + roots`
// nodes, more, and more), with the sign the rule reads asserted from `see`
// before the search is asked. What `see` itself gets right -- pins, x-rays,
// discovered checks -- is that function's gate, `tests/see.rs`, and not
// repeated here.

/// `see` of the one noisy reply the side to move has after each of the root
/// side's quiet moves, asserted to be the same move and the same value
/// after every one of them, and that value returned. A noisy root move is
/// the main search's business and is skipped.
fn the_only_reply_exchange(b: &mut Board, uci: &str) -> i32 {
    let root = generate_legal(b);
    let mut value = None;
    for m in root.iter().filter(|m| !m.is_noisy()) {
        b.make_move(m);
        let noisy = generate_noisy(b);
        assert_eq!(
            noisy.len(),
            1,
            "{}: after {}, noisy replies {:?}",
            fen(b),
            m.to_uci_chess960(),
            noisy.iter().map(Move::to_uci_chess960).collect::<Vec<_>>()
        );
        let reply = noisy.as_slice()[0];
        assert_eq!(reply.to_uci_chess960(), uci, "{}", fen(b));
        let v = see(b, reply);
        b.unmake_move(m);
        if let Some(prev) = value {
            assert_eq!(
                prev,
                v,
                "{}: the exchange value depends on the root move",
                fen(b)
            );
        }
        value = Some(v);
    }
    value.expect("a quiet root move")
}

fn fen(b: &Board) -> String {
    b.to_fen(cadence_core::fen::FenStyle::Shredder)
}

/// The stand-pat position: after every white king move Black's only noisy
/// move is Qxb3, a pawn defended by a pawn. Refused without being searched:
/// one node per root move, and the score is the best static reply.
///
/// The refusal is what makes each leaf a single node, and the count is the
/// same arithmetic as the quiet horizon above: a leaf under a root move
/// that beat everything before it is visited twice, because the window it
/// was first searched in had no room for the answer it gave. A capture
/// searched instead of refused shows up as a node under a leaf, which
/// neither term accounts for.
#[test]
fn a_losing_capture_at_the_horizon_is_refused_without_being_searched() {
    for (fen, reply) in both_colours(STAND_PAT, "f7b3") {
        let mut b = board(&fen);
        assert!(the_only_reply_exchange(&mut b, &reply) < 0);
        let expected = best_static_reply(&mut b);
        let roots = generate_legal(&b).len() as u64;
        let re_searched = root_re_searches(&mut b);
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.score, expected, "{fen}");
        assert_eq!(
            r.nodes,
            1 + roots + re_searched,
            "{fen}: the losing capture was searched"
        );
    }
}

/// The same shape with the pawn undefended: Qxa2 wins a pawn, is searched,
/// and the root scores below its best static reply. The knight on a3 is
/// there to block the pawn, so that every root move is a king move and
/// the capture is the same after each; it attacks nothing.
const WINNING_AT_THE_HORIZON: &str = "7k/5q2/8/8/8/n7/P7/7K w - - 0 1";

#[test]
fn a_winning_capture_at_the_horizon_is_searched() {
    for (fen, reply) in both_colours(WINNING_AT_THE_HORIZON, "f7a2") {
        let mut b = board(&fen);
        assert!(the_only_reply_exchange(&mut b, &reply) > 0);
        let ceiling = best_static_reply(&mut b);
        let roots = generate_legal(&b).len() as u64;
        let r = search(&mut b, Limits::depth(1));
        assert!(
            r.nodes > 1 + roots,
            "{fen}: the winning capture was not searched"
        );
        assert!(r.score < ceiling, "{fen}: {} against {ceiling}", r.score);
    }
}

/// An even exchange is not a losing one: after every white king move the
/// only noisy reply is axb3, a pawn for a pawn, and it is searched. The
/// rule is `see < 0`, and this is the boundary. White's a- and b-pawns are
/// blocked so that no quiet root move changes what defends b3; bxa4 is a
/// root move too, which the main search plays and the helper skips, and
/// under it Black has nothing noisy, so it is one node either way.
const EVEN_AT_THE_HORIZON: &str = "7k/8/8/8/pp6/nP6/P7/7K w - - 0 1";

#[test]
fn an_even_exchange_at_the_horizon_is_searched() {
    for (fen, reply) in both_colours(EVEN_AT_THE_HORIZON, "a4b3") {
        let mut b = board(&fen);
        assert_eq!(the_only_reply_exchange(&mut b, &reply), 0);
        let roots = generate_legal(&b).len() as u64;
        let r = search(&mut b, Limits::depth(1));
        assert!(
            r.nodes > 1 + roots,
            "{fen}: the even exchange was not searched"
        );
    }
}

/// In check, nothing is refused. The defended-blocker position: Ng1 is
/// White's only move and gives check; Black's answers are Qxg1, which
/// loses the queen for the knight, and seven king moves. Nine of the nodes
/// are the eight evasions and the recapture under the queen capture, and
/// the queen capture being two of them is the point: it is searched, and
/// the king takes back under it. A refused evasion is those two nodes
/// missing.
///
/// **Two more nodes are the root and the node below the check, and the
/// rest are the null window paying for being wrong.** The node below the
/// check searches its first evasion in the full window and the rest in one
/// with no room for a better answer, so an evasion that turns out to be
/// better is searched twice. Three of them are in one colour and one in
/// the other: eleven nodes became fourteen and twelve. That difference is
/// not a difference about check evasions. It is how many of them improved
/// on the evasions tried before them, which is the order they are tried in
/// and what they are worth, and a mirrored position is not searched in a
/// mirrored order. The counts are exact and stay exact; what they stopped
/// being is one number.
#[test]
fn a_losing_evasion_is_searched_all_the_same() {
    for ((fen, _), expected) in both_colours(DEFENDED_BLOCKER, "a1a1")
        .into_iter()
        .zip([14, 12])
    {
        let mut b = board(&fen);
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.nodes, expected, "{fen}");
    }
}

/// The corpus at depth two, with losing captures refused. Measured with
/// them searched: 68,845 nodes, Kiwipete the worst at 59,825 (the ceiling
/// the evasion sort's gate holds is 150,000). With them refused the
/// measured total falls well under the ceiling here, which sits between
/// the two: a coverage assertion again, failing if the rule is not applied
/// and clearing by a wide margin if it is.
const CORPUS_DEPTH_TWO_PRUNED_CEILING: u64 = 40_000;

#[test]
fn refusing_losing_captures_saves_nodes() {
    let mut total = 0;
    let mut worst = (0u64, String::new());
    for fen in support::corpus_fens() {
        let mut b = board(&fen);
        if generate_legal(&b).is_empty() {
            continue;
        }
        let r = search(&mut b, Limits::depth(2));
        total += r.nodes;
        if r.nodes > worst.0 {
            worst = (r.nodes, fen.clone());
        }
    }
    println!(
        "corpus at depth two, losing captures refused: {total} nodes, worst {} in {}",
        worst.0, worst.1
    );
    assert!(
        total < CORPUS_DEPTH_TWO_PRUNED_CEILING,
        "{total} nodes over the corpus at depth two"
    );
}

// ---------------------------------------------------------------------------
// The tree below the horizon is bounded
// ---------------------------------------------------------------------------

/// Depth one from every corpus position -- the standard suite, the DFRC
/// arrays, the castling-legality set, the edge cases -- completes within a
/// fixed number of nodes. The horizon resolves captures and promotions,
/// which material bounds, and a horizon that recursed on anything else
/// would not stay under the ceiling. Measured when the quiescence search
/// landed, noisy moves ordered by MVV-LVA and nothing pruned: 245,781
/// nodes, in Kiwipete, and 159,421,843 there before the ordering, with the
/// ceiling at a million, four times the measurement. With losing captures
/// refused it is 3,019, Kiwipete still, and the ceiling has followed it
/// down to about five times that.
const DEPTH_ONE_NODE_CEILING: u64 = 15_000;

#[test]
fn depth_one_is_bounded_in_every_corpus_position() {
    let mut worst = (0u64, String::new());
    for fen in support::corpus_fens() {
        let mut b = board(&fen);
        if generate_legal(&b).is_empty() {
            continue;
        }
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.depth, 1, "{fen}");
        assert!(
            r.nodes < DEPTH_ONE_NODE_CEILING,
            "{fen}: {} nodes at depth one",
            r.nodes
        );
        if r.nodes > worst.0 {
            worst = (r.nodes, fen.clone());
        }
    }
    println!("largest depth-one tree: {} nodes in {}", worst.0, worst.1);
}
