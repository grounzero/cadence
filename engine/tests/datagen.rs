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

/// Each worker holds a stack and a table, so a huge count starts none for no games and one for one.
#[test]
fn a_huge_thread_count_starts_no_more_workers_than_games() {
    let empty = format!("{}\n", datagen::provenance(7, 0));
    assert_eq!(run(7, 0, 100_000), empty.as_bytes());
    assert_eq!(run(7, 1, 100_000), run(7, 1, 1));

    let out = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .args(["datagen", "play", "--games", "0", "--seed", "7"])
        .args(["--threads", "100000"])
        .output()
        .expect("runs");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout, empty.as_bytes());
    let note = format!("is cut to {}", datagen::MAX_THREADS);
    let err = String::from_utf8(out.stderr).expect("utf-8");
    assert!(err.contains(&note), "{err}");
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
        "0 | 518 518 | a\u{e9}1 |  | 1-0 mate | 0",
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
    assert_eq!(positions(&none), [] as [&str; 0]);
    let (none_train, all_held, _) = extract(&file, 1);
    assert_eq!(positions(&none_train), [] as [&str; 0]);
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

/// A record file cut short still names the whole run in its provenance, so the count shows it.
#[test]
fn extraction_refuses_a_file_holding_fewer_games_than_it_names() {
    let file = run(5, 3, 1);
    let text = std::str::from_utf8(&file).expect("utf-8");
    let cut = text.lines().take(3).collect::<Vec<_>>().join("\n");
    let (mut train, mut holdout) = (Vec::new(), Vec::new());
    let refused = extract::extract(cut.as_bytes(), &mut train, &mut holdout, 0);
    assert!(
        refused.is_err(),
        "two of three games were extracted as whole"
    );
    let joined = [&file[..], &file[..]].concat();
    let (mut train, mut holdout) = (Vec::new(), Vec::new());
    let stats = extract::extract(&joined[..], &mut train, &mut holdout, 0).expect("two runs");
    assert_eq!(stats.games, 6);
}

/// A stage that fails leaves no output for the next one to read as whole.
#[test]
fn a_failed_extraction_leaves_no_output_behind() {
    let dir = std::env::temp_dir().join(format!("cadence-datagen-cut-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let games = dir.join("games.txt");
    let mut text = String::from_utf8(run(6, 2, 1)).expect("utf-8");
    text.push_str("0 | 518 518 | e2e5 |  | 1-0 mate | 0\n");
    std::fs::write(&games, text).expect("writes");
    let out = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .args([
            "datagen",
            "extract",
            games.to_str().expect("path"),
            "--train",
        ])
        .arg(dir.join("train.txt"))
        .arg("--holdout")
        .arg(dir.join("holdout.txt"))
        .output()
        .expect("runs");
    let mut left: Vec<_> = std::fs::read_dir(&dir)
        .expect("lists")
        .map(|e| e.expect("an entry").file_name())
        .collect();
    left.sort();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success(), "a malformed record was extracted");
    assert_eq!(left, ["games.txt"], "left behind");
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

/// A provenance line must parse to its end, or a header cut short would skip the count it carries;
/// another comment is still only a comment.
#[test]
fn extraction_refuses_a_provenance_line_cut_short() {
    let text = String::from_utf8(run(7, 2, 1)).expect("utf-8");
    let records: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
    let whole = datagen::provenance(7, 2);
    let games = whole.find(" games ").expect("a game count");
    let extract_with = |header: &str| {
        let file = format!("{header}\n{}\n", records.join("\n"));
        let (mut train, mut holdout) = (Vec::new(), Vec::new());
        extract::extract(file.as_bytes(), &mut train, &mut holdout, 0)
    };
    extract_with(&whole).expect("a whole header");
    extract_with(&format!("# a note\n{whole}")).expect("another comment beside it");
    for cut in [
        whole[..games].to_string(),
        whole[..games + " games".len()].to_string(),
        whole.replacen(" games 2 ", " games two ", 1),
        "# cadence datagen".to_string(),
    ] {
        assert!(extract_with(&cut).is_err(), "{cut}");
    }
}

// Extraction's paths
// ---------------------------------------------------------------------------

/// A fresh directory holding `games.txt`, a short run, and the run's bytes.
fn scratch(name: &str) -> (std::path::PathBuf, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("cadence-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let games = run(9, 3, 1);
    std::fs::write(dir.join("games.txt"), &games).expect("writes");
    (dir, games)
}

/// Run in `dir`, so each path is the spelling given: whether it succeeded, and every entry left in
/// `dir` with its bytes, a directory's name ending in a slash.
fn extract_in(dir: &std::path::Path, args: &str) -> (bool, Vec<(String, Vec<u8>)>) {
    let status = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .current_dir(dir)
        .args(["datagen", "extract"])
        .args(args.split_whitespace())
        .output()
        .expect("runs")
        .status;
    let mut left: Vec<_> = std::fs::read_dir(dir)
        .expect("lists")
        .map(|e| {
            let entry = e.expect("an entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() {
                (format!("{name}/"), Vec::new())
            } else {
                (name, std::fs::read(entry.path()).expect("reads"))
            }
        })
        .collect();
    left.sort();
    let _ = std::fs::remove_dir_all(dir);
    (status.success(), left)
}

fn names(left: &[(String, Vec<u8>)]) -> Vec<&str> {
    left.iter().map(|(entry, _)| entry.as_str()).collect()
}

/// The stage beside an output was once the output's name and `.partial`, here the input's own
/// name, so the input was emptied before it was read.
#[test]
fn extraction_never_writes_over_its_input() {
    let (dir, games) = scratch("extract-stage");
    std::fs::rename(dir.join("games.txt"), dir.join("games.partial")).expect("renames");
    let (ok, left) = extract_in(&dir, "games.partial --train games --holdout held");
    assert!(ok, "{:?}", names(&left));
    assert_eq!(names(&left), ["games", "games.partial", "held"]);
    assert!(left[1].1 == games, "the input changed");
}

/// The same file under two spellings, or the input named as an output, is refused before anything
/// is created, and every file is left as it was.
#[test]
fn extraction_refuses_paths_that_name_one_file() {
    for (name, args) in [
        ("outputs", "games.txt --train out --holdout ./out"),
        ("input", "games.txt --train games.txt --holdout held"),
    ] {
        let (dir, games) = scratch(&format!("extract-{name}"));
        std::fs::write(dir.join("out"), b"before\n").expect("writes");
        let (ok, left) = extract_in(&dir, args);
        assert!(!ok, "{name}: extracted, leaving {:?}", names(&left));
        assert_eq!(names(&left), ["games.txt", "out"], "{name}");
        assert!(left[0].1 == games, "{name}: the input changed");
        assert!(left[1].1 == b"before\n", "{name}: an output changed");
    }
}

/// A file system that ignores case reads two names differing only in case as one file.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn extraction_refuses_outputs_differing_only_in_case() {
    let (dir, games) = scratch("extract-case");
    let (ok, left) = extract_in(&dir, "games.txt --train out --holdout OUT");
    assert!(!ok, "extracted, leaving {:?}", names(&left));
    assert_eq!(names(&left), ["games.txt"]);
    assert!(left[0].1 == games, "the input changed");
}

/// A destination that is a directory is found before either output is published, and no stage is
/// left behind, on every platform's rename.
#[test]
fn extraction_publishes_neither_output_when_one_cannot_be() {
    let (dir, games) = scratch("extract-dir");
    std::fs::write(dir.join("train.txt"), b"before\n").expect("writes");
    std::fs::create_dir(dir.join("held")).expect("a directory");
    let (ok, left) = extract_in(&dir, "games.txt --train train.txt --holdout held");
    assert!(!ok, "extracted, leaving {:?}", names(&left));
    assert_eq!(names(&left), ["games.txt", "held/", "train.txt"]);
    assert!(left[0].1 == games, "the input changed");
    assert!(left[2].1 == b"before\n", "the train output was published");
}
