// SPDX-License-Identifier: GPL-3.0-or-later

//! Drives the real binary over a pipe. The UCI surface is the one interface
//! a GUI sees, and it is not exercised by anything that links the crate as a
//! library, so the plumbing -- `go` on its own thread, `stop` raising the
//! flag and waiting for `bestmove`, `isready` answered mid-search, `quit`
//! during a search -- is tested as a subprocess or not at all.
//!
//! The handlers themselves are tested as functions in `position_handler.rs`
//! and `bestmove.rs`; what is checked here is that the right line comes out
//! of the pipe, in the right order, and that the process comes back.

mod support;

use std::process::Command;

use cadence_core::position::Board;
use cadence_core::{START_FEN, generate_legal, parse_uci, to_uci};
use support::{Engine, bestmove, bestmoves, talk, talk_bytes};

#[test]
fn uci_reports_identity() {
    let out = talk("uci\nquit\n");
    let lines: Vec<&str> = out.lines().collect();
    // Against `version::VERSION` and not the package version: what is being
    // checked is that whatever this build calls itself is what comes out of
    // the pipe. A dev build reports the commit, and a GUI reading this line
    // is how anyone tells two of them apart.
    assert!(
        lines.contains(&format!("id name Cadence {}", cadence_engine::version::VERSION).as_str()),
        "no id name line in {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("id author ")),
        "no id author line in {lines:?}"
    );
    assert_eq!(
        lines.last(),
        Some(&"uciok"),
        "uciok must be the last line of the uci reply, in {lines:?}"
    );
}

/// Cute Chess and other GUIs gate Chess960 games on the engine declaring
/// the option. An engine that parses FRC perfectly but does not announce it
/// cannot be given an FRC game at all.
#[test]
fn uci_advertises_uci_chess960() {
    let out = talk("uci\nquit\n");
    assert!(
        out.lines()
            .any(|l| l == "option name UCI_Chess960 type check default false"),
        "no UCI_Chess960 option line in {out:?}"
    );
    // Every option line comes before uciok.
    let uciok = out.lines().position(|l| l == "uciok").expect("uciok");
    for (i, l) in out.lines().enumerate() {
        if l.starts_with("option ") {
            assert!(i < uciok, "option line after uciok: {l}");
        }
    }
}

#[test]
fn isready_reports_readyok() {
    assert!(talk("isready\nquit\n").lines().any(|l| l == "readyok"));
}

#[test]
fn unknown_commands_are_ignored_not_fatal() {
    let out = talk("frobnicate the bishop\n\n   \nisready\nquit\n");
    assert!(out.lines().any(|l| l == "readyok"));
}

#[test]
fn quit_stops_reading() {
    // Anything after `quit` must not be answered.
    let out = talk("quit\nisready\n");
    assert!(!out.contains("readyok"), "kept reading past quit: {out:?}");
}

#[test]
fn end_of_input_exits_cleanly() {
    // No commands at all: the banner, then exit 0. This is the smoke test CI
    // relies on.
    let out = talk("");
    assert!(out.starts_with("Cadence "), "no banner in {out:?}");
}

#[test]
fn unknown_subcommand_is_a_usage_error() {
    let out = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .arg("frobnicate")
        .output()
        .expect("run cadence");
    assert_eq!(out.status.code(), Some(2), "expected a usage exit code");
    let err = String::from_utf8(out.stderr).expect("stderr is UTF-8");
    assert!(err.contains("unknown subcommand"), "{err:?}");
}

/// A byte that is not UTF-8 must not end the session. `BufRead::lines` turns
/// it into an `Err`, and an earlier loop treated that as end of input: exit
/// 0, no message, and a GUI reporting a crash with nothing to attribute it
/// to -- the worst shape a fault can have.
#[test]
fn invalid_utf8_does_not_end_the_session() {
    let out = talk_bytes(b"\xff\nisready\nquit\n");
    assert!(
        out.lines().any(|l| l == "readyok"),
        "the session died on a stray byte: {out:?}"
    );
}

/// The same byte inside a value a GUI might actually send: a Latin-1 path.
/// The option is not understood and is ignored; the session survives and
/// the next command is answered.
#[test]
fn latin1_in_a_setoption_value_is_tolerated() {
    let out = talk_bytes(b"setoption name EvalFile value /home/Jos\xe9/net.bin\nisready\nquit\n");
    assert!(
        out.lines().any(|l| l == "readyok"),
        "the session died on a Latin-1 path: {out:?}"
    );
}

// ---------------------------------------------------------------------------
// go / stop / bestmove
// ---------------------------------------------------------------------------

/// The `bestmove` of `out` parses against the legal moves of `fen` and is
/// spelled the way `to_uci` spells it under `chess960`.
fn assert_bestmove_legal(out: &str, fen: &str, chess960: bool) {
    let mv = bestmove(out);
    let board = Board::from_fen(fen).expect("fen parses");
    let legal = generate_legal(&board);
    let m = parse_uci(&legal, &mv)
        .unwrap_or_else(|| panic!("bestmove {mv} is not legal in {fen}; output {out:?}"));
    assert_eq!(
        mv,
        to_uci(m, &legal, chess960),
        "bestmove spelled for the wrong UCI_Chess960 value (chess960={chess960})"
    );
}

#[test]
fn go_depth_yields_a_legal_bestmove() {
    let out = talk("position startpos\ngo depth 1\nquit\n");
    assert_bestmove_legal(&out, START_FEN, false);
}

#[test]
fn go_without_a_position_searches_the_start_position() {
    let out = talk("go depth 1\nquit\n");
    assert_bestmove_legal(&out, START_FEN, false);
}

#[test]
fn go_with_a_clock_yields_a_legal_bestmove() {
    let fen = "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";
    let out = talk(&format!(
        "position fen {fen}\ngo wtime 1000 btime 1000 winc 10 binc 10\nquit\n"
    ));
    assert_bestmove_legal(&out, fen, false);
    let out = talk(&format!("position fen {fen}\ngo movetime 50\nquit\n"));
    assert_bestmove_legal(&out, fen, false);
    // Bare `go`: no limit applies; `stop` ends it.
    let out = talk(&format!("position fen {fen}\ngo\nstop\nquit\n"));
    assert_bestmove_legal(&out, fen, false);
}

#[test]
fn go_after_a_moves_list_searches_the_position_reached() {
    // After 1.e4 e5 2.Nf3 Nc6 3.Bb5 a6, it is White to move in the Ruy Lopez.
    let out = talk("position startpos moves e2e4 e7e5 g1f3 b8c6 f1b5 a7a6\ngo depth 1\nquit\n");
    assert_bestmove_legal(
        &out,
        "r1bqkbnr/1ppp1ppp/p1n5/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R w KQkq - 0 4",
        false,
    );
}

/// `go infinite` must not return on its own. `stop` brings the `bestmove`,
/// and `isready` is answered in between without ending the search.
#[test]
fn go_infinite_then_stop_yields_a_bestmove_and_isready_is_answered_meanwhile() {
    let out = talk("position startpos\ngo infinite\nisready\nstop\nquit\n");
    assert_bestmove_legal(&out, START_FEN, false);
    let lines: Vec<&str> = out.lines().collect();
    let readyok = lines.iter().position(|l| *l == "readyok").expect("readyok");
    let best = lines
        .iter()
        .position(|l| l.starts_with("bestmove "))
        .expect("bestmove");
    assert!(
        readyok < best,
        "readyok must come before the bestmove that stop produces: {lines:?}"
    );
    assert_eq!(bestmoves(&out).len(), 1, "exactly one bestmove: {out:?}");
}

#[test]
fn quit_during_an_infinite_search_exits() {
    // No stop: quit must end the search and the process. The subprocess
    // helper fails the test if the process does not come back.
    let out = talk("position startpos\ngo infinite\nquit\n");
    // Whether a bestmove is printed on quit is not specified; if one is, it
    // is legal.
    if !bestmoves(&out).is_empty() {
        assert_bestmove_legal(&out, START_FEN, false);
    }
}

#[test]
fn a_second_go_stops_the_first() {
    let out = talk("position startpos\ngo infinite\ngo infinite\nstop\nquit\n");
    let moves = bestmoves(&out);
    assert_eq!(moves.len(), 2, "one bestmove per go: {out:?}");
}

#[test]
fn stop_without_a_search_is_harmless() {
    let out = talk("stop\nisready\nstop\nquit\n");
    assert!(out.lines().any(|l| l == "readyok"));
    assert!(bestmoves(&out).is_empty());
}

#[test]
fn ucinewgame_is_accepted_and_the_next_go_works() {
    let out = talk("ucinewgame\nisready\nposition startpos\ngo depth 1\nquit\n");
    assert!(out.lines().any(|l| l == "readyok"));
    assert_bestmove_legal(&out, START_FEN, false);
}

#[test]
fn bestmove_on_a_position_with_no_legal_move_is_the_null_move() {
    for fen in [
        "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1", // mated
        "7k/8/6Q1/8/8/8/8/7K b - - 0 1",  // stalemated
    ] {
        let out = talk(&format!("position fen {fen}\ngo depth 1\nquit\n"));
        assert_eq!(bestmove(&out), "0000", "{fen}: {out:?}");
    }
}

/// Castling spelled per `UCI_Chess960`: king-takes-rook when it is on,
/// king-to-destination when it is off and unambiguous.
///
/// Whichever move the engine picks, the spelling must be the one `to_uci`
/// gives under the option value in force, and `assert_bestmove_legal`
/// checks that for every position here, castle or not. A castle is what
/// exercises the branch, so the positions are ones where castling is legal
/// and the test asserts that at least one of them produced a castling
/// bestmove -- so a change in the move chooser that stops reaching the
/// branch is noticed rather than silently passing.
#[test]
fn castling_bestmove_is_spelled_per_the_option() {
    let fens = [
        "1k6/8/8/8/8/8/8/2K4R w H - 0 1",
        "1k6/8/8/8/8/8/8/RK6 w A - 0 1",
        "4k3/8/8/8/8/8/8/R3K3 w Q - 0 1",
        "4k3/8/8/8/8/8/8/4K2R w K - 0 1",
        "r3k3/8/8/8/8/8/8/4K3 b q - 0 1",
        "4k2r/8/8/8/8/8/8/4K3 b k - 0 1",
        "4k3/8/8/8/8/8/8/R3K2R w KQ - 0 1",
        "r3k2r/8/8/8/8/8/8/4K3 b kq - 0 1",
        // The bare-king positions above are endgames, where the evaluation
        // rightly prefers centralising the king to castling it; these four
        // have enough material on the board that castling is what the
        // search chooses, standard and DFRC, both colours.
        "r2qk2r/pppppppp/2n2n2/8/8/2N2N2/PPPPPPPP/R2QK2R w Kk - 0 1",
        "r2qk2r/pppppppp/2n2n2/8/8/2N2N2/PPPPPPPP/R2QK2R b Kk - 0 1",
        "nnrkqbbr/pppppppp/8/8/8/8/PPPPPPPP/NNRKQBBR w HChc - 0 1",
        "nnrkqbbr/pppppppp/8/8/8/8/PPPPPPPP/NNRKQBBR b HChc - 0 1",
    ];
    let mut castles = 0;
    for chess960 in [false, true] {
        for fen in fens {
            let value = if chess960 { "true" } else { "false" };
            let out = talk(&format!(
                "setoption name UCI_Chess960 value {value}\nposition fen {fen}\ngo depth 1\nquit\n"
            ));
            assert_bestmove_legal(&out, fen, chess960);
            let board = Board::from_fen(fen).expect("fen parses");
            let legal = generate_legal(&board);
            let m = parse_uci(&legal, &bestmove(&out)).expect("checked legal above");
            if m.is_castle() {
                castles += 1;
            }
        }
    }
    assert!(
        castles >= 4,
        "only {castles} castling bestmoves, so the spelling branch went under-tested"
    );
}

/// Over the pipe: after an illegal move in the list, `go` searches the
/// position where the replay stopped, and its bestmove is legal there.
#[test]
fn go_after_an_illegal_move_searches_where_the_replay_stopped() {
    let out = talk("position startpos moves e2e4 e7e5 e2e4\ngo depth 1\nquit\n");
    assert_bestmove_legal(
        &out,
        "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2",
        false,
    );
    assert!(
        out.lines().any(|l| l.starts_with("info string position:")),
        "the rejected move is reported: {out:?}"
    );
    // And after a malformed FEN, the previous position is what is searched.
    let out = talk(
        "position startpos moves e2e4 e7e5\nposition fen garbage moves g1f3\ngo depth 1\nquit\n",
    );
    assert_bestmove_legal(
        &out,
        "rnbqkbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 2",
        false,
    );
}

/// A `go` on a position no legal play can reach comes back with a move, and
/// the process is still there to be asked for another.
///
/// Regression: the side not to move being in check made generation offer
/// the king capture, `make_move` remove the king, and the
/// recomputed check info ask for a king that was no longer on the board.
/// With `panic = "abort"` in the release profile the whole process went,
/// SIGABRT, mid-search.
///
/// Driven with `Engine::go_within` rather than `talk`, and that is
/// load-bearing twice over. `talk` pipes `quit` in with everything else,
/// `quit` stops a running search, and a search stopped before it makes its
/// first root move does not reach the fault at all: the bug reproduces only
/// when the search is allowed to run. And the deadline is not decoration --
/// against the unfixed engine this test *hangs* rather than fails, because
/// the test profile does not abort on panic, so the search thread unwinds
/// and dies while the UCI loop reads on.
#[test]
fn go_on_a_position_with_the_side_not_to_move_in_check_returns_a_move() {
    for fen in [
        "k7/8/8/8/8/8/8/R6K w - - 0 1",
        "4k3/8/8/8/8/8/8/4R2K w - - 0 1",
        // Adjacent kings: the same abort reached through the checkers rather
        // than through the target sets.
        "kK6/8/8/8/8/8/8/8 w - - 0 1",
    ] {
        let (_, lines) = Engine::go_within(
            &[&format!("position fen {fen}")],
            "go depth 4",
            std::time::Duration::from_secs(20),
        );
        assert_bestmove_legal(&lines.join("\n"), fen, false);
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("info string position:") && l.contains("not to move")),
            "{fen}: the position is not named as illegal: {lines:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The info fields a watcher reads
// ---------------------------------------------------------------------------

/// The value `name` carries on a UCI `info` line, parsed.
///
/// Token position and not a byte offset, because that is how a GUI reads one
/// and it is what lets a field be inserted without moving the others.
fn field<T: std::str::FromStr>(line: &str, name: &str) -> Option<T> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let at = toks.iter().position(|t| *t == name)?;
    toks.get(at + 1)?.parse().ok()
}

/// Every iteration line carries a `seldepth`, and it is never below the
/// `depth` beside it.
///
/// The second assertion is what stops the field being `depth` under another
/// name: quiescence searches past the horizon, so a real reading is strictly
/// deeper on most iterations and this one demands it on at least one.
#[test]
fn iteration_lines_carry_a_seldepth_the_quiescence_search_pushes_past_the_depth() {
    let out = Engine::go(&["position startpos"], "go depth 10").join("\n");
    let lines: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("info depth "))
        .collect();
    assert_eq!(lines.len(), 10, "{out}");
    let mut deeper = 0;
    for line in &lines {
        let depth: usize = field(line, "depth").unwrap_or_else(|| panic!("no depth on {line}"));
        let seldepth: usize =
            field(line, "seldepth").unwrap_or_else(|| panic!("no seldepth on {line}"));
        assert!(
            seldepth >= depth,
            "seldepth {seldepth} is above the horizon of a depth {depth} iteration: {line}"
        );
        assert!(seldepth <= cadence_core::MAX_PLY, "{line}");
        if seldepth > depth {
            deeper += 1;
        }
    }
    assert!(
        deeper > 0,
        "no iteration reached past its own depth, so `seldepth` is spelling `depth`: {out}"
    );
}

/// `hashfull` is a permill, it never falls inside one search, and
/// `ucinewgame` puts it back.
///
/// Rising and falling are both asserted because either alone passes against
/// a constant: a field wired to zero never falls, and one wired to the depth
/// never returns.
#[test]
fn hashfull_rises_within_a_search_and_ucinewgame_puts_it_back() {
    let permills = |lines: &[String]| -> Vec<u32> {
        lines
            .iter()
            .filter(|l| l.starts_with("info depth "))
            .map(|l| field(l, "hashfull").unwrap_or_else(|| panic!("no hashfull on {l}")))
            .collect()
    };
    let mut engine = Engine::spawn();
    engine.sync();
    engine.send("position startpos");
    engine.send("go depth 16");
    let filling = permills(&engine.read_until("bestmove "));
    assert_eq!(filling.len(), 16, "{filling:?}");
    for pair in filling.windows(2) {
        assert!(
            pair[1] >= pair[0],
            "hashfull fell from {} to {} inside one search: {filling:?}",
            pair[0],
            pair[1]
        );
    }
    for &permill in &filling {
        assert!(permill <= 1000, "hashfull {permill} is not a permill");
    }
    let filled = *filling.last().expect("an iteration");
    assert!(
        filled > 0,
        "a depth 16 search left the table reading empty: {filling:?}"
    );
    engine.send("ucinewgame");
    engine.send("position startpos");
    engine.send("go depth 1");
    let emptied = permills(&engine.read_until("bestmove "));
    assert!(
        emptied.first().is_some_and(|&p| p < filled),
        "ucinewgame emptied the table and hashfull went from {filled} to {emptied:?}"
    );
    engine.quit();
}

/// A clocked search past [`cadence_engine::search::CURRMOVE_AFTER_MS`] names
/// the root move it is on; a node-limited search over the same tree names
/// none.
///
/// The pair is one test because the second half is only worth anything
/// against a first half that fired: the node limit is taken from the clocked
/// run, so the two searches cost the same and the difference between them is
/// the limit rather than the machine.
#[test]
fn a_clocked_search_names_its_root_move_and_a_node_limited_one_stays_silent() {
    let watched = cadence_engine::search::CURRMOVE_AFTER_MS * 2;
    let (elapsed, clocked) = Engine::go_within(
        &["position startpos"],
        &format!("go movetime {watched}"),
        std::time::Duration::from_secs(60),
    );
    // The one thing `bench` cannot say anything about: these lines are
    // written down a live pipe while the clock runs, and a search that
    // overshoots its movetime writing them loses games rather than nodes.
    assert!(
        elapsed < std::time::Duration::from_millis(watched * 2),
        "a {watched} ms search took {elapsed:?} while naming its root moves"
    );
    let named: Vec<&String> = clocked
        .iter()
        .filter(|l| l.starts_with("info currmove "))
        .collect();
    assert!(
        !named.is_empty(),
        "a {watched} ms search named no root move: {clocked:?}"
    );

    let board = Board::from_fen(START_FEN).expect("the start position");
    let legal = generate_legal(&board);
    let mut numbers = Vec::new();
    for line in &named {
        let toks: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            toks.len(),
            5,
            "a currmove line is `info currmove <move> currmovenumber <n>`: {line}"
        );
        assert_eq!(toks[3], "currmovenumber", "{line}");
        assert!(
            parse_uci(&legal, toks[2]).is_some(),
            "`{}` is not a legal move at the root: {line}",
            toks[2]
        );
        let number: usize = toks[4].parse().unwrap_or_else(|_| panic!("{line}"));
        assert!(
            (1..=legal.len()).contains(&number),
            "currmovenumber {number} is outside the {} root moves: {line}",
            legal.len()
        );
        numbers.push(number);
    }
    // The number is a place in the root list, so it climbs through an
    // iteration and the only value it may fall to is the next iteration's
    // first.
    for pair in numbers.windows(2) {
        assert!(
            pair[1] > pair[0] || pair[1] == 1,
            "currmovenumber went {} then {}: {numbers:?}",
            pair[0],
            pair[1]
        );
    }

    let nodes: u64 = clocked
        .iter()
        .rfind(|l| l.starts_with("info depth "))
        .and_then(|l| field(l, "nodes"))
        .expect("a completed iteration");
    let (_, counted) = Engine::go_within(
        &["position startpos"],
        &format!("go nodes {nodes}"),
        std::time::Duration::from_secs(60),
    );
    assert!(
        !counted.iter().any(|l| l.contains("currmove")),
        "a node-limited search of {nodes} nodes named a root move: {counted:?}"
    );
}
