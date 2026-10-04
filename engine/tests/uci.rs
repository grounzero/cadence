// SPDX-License-Identifier: GPL-3.0-or-later

//! The UCI plumbing is tested as a subprocess or not at all: nothing linking the crate as a library
//! exercises it. The handlers are tested as functions in `position_handler.rs` and `bestmove.rs`.

mod support;

use std::process::Command;

use cadence_core::position::Board;
use cadence_core::{START_FEN, generate_legal, parse_uci, to_uci};
use support::{Engine, bestmove, bestmoves, talk, talk_bytes};

#[test]
fn uci_reports_identity() {
    let out = talk("uci\nquit\n");
    let lines: Vec<&str> = out.lines().collect();
    // Against `version::VERSION`, not the package version: a dev build reports its commit, which is
    // how a GUI tells two builds apart.
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

/// Cute Chess and other GUIs give a Chess960 game only to an engine that declares the option.
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
    // No commands: the banner, then exit 0, the smoke test CI relies on.
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

/// `BufRead::lines` turns a non-UTF-8 byte into an `Err`, which an earlier loop read as end of
/// input: exit 0, no message, a crash a GUI cannot attribute.
#[test]
fn invalid_utf8_does_not_end_the_session() {
    let out = talk_bytes(b"\xff\nisready\nquit\n");
    assert!(
        out.lines().any(|l| l == "readyok"),
        "the session died on a stray byte: {out:?}"
    );
}

/// The option is not understood and is ignored; the session survives.
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

/// Spelled the way `to_uci` spells it under `chess960`.
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
fn multicore_go_yields_one_legal_bestmove() {
    let (_, lines) = Engine::go_within(
        &["setoption name Threads value 4", "position startpos"],
        "go depth 3",
        std::time::Duration::from_secs(20),
    );
    let out = lines.join("\n");
    assert_bestmove_legal(&out, START_FEN, false);
    assert_eq!(bestmoves(&out).len(), 1, "exactly one bestmove: {out:?}");
    assert!(
        out.lines().any(|line| line.starts_with("info depth 3 ")),
        "the finite search completed rather than being stopped early: {out:?}"
    );
}

#[test]
fn multicore_infinite_search_stops_cleanly() {
    let out = talk(
        "setoption name Threads value 4\nposition startpos\ngo infinite\nisready\nstop\nquit\n",
    );
    assert_bestmove_legal(&out, START_FEN, false);
    assert_eq!(bestmoves(&out).len(), 1, "exactly one bestmove: {out:?}");
    assert!(out.lines().any(|line| line == "readyok"), "{out:?}");
}

/// Strictly greater, not a ratio: a group that contributed nothing reports exactly the
/// single-thread count. The summation is asserted deterministically in `search.rs`; this half
/// depends on the helpers being scheduled.
#[test]
fn four_threads_out_node_one_at_a_fixed_depth() {
    // Kiwipete at depth eight is about 230,000 nodes, where four threads read about 3.7 times one;
    // the start position's tree was too small to separate them on a three-core runner.
    const DEPTH: u32 = 8;
    let one = nodes_at_fixed_depth(1, DEPTH);
    let four = nodes_at_fixed_depth(4, DEPTH);
    assert!(
        four > one,
        "four threads reported {four} nodes at depth {DEPTH} against one thread's {one}; \
         equal means the group contributed nothing, which is a single-threaded host"
    );
}

/// Kiwipete, whose depth-eight tree leaves the helpers room to contribute.
const THREAD_POSITION: &str =
    "position fen r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1";

/// Fixed depth rather than time, because duplicated work scales with the workers however few
/// cores they share.
fn nodes_at_fixed_depth(threads: usize, depth: u32) -> u64 {
    let option = format!("setoption name Threads value {threads}");
    let (_, lines) = Engine::go_within(
        &[&option, THREAD_POSITION],
        &format!("go depth {depth}"),
        std::time::Duration::from_secs(60),
    );
    let prefix = format!("info depth {depth} ");
    let line = lines
        .iter()
        .rfind(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` line at Threads={threads} in {lines:?}"));
    let mut fields = line.split_whitespace();
    while let Some(field) = fields.next() {
        if field == "nodes" {
            return fields
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("unparsable nodes in `{line}`"));
        }
    }
    panic!("no nodes field in `{line}`")
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

/// `isready` is answered without ending the search.
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
    // The subprocess helper fails the test if the process does not come back.
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

/// A castle is what exercises the branch, so at least one castling bestmove is required. It sits at
/// its threshold, four of four, and the search is deterministic, so a failure on every machine is a
/// king-table change and one on some runners a race.
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
        // With enough material that castling is what the search chooses, standard and DFRC, both
        // colours.
        "r2qk2r/pppppppp/2n2n2/8/8/2N2N2/PPPPPPPP/R2QK2R w Kk - 0 1",
        "r2qk2r/pppppppp/2n2n2/8/8/2N2N2/PPPPPPPP/R2QK2R b Kk - 0 1",
        "nnrkqbbr/pppppppp/8/8/8/8/PPPPPPPP/NNRKQBBR w HChc - 0 1",
        "nnrkqbbr/pppppppp/8/8/8/8/PPPPPPPP/NNRKQBBR b HChc - 0 1",
    ];
    let mut castles = 0;
    for chess960 in [false, true] {
        for fen in fens {
            let value = if chess960 { "true" } else { "false" };
            // Read to the bestmove before quitting: `quit` stops a running search, so the move
            // would turn on whether depth one had finished.
            let option = format!("setoption name UCI_Chess960 value {value}");
            let position = format!("position fen {fen}");
            let (_, lines) = Engine::go_within(
                &[option.as_str(), position.as_str()],
                "go depth 1",
                support::SUBPROCESS_TIMEOUT,
            );
            let out = lines.join("\n");
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

/// The side not to move in check once made generation offer the king capture and abort the process
/// mid-search. `Engine::go_within`, because `talk`'s piped `quit` stops the search before the
/// fault, and the deadline because the unfixed engine hangs rather than fails in the test profile.
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

/// By token position, as a GUI reads it, so a field can be inserted without moving the others.
fn field<T: std::str::FromStr>(line: &str, name: &str) -> Option<T> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let at = toks.iter().position(|t| *t == name)?;
    toks.get(at + 1)?.parse().ok()
}

/// Quiescence searches past the horizon, so on at least one iteration `seldepth` must exceed
/// `depth`, or it is `depth` under another name.
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

/// Both rising and falling are asserted, because either alone passes against a constant.
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

/// The node limit is taken from the clocked run, so the two cost the same and differ only in the
/// limit.
#[test]
fn a_clocked_search_names_its_root_move_and_a_node_limited_one_stays_silent() {
    let watched = cadence_engine::search::CURRMOVE_AFTER_MS * 2;
    let (elapsed, clocked) = Engine::go_within(
        &["position startpos"],
        &format!("go movetime {watched}"),
        std::time::Duration::from_secs(60),
    );
    // Written down a live pipe while the clock runs, which `bench` cannot see: a search that
    // overshoots its movetime writing them loses games.
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
    // A place in the root list: it climbs, and may fall only to the next iteration's first.
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

/// Both are wall-clock readings of the run, not the tree.
fn iteration_lines(out: &[String]) -> Vec<String> {
    out.iter()
        .filter(|l| l.starts_with("info depth "))
        .map(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            let mut kept = Vec::new();
            let mut i = 0;
            while i < toks.len() {
                if toks[i] == "nps" || toks[i] == "time" {
                    i += 2;
                    continue;
                }
                kept.push(toks[i]);
                i += 1;
            }
            kept.join(" ")
        })
        .collect()
}

/// A mate maps outside the centipawn range, worth more than any score of the same sign.
fn score_key(line: &str) -> i64 {
    let toks: Vec<&str> = line.split_whitespace().collect();
    let at = toks
        .iter()
        .position(|t| *t == "score")
        .unwrap_or_else(|| panic!("no score on {line}"));
    let kind = toks.get(at + 1).unwrap_or_else(|| panic!("{line}"));
    let value: i64 = toks
        .get(at + 2)
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{line}"));
    match *kind {
        "cp" => value,
        "mate" if value > 0 => 1_000_000 - value,
        "mate" => -1_000_000 - value,
        other => panic!("score kind `{other}` on {line}"),
    }
}

/// The maximum is `MAX_MOVES`, since a root asked for more lines reports the moves it has.
#[test]
fn uci_advertises_multipv() {
    let out = talk("uci\nquit\n");
    let expected = format!(
        "option name MultiPV type spin default 1 min 1 max {}",
        cadence_core::MAX_MOVES
    );
    assert!(
        out.lines().any(|l| l == expected),
        "no `{expected}` line in {out:?}"
    );
}

/// The default is what every rating list and SPRT plays, so a line that moves here is a defect. A
/// single line carries no `multipv` field.
#[test]
fn multipv_one_emits_exactly_what_the_option_never_set_emits() {
    let untouched = Engine::go(&["position startpos"], "go depth 10");
    let asked = Engine::go(
        &["setoption name MultiPV value 1", "position startpos"],
        "go depth 10",
    );
    let a = iteration_lines(&untouched);
    let b = iteration_lines(&asked);
    assert_eq!(a.len(), 10, "{untouched:?}");
    assert_eq!(a, b, "MultiPV 1 moved the iteration lines");
    for line in a.iter().chain(&b) {
        assert!(
            !line.contains(" multipv "),
            "a single line was numbered: {line}"
        );
    }
}

/// Numbered from one and restarting each iteration, which a GUI reads to keep the panel in place.
#[test]
fn multipv_above_one_reports_distinct_legal_moves_in_descending_order() {
    let wanted = 4;
    let out = Engine::go(
        &[
            &format!("setoption name MultiPV value {wanted}"),
            "position startpos",
        ],
        "go depth 8",
    );
    let board = Board::from_fen(START_FEN).expect("the start position");
    let legal = generate_legal(&board);
    let lines = iteration_lines(&out);
    assert_eq!(lines.len(), 8 * wanted, "{out:?}");
    for iteration in lines.chunks(wanted) {
        let mut moves = Vec::new();
        let mut scores = Vec::new();
        for (i, line) in iteration.iter().enumerate() {
            let number: usize =
                field(line, "multipv").unwrap_or_else(|| panic!("no multipv on {line}"));
            assert_eq!(number, i + 1, "the lines are numbered out of order: {line}");
            let toks: Vec<&str> = line.split_whitespace().collect();
            let at = toks
                .iter()
                .position(|t| *t == "pv")
                .unwrap_or_else(|| panic!("no pv on {line}"));
            let first = toks
                .get(at + 1)
                .unwrap_or_else(|| panic!("empty pv: {line}"));
            assert!(
                parse_uci(&legal, first).is_some(),
                "`{first}` is not a legal move at the root: {line}"
            );
            moves.push((*first).to_string());
            scores.push(score_key(line));
        }
        let mut distinct = moves.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            moves.len(),
            "a move was reported twice: {moves:?}"
        );
        for pair in scores.windows(2) {
            assert!(
                pair[0] >= pair[1],
                "the lines are not in descending order: {scores:?}"
            );
        }
    }
}

/// A second root move is a second search, so the count has to rise, or the option is accepted and
/// ignored.
#[test]
fn multipv_is_bounded_by_the_root_moves_and_costs_nodes_above_one() {
    let board = Board::from_fen(START_FEN).expect("the start position");
    let roots = generate_legal(&board).len();
    let out = Engine::go(
        &[
            &format!("setoption name MultiPV value {}", roots + 5),
            "position startpos",
        ],
        "go depth 5",
    );
    let lines = iteration_lines(&out);
    assert_eq!(lines.len(), 5 * roots, "{out:?}");

    let nodes = |setup: &[&str]| -> u64 {
        let out = Engine::go(setup, "go depth 8");
        out.iter()
            .rfind(|l| l.starts_with("info depth "))
            .and_then(|l| field(l, "nodes"))
            .expect("a completed iteration")
    };
    let one = nodes(&["setoption name MultiPV value 1", "position startpos"]);
    let four = nodes(&["setoption name MultiPV value 4", "position startpos"]);
    assert!(
        four > one,
        "MultiPV 4 searched {four} nodes against MultiPV 1's {one}"
    );
}

// ---------------------------------------------------------------------------
// Ponder
// ---------------------------------------------------------------------------

/// The declaration makes `go ponder` reachable. Off by default, as every rating list and SPRT
/// plays.
#[test]
fn uci_advertises_ponder() {
    let out = talk("uci\nquit\n");
    assert!(
        out.lines()
            .any(|l| l == "option name Ponder type check default false"),
        "no Ponder option line in {out:?}"
    );
    let uciok = out.lines().position(|l| l == "uciok").expect("uciok");
    let at = out
        .lines()
        .position(|l| l.starts_with("option name Ponder "))
        .expect("the Ponder option");
    assert!(at < uciok, "the Ponder option is declared after uciok");
}

/// The ponder token must not reach the wire under the default every rating list and SPRT plays.
#[test]
fn ponder_off_emits_exactly_what_the_option_never_set_emits() {
    let untouched = Engine::go(&["position startpos"], "go depth 10");
    let off = Engine::go(
        &["setoption name Ponder value false", "position startpos"],
        "go depth 10",
    );
    assert_eq!(iteration_lines(&untouched).len(), 10, "{untouched:?}");
    assert_eq!(
        iteration_lines(&untouched),
        iteration_lines(&off),
        "Ponder off moved the iteration lines"
    );
    for lines in [&untouched, &off] {
        let best = lines.last().expect("a bestmove line");
        assert!(
            !best.contains(" ponder "),
            "a ponder move with the option off: {best}"
        );
    }
}

/// The move the engine expects the opponent to play, the one position worth thinking about on their
/// clock.
#[test]
fn the_ponder_move_is_the_second_move_of_the_principal_variation() {
    let lines = Engine::go(
        &["setoption name Ponder value true", "position startpos"],
        "go depth 10",
    );
    let pv: Vec<String> = lines
        .iter()
        .rev()
        .find(|l| l.starts_with("info depth "))
        .and_then(|l| l.split(" pv ").nth(1))
        .map(|rest| rest.split_whitespace().map(str::to_string).collect())
        .expect("an info line with a pv");
    assert!(pv.len() >= 2, "the pv is too short to ponder on: {pv:?}");
    assert_eq!(
        lines.last().expect("a bestmove line"),
        &format!("bestmove {} ponder {}", pv[0], pv[1]),
        "the bestmove line does not carry the pv's second move"
    );

    // The root's move list cannot spell it.
    let mut board = Board::from_fen(START_FEN).expect("the start position");
    board.play(parse_uci(&generate_legal(&board), &pv[0]).expect("the best move is legal"));
    assert!(
        parse_uci(&generate_legal(&board), &pv[1]).is_some(),
        "the ponder move {} is not legal after {}",
        pv[1],
        pv[0]
    );
}
