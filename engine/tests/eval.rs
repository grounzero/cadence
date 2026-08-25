// SPDX-License-Identifier: GPL-3.0-or-later

//! The static evaluation: antisymmetric under a colour flip, bounded away
//! from the mate scale, and not trivial.
//!
//! The property with teeth is `white(pos) == -white(mirror(pos))`, where
//! `white` is the evaluation from White's point of view. A piece-square
//! table indexed with the wrong flip for one colour, a term counted for
//! White and not for Black, a tempo bonus applied on the wrong side of the
//! sign change: all of them break it. The property is only as good as the
//! set it runs over, so the coverage is counted and asserted -- total
//! positions, positions with castling rights live, DFRC positions, and
//! positions at each end of the phase scale and in between -- rather than
//! assumed. And the mirror itself is checked first, because a mirror that
//! quietly produced a different kind of position would make every count
//! above it vacuous.
//!
//! "Not trivial" is there because a zero evaluation is perfectly symmetric.

mod support;

use cadence_core::position::Board;
use cadence_core::{CastlingRights, Colour, FenStyle, START_FEN, generate_legal};
use cadence_engine::eval::{PHASE_MAX, evaluate, phase};
use cadence_engine::score::{MAX_EVAL, Score};
use support::{Rng, mirror, mirror_fen};

/// The evaluation from White's point of view.
fn white(board: &Board) -> Score {
    match board.side_to_move() {
        Colour::White => evaluate(board),
        Colour::Black => -evaluate(board),
    }
}

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// Whether any castling right of `board` is a DFRC one: a king off the
/// e-file or a castling rook off the a/h-file.
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

/// Every position the symmetry and bound properties run over: the corpus,
/// and random walks from the start position, the DFRC arrays and the
/// endgame seeds. Walks move on with a random legal move; a walk that ends
/// (no legal move) restarts from its seed with the next seed of the RNG.
fn positions() -> Vec<Board> {
    let mut out: Vec<Board> = support::corpus_fens().iter().map(|f| board(f)).collect();
    let mut seeds: Vec<String> = vec![START_FEN.to_string()];
    seeds.extend(support::dfrc_arrays().into_iter().map(|(_, _, f)| f));
    seeds.extend(support::ENDGAME_FENS.iter().map(|s| (*s).to_string()));
    let mut rng = Rng::new(0xE7A1_5E7A_1A7E_D001);
    for (i, fen) in seeds.iter().enumerate() {
        // A seed with the side not to move in check is accepted by
        // `from_fen`, which validates that a position is representable and
        // not that it is legal. Walking from one no longer panics inside
        // `core` -- a king is never a target, so the first move cannot take
        // one -- but it is still not a position to measure an evaluation's
        // symmetry over, because no game reaches it. The assertion names a
        // bad seed immediately rather than silently walking from it.
        let seed = board(fen);
        assert!(
            !seed.opponent_in_check(),
            "seed {fen}: the side not to move is in check"
        );
        // More walking from the endgame seeds, which are few, so the quiet
        // end of the phase scale is populated.
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
    // The coverage, asserted. A walk that did not reach the endgame, or a
    // seed list with no DFRC in it, would pass the property above and mean
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

#[test]
fn the_evaluation_stays_inside_the_evaluation_bound() {
    let mut positions = positions();
    // Absurd material, both ways, and the evaluation must still not reach
    // the mate scale: a mate score that is really an evaluation would be
    // preferred to a real mate, or feared like one.
    for fen in [
        "QQQQQQQQ/QQQQQQQQ/8/8/8/8/8/k6K w - - 0 1",
        "qqqqqqqq/qqqqqqqq/8/8/8/8/8/K6k w - - 0 1",
        "RRRRRRRR/RRRRRRRR/RRRRRRRR/8/8/8/8/k6K b - - 0 1",
        "k6K/8/8/8/8/nnnnnnnn/bbbbbbbb/qqqqqqqq w - - 0 1",
    ] {
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

/// Remove the piece on `sq` from `fen`.
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
    // Rebuild the placement field.
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

    // Taking a Black piece off the start position favours White, by more
    // for a more valuable piece. The castling field is dropped so that
    // removing a rook does not make the FEN inconsistent.
    let base = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w - - 0 1";
    let gain = |sq: &str| white(&without(base, sq));
    let (pawn, knight, bishop, rook, queen) =
        (gain("d7"), gain("b8"), gain("c8"), gain("a8"), gain("d8"));
    assert!(pawn > 0, "a pawn up is worth {pawn}");
    assert!(pawn < knight, "pawn {pawn} vs knight {knight}");
    assert!(pawn < bishop, "pawn {pawn} vs bishop {bishop}");
    assert!(knight < rook, "knight {knight} vs rook {rook}");
    assert!(bishop < rook, "bishop {bishop} vs rook {rook}");
    assert!(rook < queen, "rook {rook} vs queen {queen}");
    // Roughly the classical scale, in centipawns. Wide bands: the point is
    // that the numbers are in the right order of magnitude, not tuned.
    assert!((60..=160).contains(&pawn), "pawn {pawn}");
    assert!((250..=400).contains(&knight), "knight {knight}");
    assert!((250..=400).contains(&bishop), "bishop {bishop}");
    assert!((400..=650).contains(&rook), "rook {rook}");
    assert!((750..=1200).contains(&queen), "queen {queen}");

    // And the same from Black's side, by symmetry of the construction.
    let base_b = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b - - 0 1";
    assert_eq!(white(&without(base_b, "d2")), -pawn);
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
    // Two positions that differ in the placement of one piece. These pin
    // that the piece-square tables are wired in and oriented the right way
    // up for both colours; the table values themselves are not pinned.
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
