// SPDX-License-Identifier: GPL-3.0-or-later

//! A game ends by the rules alone, a game is a function of its seed and number, a run's file is the
//! same at any thread count, a record replays to the ending it names, and extraction splits by game
//! and keeps no position in check.

use std::process::Command;

use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{Move, generate_legal, parse_uci};
use cadence_engine::datagen::game::{Ending, Outcome, ending};
use cadence_engine::datagen::opening::{self, RANDOM_PLIES, WINDOW};
use cadence_engine::datagen::record::Record;
use cadence_engine::datagen::{self, extract, game};
use cadence_engine::position::Position;
use cadence_engine::tt::Table;

fn table() -> Table {
    Table::new(datagen::HASH_MB).expect("a datagen table")
}

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

fn run(seed: u64, games: u64, threads: usize) -> Vec<u8> {
    let mut out = Vec::new();
    datagen::generate(seed, games, threads, &mut out, &mut |_| {}).expect("a run into memory");
    out
}

fn records(file: &[u8]) -> Vec<Record> {
    std::str::from_utf8(file)
        .expect("utf-8")
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| Record::parse(l).unwrap_or_else(|e| panic!("{l}: {e}")))
        .collect()
}

// Endings
// ---------------------------------------------------------------------------

#[test]
fn each_rule_ends_the_game_with_its_result() {
    let cases = [
        (
            "7k/6Q1/6K1/8/8/8/8/8 b - - 0 1",
            Ending::Mate,
            Outcome::White,
        ),
        (
            "8/8/8/8/8/6k1/6q1/7K w - - 0 1",
            Ending::Mate,
            Outcome::Black,
        ),
        (
            "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1",
            Ending::Stalemate,
            Outcome::Draw,
        ),
        (
            "8/8/8/4k3/8/8/8/R3K3 w - - 100 80",
            Ending::FiftyMoves,
            Outcome::Draw,
        ),
        (
            "8/8/8/4k3/8/8/8/2B1K3 w - - 0 1",
            Ending::InsufficientMaterial,
            Outcome::Draw,
        ),
        // A mate on the hundredth quiet ply is a mate and not a draw.
        (
            "7k/6Q1/6K1/8/8/8/8/8 b - - 100 90",
            Ending::Mate,
            Outcome::White,
        ),
    ];
    for (fen, want, result) in cases {
        assert_eq!(ending(&board(fen)), Some((want, result)), "{fen}");
    }
    assert_eq!(ending(&board(cadence_core::START_FEN)), None);
}

#[test]
fn the_third_occurrence_ends_the_game_and_the_second_does_not() {
    let mut b = board(cadence_core::START_FEN);
    let shuffle = ["g1f3", "g8f6", "f3g1", "f6g8"];
    for round in 0..2 {
        for text in shuffle {
            assert_eq!(ending(&b), None, "round {round} before {text}");
            let m = parse_uci(&generate_legal(&b), text).expect("legal");
            b.play(m);
        }
    }
    assert_eq!(ending(&b), Some((Ending::Repetition, Outcome::Draw)));
}

// Reproducibility
// ---------------------------------------------------------------------------

#[test]
fn a_game_is_a_function_of_its_seed_and_number_alone() {
    let fresh = datagen::play(11, 3, &table());
    let dirty = table();
    let _ = datagen::play(11, 4, &dirty);
    assert_eq!(datagen::play(11, 3, &dirty), fresh);
    assert_ne!(datagen::play(12, 3, &dirty), fresh);
}

#[test]
fn a_game_from_a_position_ignores_what_the_table_held() {
    let fen = "r1bqkbnr/pppp1ppp/2n5/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R w KQkq - 2 3";
    let fresh = game::play(&mut Position::new(board(fen)), datagen::NODES, &table());
    let dirty = table();
    // The same game played deeper leaves entries for the very positions the game will reach.
    let _ = game::play(&mut Position::new(board(fen)), 4 * datagen::NODES, &dirty);
    assert_eq!(
        game::play(&mut Position::new(board(fen)), datagen::NODES, &dirty),
        fresh
    );
}

#[test]
fn a_run_is_the_same_file_at_any_thread_count() {
    let one = run(5, 6, 1);
    assert_eq!(run(5, 6, 3), one);
    let numbers: Vec<u64> = records(&one).iter().map(|r| r.number).collect();
    assert_eq!(numbers, (0..6).collect::<Vec<_>>());
    let first = std::str::from_utf8(&one).expect("utf-8").lines().next();
    assert_eq!(first, Some(datagen::provenance(5, 6).as_str()));
}

// Openings
// ---------------------------------------------------------------------------

#[test]
fn a_start_that_passes_is_inside_the_window_after_its_random_plies() {
    let tt = table();
    let mut rng = Rng::new(3);
    for _ in 0..12 {
        let (mut o, _) = opening::next(&mut rng, &tt);
        assert_eq!(o.plies.len(), RANDOM_PLIES);
        assert!(o.white < 960 && o.black < 960);
        assert_eq!(ending(&o.board), None);
        let score = game::probe(&mut o.board, opening::SCREEN_NODES, &table());
        assert!(score.abs() <= WINDOW, "screened {score}");
    }
}

// Records
// ---------------------------------------------------------------------------

#[test]
fn a_record_reads_back_as_itself_and_replays_to_the_ending_it_names() {
    let file = run(9, 4, 2);
    let text = std::str::from_utf8(&file).expect("utf-8");
    for (line, record) in text.lines().skip(1).zip(records(&file)) {
        assert_eq!(record.line(), line);
        let mut b: Position = record.start();
        let moves: Vec<Move> = record
            .plies
            .iter()
            .copied()
            .chain(record.moves.iter().map(|&(m, _)| m))
            .collect();
        for m in moves {
            assert_eq!(ending(&b), None, "game {} ended early", record.number);
            b.play(m);
        }
        assert_eq!(ending(&b), Some((record.ending, record.outcome)));
    }
}

#[test]
fn a_malformed_record_is_refused() {
    let good = datagen::play(1, 0, &table()).line();
    assert!(Record::parse(&good).is_ok());
    let mut fields: Vec<String> = good.split(" | ").map(String::from).collect();
    fields[2] = format!("a1a1 {}", fields[2]);
    let bad_move = fields.join(" | ");
    for line in [
        "",
        "0 | 518 518",
        "0 | 960 518 |  |  | 1-0 mate | 0",
        "0 | 518 518 | e2e5 |  | 1-0 mate | 0",
        "0 | 518 518 |  | e2e4 |  | 0",
        bad_move.as_str(),
    ] {
        assert!(Record::parse(line).is_err(), "{line}");
    }
}

// Extraction
// ---------------------------------------------------------------------------

fn extract(file: &[u8], every: u64) -> (String, String, extract::Stats) {
    let (mut train, mut holdout) = (Vec::new(), Vec::new());
    let stats = extract::extract(file, &mut train, &mut holdout, every).expect("extracts");
    let s = |v: Vec<u8>| String::from_utf8(v).expect("utf-8");
    (s(train), s(holdout), stats)
}

fn positions(text: &str) -> Vec<&str> {
    text.lines().filter(|l| !l.starts_with('#')).collect()
}

#[test]
fn extraction_holds_out_whole_games_by_number() {
    let file = run(21, 12, 3);
    let (all_train, none, whole) = extract(&file, 0);
    assert!(positions(&none).is_empty());
    let (none_train, all_held, _) = extract(&file, 1);
    assert!(positions(&none_train).is_empty());
    assert_eq!(positions(&all_held), positions(&all_train));

    // Every fourth game: 0, 4 and 8, each wholly on one side.
    let (train, held, stats) = extract(&file, 4);
    let mut want_held = Vec::new();
    for r in records(&file) {
        let one: Vec<u8> = [r.line().as_bytes(), b"\n"].concat();
        let (lines, _, _) = extract(&one, 0);
        if r.number % 4 == 0 {
            want_held.extend(positions(&lines).iter().map(ToString::to_string));
        }
    }
    assert_eq!(positions(&held), want_held);
    assert_eq!(
        positions(&train).len() + positions(&held).len(),
        positions(&all_train).len()
    );
    assert_eq!(stats.train + stats.holdout, whole.train);
    assert_eq!(stats.games, 12);
}

#[test]
fn no_kept_position_is_in_check_and_every_count_adds_up() {
    let file = run(2, 6, 2);
    let (train, _, stats) = extract(&file, 0);
    for line in positions(&train) {
        let (fen, result) = line.rsplit_once(" | ").expect("fen | result");
        assert!(!board(fen).in_check(), "{fen}");
        assert!(["1-0", "0-1", "1/2-1/2"].contains(&result));
    }
    assert!(train.starts_with("# cadence datagen"));
    let kept = stats.train;
    assert_eq!(
        stats.outside + stats.in_check + stats.noisy + stats.mate + kept,
        stats.visited
    );
    // The window is counted from the records alone, so the extractor's count can be checked.
    let outside: usize = records(&file)
        .iter()
        .map(|r| {
            let n = r.moves.len();
            n.min(extract::FIRST_PLY) + n.saturating_sub(extract::LAST_PLY + 1)
        })
        .sum();
    assert_eq!(stats.outside, outside as u64);
    for dist in [&stats.phase, &stats.balance, &stats.ply] {
        assert_eq!(dist.iter().sum::<u64>(), kept);
    }
    let mut report = Vec::new();
    stats.report(&mut report).expect("reports");
    assert!(String::from_utf8(report).expect("utf-8").contains("yield"));
}

#[test]
fn the_tuner_uses_a_named_holdout_whole() {
    let dir = std::env::temp_dir().join(format!("cadence-datagen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (train, held, stats) = extract(&run(4, 10, 2), 5);
    let (t, h) = (dir.join("train.txt"), dir.join("holdout.txt"));
    std::fs::write(&t, train).expect("writes");
    std::fs::write(&h, held).expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .args(["texel", t.to_str().expect("path"), "--holdout-file"])
        .arg(&h)
        .args(["--iterations", "1", "--report", "1"])
        .output()
        .expect("runs");
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    let want = format!("positions train {} holdout {}", stats.train, stats.holdout);
    assert!(stdout.lines().any(|l| l == want), "{stdout}");
}

#[test]
fn the_window_keeps_plies_eight_to_two_hundred_and_forty_inclusive() {
    // Knights shuffling from the standard start: every position is quiet and none is in check.
    let shuffle = ["g1f3", "g8f6", "f3g1", "f6g8"];
    let moves: Vec<String> = (0..300).map(|i| format!("{}:0", shuffle[i % 4])).collect();
    let plies: Vec<&str> = (0..8).map(|i| shuffle[i % 4]).collect();
    let line = format!(
        "0 | 518 518 | {} | {} | 1/2-1/2 repetition | 0\n",
        plies.join(" "),
        moves.join(" ")
    );
    let (train, _, stats) = extract(line.as_bytes(), 0);
    let kept = extract::LAST_PLY - extract::FIRST_PLY + 1;
    assert_eq!(positions(&train).len(), kept);
    assert_eq!(stats.outside, (300 - kept) as u64);
}
