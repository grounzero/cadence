// SPDX-License-Identifier: GPL-3.0-or-later

//! The `position` handler, as a function.
//!
//! `position [startpos | fen <fen>] [moves <m>...]` rebuilds the session's
//! board from scratch: parse the FEN, then match each move against the
//! generated legal list and play it as a game move (matched,
//! never constructed from the string). The tests here drive
//! `Session::handle_line` directly and read the board back, which is what
//! the library split exists for.
//!
//! The round trips are genuine: a walker makes random moves on a board of
//! its own, spells each one with `to_uci` under the option value the session
//! has been given, and the session must arrive at the walker's position --
//! FEN, key and history -- having seen only the strings.

mod support;

use cadence_core::position::Board;
use cadence_core::{FenStyle, START_FEN, generate_legal, to_uci};
use cadence_engine::uci::Session;
use support::{Rng, dfrc_arrays};

fn shredder(board: &Board) -> String {
    board.to_fen(FenStyle::Shredder)
}

/// Walk `plies` random game moves from `fen`, returning the moves as UCI
/// strings spelled for `chess960`, the keys of every position left behind,
/// and the final board.
fn walk(fen: &str, seed: u64, plies: usize, chess960: bool) -> (Vec<String>, Vec<u64>, Board) {
    let mut board = Board::from_fen(fen).expect("walk seed parses");
    let mut rng = Rng::new(seed);
    let mut spelled = Vec::new();
    let mut keys = Vec::new();
    for _ in 0..plies {
        let legal = generate_legal(&board);
        if legal.is_empty() {
            break;
        }
        let m = legal.as_slice()[rng.below(legal.len())];
        spelled.push(to_uci(m, &legal, chess960));
        keys.push(board.key());
        board.play(m);
    }
    (spelled, keys, board)
}

fn session_with(chess960: bool) -> Session {
    let mut s = Session::new();
    let value = if chess960 { "true" } else { "false" };
    assert!(s.handle_line(&format!("setoption name UCI_Chess960 value {value}")));
    assert_eq!(s.chess960(), chess960);
    s
}

/// Feed one `position` line and check the session arrived where the walker
/// did.
fn check_round_trip(prefix: &str, fen: &str, seed: u64, plies: usize, chess960: bool) {
    let (moves, keys, want) = walk(fen, seed, plies, chess960);
    let mut line = String::from(prefix);
    if !moves.is_empty() {
        line.push_str(" moves ");
        line.push_str(&moves.join(" "));
    }
    let mut s = session_with(chess960);
    assert!(s.handle_line(&line), "the position line ended the session");
    let got = s.board();
    assert_eq!(
        shredder(got),
        shredder(&want),
        "chess960={chess960} seed={seed} after {} moves from {fen}:\n{line}",
        moves.len()
    );
    assert_eq!(got.key(), want.key());
    assert_eq!(got.ply(), 0, "the handler leaves the root at ply zero");
    assert_eq!(
        got.game_history(),
        &keys[..],
        "the history is the keys left behind"
    );
    assert_eq!(got.game_history().len(), moves.len());
}

#[test]
fn position_startpos_moves_round_trips_long_games() {
    // 300 plies is past MAX_PLY: the history is the game's, not the stack's.
    for seed in 0..12u64 {
        check_round_trip("position startpos", START_FEN, seed, 300, seed % 2 == 0);
    }
}

#[test]
fn position_fen_moves_round_trips_dfrc_games() {
    for (i, (wid, bid, fen)) in dfrc_arrays().into_iter().enumerate() {
        let prefix = format!("position fen {fen}");
        let seed = u64::from(wid) * 1000 + u64::from(bid);
        check_round_trip(&prefix, &fen, seed, 200, i % 2 == 0);
        check_round_trip(&prefix, &fen, seed + 1, 200, i % 2 == 1);
    }
}

#[test]
fn position_fen_moves_round_trips_from_every_corpus_position() {
    for (i, fen) in support::corpus_fens().into_iter().enumerate() {
        let prefix = format!("position fen {fen}");
        check_round_trip(&prefix, &fen, 99 + i as u64, 40, i % 2 == 0);
    }
}

#[test]
fn position_startpos_is_the_start_position() {
    let mut s = Session::new();
    // Something else first, so `startpos` is seen to replace it.
    s.handle_line("position fen 8/8/8/8/8/8/8/K6k w - - 0 1");
    assert_ne!(s.board().to_fen(FenStyle::XFen), START_FEN);
    s.handle_line("position startpos");
    assert_eq!(s.board().to_fen(FenStyle::XFen), START_FEN);
    assert!(s.board().game_history().is_empty());
}

#[test]
fn position_fen_without_moves_is_the_fen() {
    let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    let mut s = Session::new();
    s.handle_line(&format!("position fen {fen}"));
    assert_eq!(s.board().to_fen(FenStyle::XFen), fen);
    assert!(s.board().game_history().is_empty());
}

#[test]
fn a_four_field_fen_is_accepted() {
    let mut s = Session::new();
    s.handle_line("position fen 4k3/8/8/8/8/8/8/4K2R w K -");
    assert_eq!(
        s.board().to_fen(FenStyle::XFen),
        "4k3/8/8/8/8/8/8/4K2R w K - 0 1"
    );
}

#[test]
fn moves_with_nothing_after_it_is_the_position_itself() {
    let mut s = Session::new();
    s.handle_line("position startpos moves");
    assert_eq!(s.board().to_fen(FenStyle::XFen), START_FEN);
    s.handle_line("position startpos moves e2e4 moves");
    // `moves` is not a move; the replay stops there.
    assert_eq!(s.board().game_history().len(), 1);
}

#[test]
fn position_replaces_rather_than_appends() {
    let mut s = Session::new();
    s.handle_line("position startpos moves e2e4 e7e5");
    assert_eq!(s.board().game_history().len(), 2);
    s.handle_line("position startpos moves d2d4");
    assert_eq!(s.board().game_history().len(), 1);
    assert_eq!(
        s.board().to_fen(FenStyle::XFen),
        "rnbqkbnr/pppppppp/8/8/3P4/8/PPP1PPPP/RNBQKBNR b KQkq d3 0 1"
    );
}

/// A move that is not legal -- or not a move at all -- stops the replay at
/// the last position reached. Nothing panics, and the session goes on.
#[test]
fn an_illegal_move_stops_the_replay_without_ending_the_session() {
    for (line, want_history) in [
        ("position startpos moves e2e4 e7e5 e2e4", 2), // no piece on e2 now
        ("position startpos moves e2e5", 0),           // not a legal move
        ("position startpos moves xyz", 0),            // not a move
        ("position startpos moves e2e4 e7e5 g1f3 Nc6", 3), // SAN is not UCI
        ("position startpos moves 0000", 0),           // a null move is not legal
        ("position startpos moves e1g1", 0),           // castling through pieces
    ] {
        let mut s = Session::new();
        assert!(s.handle_line(line), "{line}");
        assert_eq!(s.board().game_history().len(), want_history, "{line}");
        assert_eq!(s.board().ply(), 0);
        // Still alive and answering.
        assert!(s.handle_line("isready"));
    }
}

#[test]
fn a_bad_fen_leaves_the_previous_position_in_place() {
    let mut s = Session::new();
    s.handle_line("position startpos moves e2e4");
    let before = shredder(s.board());
    for line in [
        "position fen",
        "position fen not a fen at all",
        "position fen 8/8/8/8/8/8/8/8 w - - 0 1", // no kings
        "position fen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR x KQkq - 0 1",
        "position",
        "position somethingelse",
    ] {
        assert!(s.handle_line(line), "{line}");
        assert_eq!(shredder(s.board()), before, "{line}");
    }
}

// ---------------------------------------------------------------------------
// Castling: both spellings, both option values
// ---------------------------------------------------------------------------

/// After `line`, the session's board equals `fen` after `castle` -- found in
/// the legal list by its king-takes-rook spelling -- has been played.
fn assert_castles(chess960: bool, fen: &str, castle_ktr: &str, line_moves: &str) {
    let mut want = Board::from_fen(fen).expect("castling fen parses");
    let legal = generate_legal(&want);
    let m = legal
        .iter()
        .find(|m| m.to_uci_chess960() == castle_ktr)
        .unwrap_or_else(|| panic!("{castle_ktr} is not legal in {fen}"));
    assert!(m.is_castle(), "{castle_ktr} is not a castle in {fen}");
    want.play(m);

    let mut s = session_with(chess960);
    s.handle_line(&format!("position fen {fen} moves {line_moves}"));
    assert_eq!(
        shredder(s.board()),
        shredder(&want),
        "chess960={chess960}: `moves {line_moves}` from {fen}"
    );
    assert_eq!(s.board().game_history().len(), 1);
}

#[test]
fn standard_castling_is_accepted_in_both_spellings_under_both_option_values() {
    // After 1.e4 e5 2.Nf3 Nc6 3.Bc4 Bc5, White may castle kingside.
    let fen = "r1bqk1nr/pppp1ppp/2n5/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4";
    for chess960 in [false, true] {
        assert_castles(chess960, fen, "e1h1", "e1g1");
        assert_castles(chess960, fen, "e1h1", "e1h1");
    }
    // The queenside, for Black.
    let fen = "r3kbnr/pppqpppp/2n5/3p1b2/3P1B2/2N5/PPPQPPPP/R3KBNR b KQkq - 6 5";
    for chess960 in [false, true] {
        assert_castles(chess960, fen, "e8a8", "e8c8");
        assert_castles(chess960, fen, "e8a8", "e8a8");
    }
    // And the literal result, as an anchor.
    let mut s = Session::new();
    s.handle_line(
        "position fen r1bqk1nr/pppp1ppp/2n5/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 4 4 moves e1g1",
    );
    assert_eq!(
        s.board().to_fen(FenStyle::XFen),
        "r1bqk1nr/pppp1ppp/2n5/2b1p3/2B1P3/5N2/PPPP1PPP/RNBQ1RK1 b kq - 5 4"
    );
}

#[test]
fn dfrc_castling_is_accepted_in_both_spellings_under_both_option_values() {
    // Kc1, Rh1: king-takes-rook is c1h1, king-to-destination is c1g1, and
    // they differ.
    let fen = "1k6/8/8/8/8/8/8/2K4R w H - 0 1";
    for chess960 in [false, true] {
        assert_castles(chess960, fen, "c1h1", "c1h1");
        assert_castles(chess960, fen, "c1h1", "c1g1");
    }
    // Kb1, Ra1, queenside: b1a1 is the castle; b1c1 is the destination
    // spelling -- and ALSO a legal quiet king move, so b1c1 must be the
    // quiet move under both option values and b1a1 the castle.
    let fen = "1k6/8/8/8/8/8/8/RK6 w A - 0 1";
    for chess960 in [false, true] {
        assert_castles(chess960, fen, "b1a1", "b1a1");
        let mut s = session_with(chess960);
        s.handle_line(&format!("position fen {fen} moves b1c1"));
        assert_eq!(
            s.board().to_fen(FenStyle::Shredder),
            "1k6/8/8/8/8/8/8/R1K5 b - - 1 1",
            "chess960={chess960}: b1c1 is the quiet king move"
        );
    }
    // Kf1, Rh1, g1 empty: f1g1 is a quiet king move and f1h1 the castle.
    let fen = "6k1/8/8/8/8/8/8/5K1R w H - 0 1";
    for chess960 in [false, true] {
        assert_castles(chess960, fen, "f1h1", "f1h1");
        let mut s = session_with(chess960);
        s.handle_line(&format!("position fen {fen} moves f1g1"));
        assert_eq!(
            s.board().to_fen(FenStyle::Shredder),
            "6k1/8/8/8/8/8/8/6KR b - - 1 1",
            "chess960={chess960}: f1g1 is the quiet king move"
        );
    }
    // The corpus's immediate castles, both spellings where they differ.
    for row in support::tsv("immediate-castles") {
        let (fen, ktr) = (&row[3], &row[4]);
        for chess960 in [false, true] {
            assert_castles(chess960, fen, ktr, ktr);
        }
    }
}

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

#[test]
fn setoption_uci_chess960_is_read_case_insensitively_and_persists() {
    let mut s = Session::new();
    assert!(!s.chess960());
    s.handle_line("setoption name UCI_Chess960 value true");
    assert!(s.chess960());
    s.handle_line("setoption name UCI_Chess960 value False");
    assert!(!s.chess960());
    s.handle_line("setoption name UCI_Chess960 value TRUE");
    assert!(s.chess960());
    // Other commands do not reset it.
    s.handle_line("ucinewgame");
    s.handle_line("position startpos");
    assert!(s.chess960());
    // Unknown options and malformed lines are ignored, not fatal.
    s.handle_line("setoption name Hash value 64");
    s.handle_line("setoption name UCI_Chess960");
    s.handle_line("setoption");
    assert!(s.chess960());
}

// ---------------------------------------------------------------------------
// State after bad input: defined, observable, and never half-applied
// ---------------------------------------------------------------------------
//
// The failure being guarded against is a half-applied position -- a board
// and a history that disagree. After an illegal move the board is exactly
// where the replay stopped and the history is exactly the moves applied;
// after a malformed FEN nothing has changed at all. In both cases the next
// `go` is legal in the position the board shows.

use std::sync::atomic::AtomicBool;

use cadence_core::parse_uci;
use cadence_engine::search::{Limits, Search};

/// The position reached by replaying `moves` from `fen` with `play`, built
/// independently of the handler.
fn replayed(fen: &str, moves: &[&str]) -> Board {
    let mut board = Board::from_fen(fen).expect("fen parses");
    for m in moves {
        let legal = generate_legal(&board);
        let m = parse_uci(&legal, m).unwrap_or_else(|| panic!("{m} is legal in {board:?}"));
        board.play(m);
    }
    board
}

/// Board, both FENs, key, ply and the full history agree.
fn assert_same_position(got: &Board, want: &Board, context: &str) {
    assert_eq!(shredder(got), shredder(want), "{context}");
    assert_eq!(
        got.to_fen(FenStyle::XFen),
        want.to_fen(FenStyle::XFen),
        "{context}"
    );
    assert_eq!(got.key(), want.key(), "{context}");
    assert_eq!(got.ply(), 0, "{context}");
    assert_eq!(
        got.game_history(),
        want.game_history(),
        "{context}: history"
    );
}

/// The search, run on a duplicate of the session's board, returns a move
/// legal in the position the board shows.
fn assert_go_is_legal_here(s: &Session, context: &str) {
    let mut board = s.board().duplicate();
    let stop = AtomicBool::new(false);
    let mut sink = Vec::new();
    let tt = support::table();
    let m = Search::new(Limits::depth(1), &stop, &tt).run(&mut board, &mut sink);
    let legal = generate_legal(s.board());
    if legal.is_empty() {
        assert!(m.is_null(), "{context}");
    } else {
        assert!(
            legal.contains(m),
            "{context}: {m:?} is not legal in {:?}",
            s.board()
        );
    }
}

#[test]
fn after_an_illegal_move_the_board_is_exactly_where_the_replay_stopped() {
    for (line, fen, applied) in [
        (
            "position startpos moves e2e4 e7e5 e2e4",
            START_FEN,
            vec!["e2e4", "e7e5"],
        ),
        ("position startpos moves e2e5", START_FEN, vec![]),
        (
            "position startpos moves e2e4 e7e5 g1f3 Nc6 b8c6",
            START_FEN,
            vec!["e2e4", "e7e5", "g1f3"],
        ),
        // The bad token in the middle: everything after it is dropped, even
        // though it would have been legal.
        (
            "position startpos moves d2d4 d7d5 c2c4 zzz c7c6",
            START_FEN,
            vec!["d2d4", "d7d5", "c2c4"],
        ),
        // From a FEN, with a castle in the applied prefix.
        (
            "position fen 1k6/8/8/8/8/8/8/2K4R w H - 0 1 moves c1g1 b8a8 h1h8",
            "1k6/8/8/8/8/8/8/2K4R w H - 0 1",
            vec!["c1g1", "b8a8"],
        ),
        // A null move is never legal, and neither is a move for the wrong side.
        (
            "position startpos moves e2e4 0000 e7e5",
            START_FEN,
            vec!["e2e4"],
        ),
        ("position startpos moves e7e5", START_FEN, vec![]),
    ] {
        let want = replayed(fen, &applied);
        let mut s = Session::new();
        assert!(s.handle_line(line));
        assert_same_position(s.board(), &want, line);
        assert_eq!(
            s.board().game_history().len(),
            applied.len(),
            "{line}: one history entry per move applied, none for the failed one"
        );
        assert_go_is_legal_here(&s, line);
    }
}

#[test]
fn after_a_malformed_fen_the_previous_position_and_history_are_intact() {
    let mut s = Session::new();
    s.handle_line("position startpos moves e2e4 e7e5 g1f3");
    let want = replayed(START_FEN, &["e2e4", "e7e5", "g1f3"]);
    assert_same_position(s.board(), &want, "setup");
    for line in [
        "position fen",
        "position fen not a fen at all",
        "position fen 8/8/8/8/8/8/8/8 w - - 0 1",
        "position fen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR x KQkq - 0 1",
        "position fen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - x 1",
        // A bad FEN followed by moves: the moves must not be applied to the
        // position that was there before.
        "position fen not a fen moves b8c6 f1b5",
        "position fen 8/8/8/8/8/8/8/8 w - - 0 1 moves b8c6",
        "position fen rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR x KQkq - 0 1 moves b8c6",
        "position",
        "position somethingelse moves b8c6",
    ] {
        assert!(s.handle_line(line), "{line}");
        assert_same_position(s.board(), &want, line);
        assert_eq!(s.board().game_history().len(), 3, "{line}");
        assert_go_is_legal_here(&s, line);
    }
    // And the session is still fully usable afterwards.
    s.handle_line("position startpos moves d2d4");
    assert_same_position(
        s.board(),
        &replayed(START_FEN, &["d2d4"]),
        "after the bad lines",
    );
}

/// The history the handler builds is what repetition detection reads.
#[test]
fn the_history_the_handler_builds_feeds_repetition_detection() {
    let loop_once = "g1f3 g8f6 f3g1 f6g8";
    let mut s = Session::new();
    s.handle_line(&format!("position startpos moves {loop_once}"));
    assert!(
        !s.board().is_repetition(),
        "the start position twice is not yet a repetition"
    );
    s.handle_line(&format!("position startpos moves {loop_once} {loop_once}"));
    assert!(
        s.board().is_repetition(),
        "the start position for the third time, all of it in the history"
    );
    assert_eq!(s.board().game_history().len(), 8);
    // A failed move after the loop leaves the loop applied and nothing else.
    s.handle_line(&format!(
        "position startpos moves {loop_once} {loop_once} zzz e2e4"
    ));
    assert!(s.board().is_repetition());
    assert_eq!(s.board().game_history().len(), 8);
    // And a failed move INSIDE the loop leaves only the prefix: no
    // repetition, and no history entry for anything after the failure.
    s.handle_line(&format!(
        "position startpos moves {loop_once} g1f3 zzz g8f6 f3g1 f6g8"
    ));
    assert!(!s.board().is_repetition());
    assert_eq!(s.board().game_history().len(), 5);
    assert_same_position(
        s.board(),
        &replayed(START_FEN, &["g1f3", "g8f6", "f3g1", "f6g8", "g1f3"]),
        "prefix",
    );
}
