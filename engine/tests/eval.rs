// SPDX-License-Identifier: GPL-3.0-or-later

//! `white(pos) == -white(mirror(pos))` catches a wrongly flipped table or a one-sided term, and is
//! only as good as its set, so the coverage is counted and asserted. "Not trivial" is there because
//! a zero evaluation is perfectly symmetric.

mod support;

use cadence_core::position::Board;
use cadence_core::{CastlingRights, Colour, FenStyle, PieceType, START_FEN, generate_legal};
use cadence_engine::eval::{PHASE_MAX, WEIGHTS, evaluate, phase, trace};
use cadence_engine::score::{MAX_EVAL, Score};
use support::{Rng, mirror, mirror_fen};

fn white(board: &Board) -> Score {
    match board.side_to_move() {
        Colour::White => evaluate(board),
        Colour::Black => -evaluate(board),
    }
}

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// A king off the e-file or a castling rook off the a/h-file.
fn is_dfrc(board: &Board) -> bool {
    let layout = board.layout();
    let rights = board.castling_rights();
    for c in Colour::ALL {
        if !rights.any(c) {
            continue;
        }
        if let Some(k) = layout.king_from[c.index()].get()
            && k.file() != cadence_core::types::File::E
        {
            return true;
        }
    }
    layout.rook_from.iter().any(|r| {
        r.get().is_some_and(|sq| {
            sq.file() != cadence_core::types::File::A && sq.file() != cadence_core::types::File::H
        })
    })
}

/// A walk that ends restarts from its seed with the RNG's next seed.
fn positions() -> Vec<Board> {
    let mut out: Vec<Board> = support::corpus_fens().iter().map(|f| board(f)).collect();
    let mut seeds: Vec<String> = vec![START_FEN.to_string()];
    seeds.extend(support::dfrc_arrays().into_iter().map(|(_, _, f)| f));
    seeds.extend(support::ENDGAME_FENS.iter().map(|s| (*s).to_string()));
    let mut rng = Rng::new(0xE7A1_5E7A_1A7E_D001);
    for (i, fen) in seeds.iter().enumerate() {
        // `from_fen` accepts a seed with the side not to move in check, which no game reaches; the
        // assertion names such a seed rather than walking from it.
        let seed = board(fen);
        assert!(
            !seed.opponent_in_check(),
            "seed {fen}: the side not to move is in check"
        );
        // The endgame seeds are few, so the quiet end of the phase scale gets more walking.
        let walks = if i > 20 { 12 } else { 4 };
        for _ in 0..walks {
            let mut b = board(fen);
            for _ in 0..80 {
                let legal = generate_legal(&b);
                if legal.is_empty() {
                    break;
                }
                b.play(legal.as_slice()[rng.below(legal.len())]);
                out.push(b.duplicate());
            }
        }
    }
    out
}

#[test]
fn the_mirror_is_an_involution_and_preserves_legality() {
    let mut checked = 0;
    for b in positions() {
        let fen = b.to_fen(FenStyle::Shredder);
        let m = mirror(&b);
        let mm = mirror(&m);
        assert_eq!(
            mm.to_fen(FenStyle::Shredder),
            fen,
            "mirror twice is not the identity"
        );
        assert_eq!(m.side_to_move(), b.side_to_move().flip(), "{fen}");
        assert_eq!(m.in_check(), b.in_check(), "{fen}");
        assert_eq!(
            generate_legal(&m).len(),
            generate_legal(&b).len(),
            "{fen} mirrors to {} with a different number of legal moves",
            m.to_fen(FenStyle::Shredder)
        );
        // The mirror of the mirror's FEN text, too: the text transform and
        // the board agree.
        assert_eq!(mirror_fen(&mirror_fen(&fen)), fen);
        checked += 1;
    }
    assert!(checked >= 5000, "only {checked} positions mirrored");
}

#[test]
fn the_evaluation_is_antisymmetric_under_a_colour_flip() {
    let positions = positions();
    let mut total = 0usize;
    let mut rights_live = 0usize;
    let mut dfrc = 0usize;
    let mut opening = 0usize; // phase == PHASE_MAX
    let mut between = 0usize; // 0 < phase < PHASE_MAX
    let mut ending = 0usize; // phase == 0
    let mut black_to_move = 0usize;
    for b in &positions {
        let m = mirror(b);
        let fen = b.to_fen(FenStyle::Shredder);
        assert_eq!(
            white(b),
            -white(&m),
            "eval({fen}) = {} but eval(mirror) = {} (mirror {})",
            white(b),
            white(&m),
            m.to_fen(FenStyle::Shredder)
        );
        // The side-to-move-relative form says the same thing.
        assert_eq!(evaluate(b), evaluate(&m), "{fen}");
        // And the phase does not depend on colour.
        assert_eq!(phase(b), phase(&m), "{fen}");

        total += 1;
        if b.castling_rights() != CastlingRights::NONE {
            rights_live += 1;
        }
        if is_dfrc(b) {
            dfrc += 1;
        }
        if b.side_to_move() == Colour::Black {
            black_to_move += 1;
        }
        let p = phase(b);
        assert!((0..=PHASE_MAX).contains(&p), "{fen}: phase {p}");
        if p == PHASE_MAX {
            opening += 1;
        } else if p == 0 {
            ending += 1;
        } else {
            between += 1;
        }
    }
    // A walk that missed the endgame, or a seed list without DFRC, would pass the property and mean
    // nothing.
    println!(
        "coverage: {total} positions, {rights_live} with rights live, {dfrc} DFRC, \
         {black_to_move} Black to move, phase {PHASE_MAX}/between/0: {opening}/{between}/{ending}"
    );
    assert!(total >= 5000, "only {total} positions");
    assert!(
        rights_live >= 1000,
        "only {rights_live} positions with castling rights live"
    );
    assert!(dfrc >= 500, "only {dfrc} DFRC positions");
    assert!(
        black_to_move >= 2000,
        "only {black_to_move} with Black to move"
    );
    assert!(
        opening >= 200,
        "only {opening} positions at phase {PHASE_MAX}"
    );
    assert!(
        between >= 2000,
        "only {between} positions strictly between the phase ends"
    );
    assert!(ending >= 200, "only {ending} positions at phase 0");
}

/// The last two carry forty queens, enough for the evaluation's clamp to bind.
const ABSURD: [&str; 6] = [
    "QQQQQQQQ/QQQQQQQQ/8/8/8/8/8/k6K w - - 0 1",
    "qqqqqqqq/qqqqqqqq/8/8/8/8/8/K6k w - - 0 1",
    "RRRRRRRR/RRRRRRRR/RRRRRRRR/8/8/8/8/k6K b - - 0 1",
    "k6K/8/8/8/8/nnnnnnnn/bbbbbbbb/qqqqqqqq w - - 0 1",
    "QQQQQQQQ/QQQQQQQQ/QQQQQQQQ/QQQQQQQQ/QQQQQQQQ/8/8/k6K b - - 0 1",
    "K6k/8/8/qqqqqqqq/qqqqqqqq/qqqqqqqq/qqqqqqqq/qqqqqqqq w - - 0 1",
];

#[test]
fn the_evaluation_stays_inside_the_evaluation_bound() {
    let mut positions = positions();
    // A mate score that is really an evaluation would be preferred to a real mate, or feared like
    // one.
    for fen in ABSURD {
        positions.push(board(fen));
    }
    for b in &positions {
        let e = evaluate(b);
        assert!(
            e > -MAX_EVAL && e < MAX_EVAL,
            "{}: {e} is outside ({}, {MAX_EVAL})",
            b.to_fen(FenStyle::Shredder),
            -MAX_EVAL
        );
    }
}

fn without(fen: &str, sq: &str) -> Board {
    let b = board(fen);
    let mut pieces: Vec<(String, char)> = Vec::new();
    for s in cadence_core::Square::all() {
        if let Some(p) = b.piece_at(s)
            && s.to_string() != sq
        {
            pieces.push((s.to_string(), p.to_char()));
        }
    }
    assert!(
        b.piece_at(cadence_core::Square::from_algebraic(sq).expect("square"))
            .is_some()
    );
    let mut rows: Vec<String> = Vec::new();
    for rank in (0..8).rev() {
        let mut row = String::new();
        let mut empty = 0;
        for file in 0..8 {
            let name = format!("{}{}", (b'a' + file) as char, rank + 1);
            match pieces.iter().find(|(s, _)| *s == name) {
                Some((_, c)) => {
                    if empty > 0 {
                        row.push_str(&empty.to_string());
                        empty = 0;
                    }
                    row.push(*c);
                }
                None => empty += 1,
            }
        }
        if empty > 0 {
            row.push_str(&empty.to_string());
        }
        rows.push(row);
    }
    let rest: Vec<&str> = fen.split_whitespace().skip(1).collect();
    board(&format!("{} {}", rows.join("/"), rest.join(" ")))
}

#[test]
fn material_is_counted_and_ordered() {
    // The start position is level.
    assert_eq!(evaluate(&board(START_FEN)), 0);

    // The castling field is dropped so removing a rook leaves the FEN consistent. f7 because its
    // removal frees no piece: a centre pawn's opens the bishop and queen at the bottom of their
    // mobility tables.
    let base = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w - - 0 1";
    let gain = |sq: &str| white(&without(base, sq));
    let (pawn, knight, bishop, rook, queen) =
        (gain("f7"), gain("b8"), gain("c8"), gain("a8"), gain("d8"));
    assert!(pawn > 0, "a pawn up is worth {pawn}");
    assert!(pawn < knight, "pawn {pawn} vs knight {knight}");
    assert!(pawn < bishop, "pawn {pawn} vs bishop {bishop}");
    assert!(knight < rook, "knight {knight} vs rook {rook}");
    assert!(bishop < rook, "bishop {bishop} vs rook {rook}");
    assert!(rook < queen, "rook {rook} vs queen {queen}");

    // And the same from Black's side, by symmetry of the construction.
    let base_b = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b - - 0 1";
    assert_eq!(white(&without(base_b, "f2")), -pawn);
}

/// Ranks 2 to 7 for the pawn, every square otherwise.
fn effective(piece: usize) -> (i32, i32) {
    use cadence_engine::eval::{MATERIAL, PST};
    let (squares, n) = if piece == 0 { (8..56, 48) } else { (0..64, 64) };
    let table = &WEIGHTS[PST + 64 * piece..PST + 64 * piece + 64];
    let (mg, eg) = squares.fold((0, 0), |(mg, eg), s| (mg + table[s].mg, eg + table[s].eg));
    let m = WEIGHTS[MATERIAL + piece];
    (m.mg + mg / n, m.eg + eg / n)
}

#[test]
fn every_piece_is_worth_roughly_its_classical_value_in_both_phases() {
    // Wide bands, for the order of magnitude. Effective values rather than a removal, because a
    // fitted table puts its largest penalties on the home squares.
    let bands = [
        ("pawn", 60..=160),
        ("knight", 250..=400),
        ("bishop", 250..=400),
        ("rook", 400..=650),
        ("queen", 750..=1200),
    ];
    for (piece, (name, band)) in bands.into_iter().enumerate() {
        let (mg, eg) = effective(piece);
        assert!(band.contains(&mg), "{name} middlegame {mg}");
        assert!(band.contains(&eg), "{name} endgame {eg}");
    }
}

#[test]
fn the_phase_spans_the_scale() {
    assert_eq!(phase(&board(START_FEN)), PHASE_MAX);
    assert_eq!(phase(&board("8/1k6/8/8/8/8/6K1/8 w - - 0 1")), 0);
    assert_eq!(phase(&board("8/8/8/4k3/8/8/4P3/4K3 w - - 0 1")), 0);
    // Monotone under removal: taking a piece off never raises the phase.
    let base = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w - - 0 1";
    for sq in ["a8", "b8", "c8", "d8", "e7", "h1", "g1", "d1"] {
        assert!(phase(&without(base, sq)) <= PHASE_MAX, "{sq}");
    }
    // Pawns do not count toward the phase; pieces do.
    assert_eq!(phase(&without(base, "e7")), PHASE_MAX);
    assert!(phase(&without(base, "d8")) < PHASE_MAX);
    // Surplus material saturates rather than overflowing the scale.
    assert_eq!(
        phase(&board("QQQQQQQQ/QQQQQQQQ/8/8/8/8/8/k6K w - - 0 1")),
        PHASE_MAX
    );
}

#[test]
fn the_evaluation_prefers_a_centralised_knight_and_an_advanced_pawn() {
    // Pins that the tables are wired in and the right way up for both colours, not their values.
    let rim = white(&board("4k3/8/8/8/8/8/8/N3K3 w - - 0 1"));
    let centre = white(&board("4k3/8/8/8/3N4/8/8/4K3 w - - 0 1"));
    assert!(centre > rim, "knight a1 {rim} vs d4 {centre}");
    let rim_b = white(&board("n3k3/8/8/8/8/8/8/4K3 w - - 0 1"));
    let centre_b = white(&board("4k3/8/8/3n4/8/8/8/4K3 w - - 0 1"));
    assert!(centre_b < rim_b, "black knight a8 {rim_b} vs d5 {centre_b}");

    let home = white(&board("4k3/8/8/8/8/8/4P3/4K3 w - - 0 1"));
    let seventh = white(&board("4k3/4P3/8/8/8/8/8/4K3 w - - 0 1"));
    assert!(seventh > home, "pawn e2 {home} vs e7 {seventh}");
    let home_b = white(&board("4k3/4p3/8/8/8/8/8/4K3 w - - 0 1"));
    let second_b = white(&board("4k3/8/8/8/8/8/4p3/4K3 w - - 0 1"));
    assert!(second_b < home_b, "black pawn e7 {home_b} vs e2 {second_b}");
}

/// Wide arithmetic, so nothing here wraps where the evaluation's own does not.
fn from_trace(b: &Board) -> i64 {
    let t = trace(b);
    let (mut mg, mut eg) = (0i64, 0i64);
    for (c, w) in t.coefficients.iter().zip(WEIGHTS.iter()) {
        mg += i64::from(*c) * i64::from(w.mg);
        eg += i64::from(*c) * i64::from(w.eg);
    }
    let p = i64::from(t.phase);
    let max = i64::from(PHASE_MAX);
    (mg * p + eg * (max - p)) / max
}

#[test]
fn the_trace_dotted_with_the_weights_is_the_evaluation() {
    // A term added outside the walk the trace records, or a weight read from anywhere but the
    // table, breaks it.
    let mut positions = positions();
    positions.extend(ABSURD.iter().map(|f| board(f)));
    let bound = i64::from(MAX_EVAL - 1);
    let (mut exact, mut clamped, mut rights_live, mut dfrc) = (0usize, 0usize, 0usize, 0usize);
    for b in &positions {
        let fen = b.to_fen(FenStyle::Shredder);
        assert_eq!(trace(b).phase, phase(b), "{fen}");
        let rebuilt = from_trace(b);
        let evaluated = i64::from(white(b));
        if rebuilt.abs() < bound {
            // The clamp is the identity here, so this is equality before it.
            assert_eq!(rebuilt, evaluated, "{fen}");
            exact += 1;
            if b.castling_rights() != CastlingRights::NONE {
                rights_live += 1;
            }
            if is_dfrc(b) {
                dfrc += 1;
            }
        } else {
            assert_eq!(rebuilt.clamp(-bound, bound), evaluated, "{fen}");
            clamped += 1;
        }
    }
    println!(
        "coverage: {exact} exact, {rights_live} with rights live, {dfrc} DFRC, {clamped} clamped"
    );
    assert!(exact >= 5000, "only {exact} positions compared exactly");
    assert!(
        rights_live >= 1000,
        "only {rights_live} positions with castling rights live"
    );
    assert!(dfrc >= 500, "only {dfrc} DFRC positions");
    assert!(clamped >= 2, "the clamp bound on only {clamped} positions");
}

// Pawn structure
// ---------------------------------------------------------------------------

/// The pawn-structure coefficients of `fen`: passed on ranks 2 to 7, isolated, doubled,
/// connected, White's count less Black's.
fn pawn_terms(fen: &str) -> ([i32; 6], i32, i32, i32) {
    use cadence_engine::eval::{CONNECTED, DOUBLED, ISOLATED, PASSED};
    let t = trace(&board(fen));
    let mut passed = [0; 6];
    passed.copy_from_slice(&t.coefficients[PASSED..PASSED + 6]);
    (
        passed,
        t.coefficients[ISOLATED],
        t.coefficients[DOUBLED],
        t.coefficients[CONNECTED],
    )
}

#[test]
fn each_pawn_term_counts_what_it_names() {
    // A lone passer on d5: passed on its fifth rank, and isolated.
    assert_eq!(
        pawn_terms("k7/8/8/3P4/8/8/8/7K w - - 0 1"),
        ([0, 0, 0, 1, 0, 0], 1, 0, 0)
    );
    // Black's lone passer on d4 is on its fifth rank too, so the count is the negative.
    assert_eq!(
        pawn_terms("k7/8/8/8/3p4/8/8/7K w - - 0 1"),
        ([0, 0, 0, -1, 0, 0], -1, 0, 0)
    );
    // An enemy pawn ahead on an adjacent file stops a passer, and here each stops the other.
    assert_eq!(pawn_terms("k7/2p5/8/3P4/8/8/8/7K w - - 0 1").0, [0; 6]);
    // One behind does not: d5 is passed on its fifth rank and c3 on Black's sixth.
    assert_eq!(
        pawn_terms("k7/8/8/3P4/8/2p5/8/7K w - - 0 1").0,
        [0, 0, 0, 1, -1, 0]
    );
    // Doubled on c2 and c3: one extra pawn on the file, both isolated, neither defended.
    // Neither has an enemy pawn ahead, so both are passed, on ranks 2 and 3.
    assert_eq!(
        pawn_terms("k7/8/8/8/8/2P5/2P5/7K w - - 0 1"),
        ([1, 1, 0, 0, 0, 0], 2, 1, 0)
    );
    // A phalanx on d4 and e4 is two connected pawns; e3 defends d4 and is not itself connected.
    assert_eq!(pawn_terms("k7/8/8/8/3PP3/8/8/7K w - - 0 1").3, 2);
    assert_eq!(pawn_terms("k7/8/8/8/3P4/4P3/8/7K w - - 0 1").3, 1);
    // The defended pawns are counted and not the defender: e3 guards d4 and f4, which is two.
    assert_eq!(pawn_terms("k7/8/8/8/3P1P2/4P3/8/7K w - - 0 1").3, 2);
}

#[test]
fn a_mirrored_position_has_every_pawn_coefficient_negated() {
    use cadence_engine::eval::{PASSED, WEIGHT_COUNT};
    let mut rng = Rng::new(0x9A11_5700_0000_0001);
    let mut checked = 0;
    for fen in support::corpus_fens() {
        let mut b = board(&fen);
        for _ in 0..40 {
            let legal = generate_legal(&b);
            if legal.is_empty() {
                break;
            }
            b.play(legal.as_slice()[rng.below(legal.len())]);
            let (t, m) = (trace(&b), trace(&mirror(&b)));
            for i in PASSED..WEIGHT_COUNT {
                assert_eq!(
                    t.coefficients[i],
                    -m.coefficients[i],
                    "{}",
                    b.to_fen(FenStyle::Shredder)
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 1000, "only {checked} positions");
}

// Mobility
// ---------------------------------------------------------------------------

/// White's pieces less Black's.
fn mobility_at(fen: &str, piece: PieceType, count: usize) -> i32 {
    use cadence_engine::eval::{MOBILITY, MOBILITY_OFFSET};
    trace(&board(fen)).coefficients[MOBILITY + MOBILITY_OFFSET[piece.index()] + count]
}

#[test]
fn each_mobility_table_counts_the_usable_squares() {
    // A knight on d4 reaches eight squares; an enemy pawn on e6 takes f5 from it but is itself a
    // square it can use, and its own pawn on c2 takes c2.
    assert_eq!(
        mobility_at("k7/8/8/8/3N4/8/8/7K w - - 0 1", PieceType::Knight, 8),
        1
    );
    assert_eq!(
        mobility_at("k7/8/4p3/8/3N4/8/8/7K w - - 0 1", PieceType::Knight, 7),
        1
    );
    assert_eq!(
        mobility_at("k7/8/4p3/8/3N4/8/2P5/7K w - - 0 1", PieceType::Knight, 6),
        1
    );
    // A slider stops at the first piece, and an own one is not counted.
    assert_eq!(
        mobility_at("k7/8/8/8/8/8/8/B6K w - - 0 1", PieceType::Bishop, 7),
        1
    );
    assert_eq!(
        mobility_at("k7/8/8/8/3P4/8/8/B6K w - - 0 1", PieceType::Bishop, 2),
        1
    );
    // An enemy king stops a rook and is counted; a Black piece counts against White.
    assert_eq!(
        mobility_at("7k/8/8/8/8/8/8/r6K w - - 0 1", PieceType::Rook, 14),
        -1
    );
    // A queen alone in the centre fills the last entry of its table.
    assert_eq!(
        mobility_at("k7/8/8/8/3Q4/8/8/7K w - - 0 1", PieceType::Queen, 27),
        1
    );
}

#[test]
fn every_knight_bishop_rook_and_queen_has_exactly_one_mobility_count() {
    use cadence_engine::eval::{MOBILITY, MOBILITY_LEN, MOBILITY_OFFSET, weight_name};
    for fen in support::corpus_fens() {
        let b = board(&fen);
        let t = trace(&b);
        for pt in [
            PieceType::Knight,
            PieceType::Bishop,
            PieceType::Rook,
            PieceType::Queen,
        ] {
            let at = MOBILITY + MOBILITY_OFFSET[pt.index()];
            let counted: i32 = t.coefficients[at..at + MOBILITY_LEN[pt.index()]]
                .iter()
                .sum();
            let pieces = b.pieces(Colour::White, pt).count().cast_signed()
                - b.pieces(Colour::Black, pt).count().cast_signed();
            assert_eq!(counted, pieces, "{fen} {}", weight_name(at));
        }
    }
    assert_eq!(weight_name(MOBILITY), "mobility.knight.0");
    assert_eq!(
        weight_name(MOBILITY + MOBILITY_OFFSET[2]),
        "mobility.bishop.0"
    );
    assert_eq!(
        weight_name(cadence_engine::eval::ATTACKERS - 1),
        "mobility.queen.27"
    );
}

// King safety
// ---------------------------------------------------------------------------

/// White's entry less Black's: the attack table, then the shield table.
fn king_terms(fen: &str) -> (Vec<i32>, Vec<i32>) {
    use cadence_engine::eval::{ATTACKERS, ATTACKERS_LEN, SHIELD, SHIELD_LEN};
    let t = trace(&board(fen));
    (
        t.coefficients[ATTACKERS..ATTACKERS + ATTACKERS_LEN].to_vec(),
        t.coefficients[SHIELD..SHIELD + SHIELD_LEN].to_vec(),
    )
}

/// A table of `len` entries holding White's count `white` and Black's count `black`.
fn counted(len: usize, white: usize, black: usize) -> Vec<i32> {
    let mut out = vec![0; len];
    out[white] += 1;
    out[black] -= 1;
    out
}

#[test]
fn the_attack_table_counts_the_pieces_reaching_the_kings_zone() {
    // White's king on g1 is on its back rank, so f3, g3 and h3 are in its zone, and Black's knight
    // on e5 reaches f3 and nothing else of it.
    assert_eq!(
        king_terms("k7/8/8/4n3/8/8/8/6K1 w - - 0 1").0,
        counted(5, 0, 1)
    );
    // On g2 the king is still on its back two ranks, so its zone reaches the fourth rank, where
    // Black's rook on a4 attacks it.
    assert_eq!(
        king_terms("k7/8/8/8/r7/8/6K1/8 w - - 0 1").0,
        counted(5, 0, 1)
    );
    // On g3 it is not, so the same rook a rank further up reaches nothing of it, while White's on
    // b1 reaches the zone of Black's king, which runs down to the sixth rank.
    assert_eq!(
        king_terms("k7/8/8/r7/8/6K1/8/1R6 w - - 0 1").0,
        counted(5, 1, 0)
    );
    // Pawns and kings are not attackers: Black's pawn on g4 and king on g3 reach the zone of
    // White's king and count nothing, while White's knight on e1 reaches theirs.
    assert_eq!(
        king_terms("8/8/8/8/6p1/6k1/8/4N1K1 w - - 0 1").0,
        counted(5, 1, 0)
    );
    // A piece counts once however many squares of the zone it reaches: the queen on g4 reaches
    // g3, g2, f3 and h3, and with the knight on e5 that is two attackers.
    assert_eq!(
        king_terms("k7/8/8/4n3/6q1/8/6P1/6K1 w - - 0 1").0,
        counted(5, 0, 2)
    );
}

#[test]
fn the_shield_table_counts_the_kings_own_pawns_ahead_on_three_files() {
    // f2, g2 and h2 shield the king on g1; Black's king has nothing in front of it.
    assert_eq!(
        king_terms("k7/8/8/8/8/8/5PPP/6K1 w - - 0 1").1,
        counted(5, 3, 0)
    );
    // Behind the king they do not, while a7 shields Black's.
    assert_eq!(
        king_terms("k7/p7/8/8/8/6K1/5PPP/8 w - - 0 1").1,
        counted(5, 0, 1)
    );
    // Neither does an own pawn two files away nor an enemy pawn in front.
    assert_eq!(
        king_terms("k7/pp6/8/8/8/6p1/4P3/6K1 w - - 0 1").1,
        counted(5, 0, 2)
    );
    // A doubled pawn counts twice, and a king on the edge has two files.
    assert_eq!(
        king_terms("k7/8/8/8/8/7P/6PP/7K w - - 0 1").1,
        counted(5, 3, 0)
    );
    // The start position less f7 leaves Black's king on e8 with d7 and e7 against White's three,
    // so the removal test's pawn carries a shield step as well as its own value.
    assert_eq!(
        king_terms("rnbqkbnr/ppppp1pp/8/8/8/8/PPPPPPPP/RNBQKBNR w - - 0 1").1,
        counted(5, 3, 2)
    );
}

#[test]
fn four_or_more_share_the_last_entry_of_each_table() {
    // Five Black pieces reach the zone of White's king on g1: the queen on h4, the rook on f8, the
    // bishop on c5 and both knights.
    assert_eq!(
        king_terms("k4r2/8/8/2b4n/4n2q/8/5PPP/6K1 w - - 0 1").0,
        counted(5, 0, 4)
    );
    // Five pawns shield it, with the g- and h-pawns doubled.
    assert_eq!(
        king_terms("k7/8/8/8/8/6PP/5PPP/6K1 w - - 0 1").1,
        counted(5, 4, 0)
    );
    // Twenty-four knights around a king, which only a FEN can hold, share the same entry.
    assert_eq!(
        king_terms("k7/2nnnnn1/1nn3nn/1n5n/1n2K2n/1n5n/1nn3nn/2nnnnn1 w - - 0 1").0,
        counted(5, 0, 4)
    );
}

#[test]
fn every_king_has_exactly_one_count_in_each_king_safety_table() {
    use cadence_engine::eval::{ATTACKERS, SHIELD, WEIGHT_COUNT, weight_name};
    for fen in support::corpus_fens() {
        let (attack, shield) = king_terms(&fen);
        for table in [attack, shield] {
            assert_eq!(table.iter().sum::<i32>(), 0, "{fen}");
            assert!(table.iter().filter(|c| **c > 0).count() <= 1, "{fen}");
            assert!(table.iter().all(|c| c.abs() <= 1), "{fen}");
        }
    }
    assert_eq!(weight_name(ATTACKERS), "attackers.0");
    assert_eq!(weight_name(SHIELD), "shield.0");
    assert_eq!(weight_name(WEIGHT_COUNT - 1), "tempo");
}

// Rook files, the bishop pair and tempo
// ---------------------------------------------------------------------------

/// Square by square through `piece_at`, so no file mask or colour mask is shared with the walk.
fn recount(b: &Board) -> [i32; 4] {
    let (mut open, mut semi, mut pair) = (0, 0, 0);
    for c in Colour::ALL {
        let sign = if c == Colour::White { 1 } else { -1 };
        let mut colours = [false; 2];
        for sq in cadence_core::Square::all() {
            match b.piece_at(sq) {
                Some(p) if p.colour() == c && p.piece_type() == PieceType::Rook => {
                    let on_file = |own: bool| {
                        cadence_core::Square::all().any(|s| {
                            s.file() == sq.file()
                                && b.piece_at(s).is_some_and(|q| {
                                    q.piece_type() == PieceType::Pawn && (q.colour() == c) == own
                                })
                        })
                    };
                    if !on_file(true) {
                        if on_file(false) {
                            semi += sign;
                        } else {
                            open += sign;
                        }
                    }
                }
                Some(p) if p.colour() == c && p.piece_type() == PieceType::Bishop => {
                    colours[(sq.file().index() + sq.rank().index()) % 2] = true;
                }
                _ => {}
            }
        }
        if colours[0] && colours[1] {
            pair += sign;
        }
    }
    let tempo = if b.side_to_move() == Colour::White {
        1
    } else {
        -1
    };
    [open, semi, pair, tempo]
}

fn traced(b: &Board) -> [i32; 4] {
    use cadence_engine::eval::{BISHOP_PAIR, ROOK_FILE, TEMPO};
    let t = trace(b);
    [
        t.coefficients[ROOK_FILE],
        t.coefficients[ROOK_FILE + 1],
        t.coefficients[BISHOP_PAIR],
        t.coefficients[TEMPO],
    ]
}

#[test]
fn rook_files_the_pair_and_tempo_count_what_they_name() {
    let (mut open, mut semi, mut pair, mut dfrc) = (0, 0, 0, 0);
    for b in &positions() {
        let expected = recount(b);
        assert_eq!(traced(b), expected, "{}", b.to_fen(FenStyle::Shredder));
        open += usize::from(expected[0] != 0);
        semi += usize::from(expected[1] != 0);
        pair += usize::from(expected[2] != 0);
        dfrc += usize::from(is_dfrc(b) && expected[..3].iter().any(|c| *c != 0));
    }
    assert!(
        open >= 500 && semi >= 500 && pair >= 500,
        "{open} {semi} {pair}"
    );
    assert!(dfrc >= 100, "only {dfrc} DFRC positions with a term live");
}

#[test]
fn each_rook_and_each_pair_is_counted_by_hand() {
    let at = |fen: &str| traced(&board(fen));
    // A file with no pawn, two rooks on it, and the other side's rook behind its own pawn.
    assert_eq!(at("r3k3/p7/8/8/8/8/1PPPPPPP/R3K3 w - - 0 1"), [0, 1, 0, 1]);
    assert_eq!(at("4k3/8/8/8/8/R7/1P6/R3K3 b - - 0 1"), [2, 0, 0, -1]);
    assert_eq!(at("3rk3/3p4/8/8/8/8/PPP1PPPP/3RK3 w - - 0 1"), [0, 1, 0, 1]);
    assert_eq!(at("4k3/R7/8/8/8/8/P7/4K3 w - - 0 1"), [0, 0, 0, 1]);
    // A pair needs both colours: two light-squared bishops are not one.
    assert_eq!(at("2b1kb2/8/8/8/8/8/8/2B1KB2 w - - 0 1"), [0, 0, 0, 1]);
    assert_eq!(at("4k3/8/8/8/8/8/8/1B1BK3 w - - 0 1"), [0, 0, 0, 1]);
    assert_eq!(at("4k3/8/8/8/8/8/8/2B1KB2 b - - 0 1"), [0, 0, 1, -1]);
}

/// The same position with the other side to move, where that is legal.
fn other_side_to_move(b: &Board) -> Option<Board> {
    if b.in_check() {
        return None;
    }
    let fen = b.to_fen(FenStyle::Shredder);
    let mut fields: Vec<&str> = fen.split_whitespace().collect();
    fields[1] = if fields[1] == "w" { "b" } else { "w" };
    fields[3] = "-";
    Board::from_fen(&fields.join(" ")).ok()
}

#[test]
fn the_side_to_move_reaches_the_evaluation_only_through_tempo() {
    // Both symmetry gates flip the side to move with the colours, so neither sees a term that
    // depends on it; this changes the side to move alone.
    use cadence_engine::eval::{TEMPO, WEIGHT_COUNT};
    let mut checked = 0;
    for b in &positions() {
        let Some(other) = other_side_to_move(b) else {
            continue;
        };
        let (t, o) = (trace(b), trace(&other));
        let fen = b.to_fen(FenStyle::Shredder);
        assert_eq!(t.phase, o.phase, "{fen}");
        for i in (0..WEIGHT_COUNT).filter(|&i| i != TEMPO) {
            assert_eq!(t.coefficients[i], o.coefficients[i], "{fen} weight {i}");
        }
        let mover = if b.side_to_move() == Colour::White {
            1
        } else {
            -1
        };
        assert_eq!(t.coefficients[TEMPO], mover, "{fen}");
        assert_eq!(o.coefficients[TEMPO], -mover, "{fen}");
        assert_eq!(from_trace(b), i64::from(white(b)), "{fen}");
        assert_eq!(from_trace(&other), i64::from(white(&other)), "{fen}");
        checked += 1;
    }
    assert!(checked >= 5000, "only {checked} positions");
}
