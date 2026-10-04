// SPDX-License-Identifier: GPL-3.0-or-later

//! Quiescence has no perft: a slightly wrong stand-pat or noisy set plays legal chess and passes
//! every test that does not know the answer. These pin, at depth one and in both colours, the
//! behaviours that define it, each constructed position checked for what it claims first.

mod support;

use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::types::PromoPiece;
use cadence_core::{Colour, Move, PieceType, START_FEN, generate_legal, generate_noisy, parse_uci};
use cadence_engine::eval;
use cadence_engine::picker::{capture_key, noisy_key, sort_noisy};
use cadence_engine::position::Position;
use cadence_engine::score::{self, Score, mate_in};
use cadence_engine::search::Limits;
use cadence_engine::see::see;
use support::table;

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

/// The ranks flip, the files stay.
fn mirror_uci(uci: &str) -> String {
    uci.chars()
        .map(|c| match c {
            '1'..='8' => char::from(b'9' - (c as u8 - b'0')),
            other => other,
        })
        .collect()
}

fn both_colours(fen: &str, uci: &str) -> [(String, String); 2] {
    [
        (fen.to_string(), uci.to_string()),
        (support::mirror_fen(fen), mirror_uci(uci)),
    ]
}

/// For the in-test material arithmetic only.
fn value(pt: PieceType) -> i32 {
    match pt {
        PieceType::Pawn => 1,
        PieceType::Knight | PieceType::Bishop => 3,
        PieceType::Rook => 5,
        PieceType::Queen => 9,
        PieceType::King => 0,
    }
}

fn balance(b: &Board, c: Colour) -> i32 {
    let mut total = 0;
    for pt in PieceType::ALL {
        let v = value(pt);
        total += v * i32::try_from(b.pieces(c, pt).count()).expect("fits");
        total -= v * i32::try_from(b.pieces(c.flip(), pt).count()).expect("fits");
    }
    total
}

fn captures(b: &Board) -> Vec<Move> {
    generate_legal(b)
        .iter()
        .filter(|m| m.is_capture())
        .collect()
}

fn captures_of(b: &Board, pt: PieceType) -> Vec<Move> {
    captures(b)
        .into_iter()
        .filter(|m| {
            !m.is_en_passant() && b.piece_at(m.to_sq()).is_some_and(|p| p.piece_type() == pt)
        })
        .collect()
}

/// The opponent's captures of it after a null move.
fn en_prise(b: &mut Position, pt: PieceType) -> bool {
    b.make_null_move();
    let hit = !captures_of(b, pt).is_empty();
    b.unmake_null_move();
    hit
}

/// The mover's balance after the worst recapture is below its balance before the capture.
fn loses_material_to_a_recapture(b: &mut Position, bad: Move) -> bool {
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

fn has_a_move_allowing_no_capture(b: &mut Position) -> bool {
    let legal = generate_legal(b);
    legal.iter().any(|m| {
        b.make_move(m);
        let none = captures(b).is_empty();
        b.unmake_move(m);
        none
    })
}

/// The depth-one score of a search whose horizon is quiet.
fn best_static_reply(b: &mut Position) -> Score {
    let mut best = Score::MIN;
    for m in generate_legal(b).iter() {
        b.make_move(m);
        best = best.max(-eval::evaluate(b));
        b.unmake_move(m);
    }
    best
}

/// A root move strictly better than everything before it is searched twice, in the null window and
/// again in the full one. Sound only where a root move's value is minus the static evaluation it
/// leads to, which each caller establishes first.
fn root_re_searches(b: &mut Position) -> u64 {
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

/// Each a piece defended once, taken by something bigger, with a quiet alternative available.
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
            let mut b = support::position(&fen);
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

/// At depth one a static horizon takes the material; resolving captures sees the recapture.
#[test]
fn a_capture_refuted_by_an_immediate_recapture_is_not_played() {
    for (fen, bad) in LOSING_CAPTURES {
        for (fen, bad) in both_colours(fen, bad) {
            let mut b = support::position(&fen);
            let bad = mv(&b, &bad);
            // Standing pat bounds the side to move at the horizon, so the root never scores above
            // its best static reply.
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

/// Plus a decoy, a knight on the rim whose centralising move is the best the piece-square tables
/// see.
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
            let mut b = support::position(&fen);
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
            let mut b = support::position(&fen);
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

/// After each of White's three king moves Black's only capture, Qxb3, loses the queen. The side to
/// move is never obliged to capture, so the depth-one score is the best static reply.
const STAND_PAT: &str = "7k/5q2/8/8/1p6/pP6/P7/7K w - - 0 1";

#[test]
fn the_stand_pat_position_is_what_it_claims() {
    for (fen, _) in both_colours(STAND_PAT, "a1a1") {
        let mut b = support::position(&fen);
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
        let mut b = support::position(&fen);
        let expected = best_static_reply(&mut b);
        let roots = generate_legal(&b).len() as u64;
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.score, expected, "{fen}: score {}", r.score);
        // The losing capture is refused by the exchange evaluation before it is searched, which the
        // section on losing captures gates.
        let _ = roots;
    }
}

/// Where no root move allows a capture or promotion, each leaf is one node, plus one for each root
/// move the null window searches twice. `root_re_searches` computes that term from the static
/// evaluations, so this pins the re-search rule as well as the cost of a quiet horizon.
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
        let mut b = support::position(&fen);
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

/// Kiwipete has eight captures at the root and plenty below.
#[test]
fn a_noisy_horizon_is_searched_below_depth_one() {
    let fen = support::standard_fen("kiwipete");
    let mut b = support::position(&fen);
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

/// A side in check may not stand pat, so a mate in one is found at depth one.
#[test]
fn mate_in_one_is_found_at_depth_one() {
    for (fen, key) in both_colours("7k/8/6K1/8/8/8/8/1R6 w - - 0 1", "b1b8") {
        let mut b = support::position(&fen);
        let key = mv(&b, &key);
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.best, key, "{fen}: played {:?}", r.best);
        assert_eq!(r.score, mate_in(1), "{fen}: score {}", r.score);
        assert_eq!(score::uci(r.score), "mate 1", "{fen}");
        assert_eq!(r.pv, vec![key], "{fen}: pv {:?}", r.pv);
    }
}

/// Every evasion is a quiet king move after which the rook takes the queen. A horizon that stands
/// pat in check or answers only with captures scores Ra8+ as nothing.
const SKEWER: &str = "4k2q/8/8/8/8/8/8/R5K1 w - - 0 1";

#[test]
fn the_skewer_is_what_it_claims() {
    for (fen, key) in both_colours(SKEWER, "a1a8") {
        let mut b = support::position(&fen);
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

/// Read from the table so a fit cannot move the score past the bar. Winning the queen scores about
/// a rook and missing it about minus the queen's margin over the rook.
fn half_a_rook() -> Score {
    let rook = PieceType::Rook.index();
    let table = &eval::WEIGHTS[eval::PST + 64 * rook..eval::PST + 64 * rook + 64];
    let squares: i32 = table.iter().map(|w| w.eg).sum();
    let value = eval::WEIGHTS[eval::MATERIAL + rook].eg + squares / 64;
    value / 2
}

#[test]
fn a_check_at_the_horizon_is_answered_with_every_evasion() {
    let bar = half_a_rook();
    for (fen, key) in both_colours(SKEWER, "a1a8") {
        let mut b = support::position(&fen);
        let key = mv(&b, &key);
        for depth in 1..=2 {
            let r = search(&mut b, Limits::depth(depth));
            assert_eq!(r.best, key, "{fen}: depth {depth} played {:?}", r.best);
            assert!(
                r.score > bar,
                "{fen}: depth {depth} scores {} against {bar}",
                r.score
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Promotions at the horizon
// ---------------------------------------------------------------------------

/// A horizon blind to promotions plays Rh7 and meets a queen. The only check, Rh8+, hangs the rook,
/// so a check cannot push the promotion out of sight.
const PROMOTION: &str = "6k1/6p1/8/8/2b5/7R/p7/6K1 w - - 0 1";

/// Whether every promotion the opponent has is met by a capture of the
/// promoted piece.
fn every_promotion_is_captured(b: &mut Position) -> bool {
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
        let mut b = support::position(&fen);
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
        let mut b = support::position(&fen);
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

/// Pawn, knight, bishop, rook, queen, the king an attacker only. The quiescence sort reads this
/// key, so its order is part of what the bench number depends on.
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

/// En passant is a pawn taking a pawn; a promotion that captures keeps the capture's order within
/// its class.
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

/// In Kiwipete the sorted order is written out, because there the generated order is not it.
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

/// Taken from the corpus, not constructed, because what the sort is worth depends on how often a
/// horizon check has a noisy answer. A check that is mate generates nothing and is not one of
/// these.
fn for_each_in_check_child(mut f: impl FnMut(&str, Move, &Board, &[Move])) {
    for fen in support::corpus_fens() {
        let mut b = support::position(&fen);
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

/// `i32::MIN` rather than `picker::QUIET`, so the assertion says below all of them and not the
/// implementation's number.
fn evasion_rank(b: &Board, m: Move) -> i32 {
    if m.is_noisy() {
        noisy_key(b, m)
    } else {
        i32::MIN
    }
}

/// The premise, read off the generator: king moves come first, so capturing the checker is tried
/// after every retreat.
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

/// The one place quiescence sorts a list holding quiet moves.
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

/// Ng1, White's only move, blocks and gives check from a defended square, so Qxg1 loses the queen
/// while the seven king moves keep it. The sort puts the losing capture first.
const DEFENDED_BLOCKER: &str = "8/8/8/8/8/7N/4k1PP/q6K w - - 0 1";

#[test]
fn the_defended_blocker_is_what_it_claims() {
    for (fen, key) in both_colours(DEFENDED_BLOCKER, "h3g1") {
        let mut b = support::position(&fen);
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

/// The noisy evasion tried first loses a queen, so a search stopping at the head would score this
/// the other way round. Eleven nodes in either order: a claim about value, not cost.
#[test]
fn a_noisy_evasion_that_loses_is_not_the_answer() {
    for (fen, key) in both_colours(DEFENDED_BLOCKER, "h3g1") {
        let mut b = support::position(&fen);
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

/// Gated on counters, not nodes: lists prepared says the in-check horizon is reached, lists
/// reordered says the sort ran in the search. A node ceiling had stopped separating the builds, and
/// asserting the sorted order directly passes with the sort deleted from `quiesce`.
#[test]
fn ordering_the_check_evasions_reaches_the_head_of_the_list() {
    let (mut lists, mut reordered) = (0u64, 0u64);
    for fen in support::corpus_fens() {
        let mut b = support::position(&fen);
        if generate_legal(&b).is_empty() {
            continue;
        }
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut s = support::search(Limits::depth(2), &stop, &tt);
        s.run(&mut b, &mut Vec::new());
        lists += s.evasion_lists();
        reordered += s.evasion_lists_reordered();
    }
    println!("{lists} evasion lists prepared, {reordered} reordered at the head");
    assert!(
        lists > 0,
        "the corpus at depth two reached no check at the horizon"
    );
    assert!(
        reordered > 0,
        "{lists} evasion lists prepared and the sort moved the head of none"
    );
}

// ---------------------------------------------------------------------------
// Losing captures are not searched
// ---------------------------------------------------------------------------
// Out of check a noisy move with negative `see` is refused unsearched; in check nothing is, since
// every evasion answers the check. What `see` gets right is `tests/see.rs`'s gate.

/// Asserted to be the same move and value after every quiet root move; a noisy root move is the
/// main search's business and is skipped.
fn the_only_reply_exchange(b: &mut Position, uci: &str) -> i32 {
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

/// One node per root move, plus one for each the null window visits twice; a searched capture shows
/// as a node under a leaf, which neither term accounts for.
#[test]
fn a_losing_capture_at_the_horizon_is_refused_without_being_searched() {
    for (fen, reply) in both_colours(STAND_PAT, "f7b3") {
        let mut b = support::position(&fen);
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

/// The knight on a3 blocks the pawn, so every root move is a king move and the capture is the same
/// after each.
const WINNING_AT_THE_HORIZON: &str = "7k/5q2/8/8/8/n7/P7/7K w - - 0 1";

#[test]
fn a_winning_capture_at_the_horizon_is_searched() {
    for (fen, reply) in both_colours(WINNING_AT_THE_HORIZON, "f7a2") {
        let mut b = support::position(&fen);
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

/// `see < 0` is the rule and this is its boundary. The blocked a- and b-pawns keep every quiet root
/// move from changing what defends b3.
const EVEN_AT_THE_HORIZON: &str = "7k/8/8/8/pp6/nP6/P7/7K w - - 0 1";

#[test]
fn an_even_exchange_at_the_horizon_is_searched() {
    for (fen, reply) in both_colours(EVEN_AT_THE_HORIZON, "a4b3") {
        let mut b = support::position(&fen);
        assert_eq!(the_only_reply_exchange(&mut b, &reply), 0);
        let roots = generate_legal(&b).len() as u64;
        let r = search(&mut b, Limits::depth(1));
        assert!(
            r.nodes > 1 + roots,
            "{fen}: the even exchange was not searched"
        );
    }
}

/// The queen capture and the king's recapture under it are searched; a refused evasion is those two
/// nodes missing. The colours differ by null-window re-searches, which follow the order the
/// evasions are tried in, so each count is exact but not one number.
#[test]
fn a_losing_evasion_is_searched_all_the_same() {
    for ((fen, _), expected) in both_colours(DEFENDED_BLOCKER, "a1a1")
        .into_iter()
        .zip([15, 13])
    {
        let mut b = support::position(&fen);
        let r = search(&mut b, Limits::depth(1));
        assert_eq!(r.nodes, expected, "{fen}");
    }
}

/// Between 63,206 nodes with losing captures searched and 10,643 with them refused, a 5.94x window:
/// this rule removes whole subtrees, so a ceiling works here where it failed for the evasion sort.
const CORPUS_DEPTH_TWO_PRUNED_CEILING: u64 = 40_000;

#[test]
fn refusing_losing_captures_saves_nodes() {
    let mut total = 0;
    let mut worst = (0u64, String::new());
    for fen in support::corpus_fens() {
        let mut b = support::position(&fen);
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

/// Material bounds a horizon that resolves only captures and promotions; one recursing on anything
/// else would not stay under. Measured at 3,019 nodes, Kiwipete the largest, with losing captures
/// refused.
const DEPTH_ONE_NODE_CEILING: u64 = 15_000;

#[test]
fn depth_one_is_bounded_in_every_corpus_position() {
    let mut worst = (0u64, String::new());
    for fen in support::corpus_fens() {
        let mut b = support::position(&fen);
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
