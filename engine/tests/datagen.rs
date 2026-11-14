// SPDX-License-Identifier: GPL-3.0-or-later

//! A game ends by the rules alone, a game is a function of its seed and number, a run's file is the
//! same at any thread count, a record replays to the ending it names, and extraction splits by game
//! and keeps no position in check.

mod support;

use std::process::{Command, Stdio};

use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_core::{FenStyle, Move, PieceType, generate_legal, parse_uci};
use cadence_engine::datagen::game::{Ending, Outcome, ending};
use cadence_engine::datagen::opening::{self, RANDOM_PLIES, WINDOW};
use cadence_engine::datagen::record::Record;
use cadence_engine::datagen::{self, extract, game};
use cadence_engine::position::Position;
use cadence_engine::score;
use cadence_engine::tt::Table;
use cadence_engine::version::VERSION;

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

/// A run header for records built by hand rather than played.
fn handmade(games: u64) -> String {
    datagen::provenance(0, games).replacen(VERSION, "0.0.0-handmade", 1)
}

fn listed(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("lists")
        .map(|e| {
            e.expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
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

    let out = Command::new(support::binary())
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
        let score = score.expect("a probe at the screen's count finishes depth 1");
        assert!(score.abs() <= WINDOW, "screened {score}");
    }
}

/// A search that cannot finish depth 1 leaves no score, never a 0 that reads as a balanced position.
#[test]
fn a_move_whose_search_did_not_finish_carries_no_score() {
    let start = || Position::new(board(cadence_core::START_FEN));
    let starved = game::play(&mut start(), 1, &table());
    assert_ne!(starved.moves, Vec::new(), "no move was played");
    let scored: Vec<_> = starved.moves.iter().filter(|(_, s)| s.is_some()).collect();
    assert_eq!(
        scored,
        [] as [&(Move, Option<i32>); 0],
        "scored at one node"
    );
    let fed = game::play(&mut start(), datagen::NODES, &table());
    assert!(fed.moves[0].1.is_some(), "no score at the full count");
}

// Records
// ---------------------------------------------------------------------------

/// A move with no score is spelled `none`, and reads back as itself.
#[test]
fn a_move_with_no_score_is_spelled_none_and_reads_back() {
    let mut record = datagen::play(1, 0, &table());
    record.moves[0].1 = None;
    let line = record.line();
    let spelled = format!("{}:none", record.moves[0].0.to_uci_chess960());
    assert!(line.contains(&spelled), "{line}");
    assert_eq!(Record::parse(&line).expect("parses"), record);
}

/// Extraction keeps no position whose move has no score, and counts it.
#[test]
fn extraction_keeps_no_position_for_a_move_with_no_score() {
    let mut record = datagen::play(1, 0, &table());
    for (_, s) in &mut record.moves {
        *s = None;
    }
    let file = format!("{}\n{}\n", handmade(1), record.line());
    let (train, _, stats) = extract(file.as_bytes(), 0);
    assert_eq!(positions(&train), [] as [&str; 0]);
    assert!(stats.no_score > 0, "no move counted");
    let total = stats.outside + stats.in_check + stats.noisy + stats.no_score + stats.mate;
    assert_eq!(total, stats.visited);
}

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
        if r.number % 4 == 0 {
            want_held.extend(kept(&r));
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

/// What extraction keeps of a game, worked out here from the record alone.
fn kept(record: &Record) -> Vec<String> {
    let mut b = record.start();
    for &m in &record.plies {
        b.play(m);
    }
    let mut out = Vec::new();
    for (ply, &(m, s)) in record.moves.iter().enumerate() {
        let window = (extract::FIRST_PLY..=extract::LAST_PLY).contains(&ply);
        let scored = s.is_some_and(|s| !score::is_mate(s));
        if window && !b.in_check() && !m.is_noisy() && scored {
            let fen = b.to_fen(FenStyle::Shredder);
            out.push(format!("{fen} | {}", record.outcome.text()));
        }
        b.play(m);
    }
    out
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
        stats.outside + stats.in_check + stats.noisy + stats.no_score + stats.mate + kept,
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

/// A record file cut short still names the whole run in its provenance, so the count shows it, and a
/// run joined to itself names its seed twice.
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
    let refused = extract::extract(&joined[..], &mut train, &mut holdout, 0);
    assert!(refused.is_err(), "a run joined to itself was extracted");
}

/// The count passes a copy of one game standing in for another, and a short run beside a long one.
#[test]
fn extraction_refuses_games_out_of_turn_in_their_run() {
    let (a, b) = (run(5, 3, 1), run(5, 4, 1));
    let three: Vec<&str> = std::str::from_utf8(&a).expect("utf-8").lines().collect();
    let four: Vec<&str> = std::str::from_utf8(&b).expect("utf-8").lines().collect();
    let duplicated = [three[0], three[1], three[1], three[3]].join("\n");
    // A game is seeded by its number, so the four-game run's first three are the three-game run's.
    let unbalanced = [
        three[0], three[1], three[2], three[0], four[1], four[2], four[3], four[4],
    ];
    for (forged, what) in [
        (duplicated, "game 0 twice and game 1 not at all"),
        (unbalanced.join("\n"), "a run of two beside a run of four"),
    ] {
        let (mut train, mut holdout) = (Vec::new(), Vec::new());
        let refused = extract::extract(forged.as_bytes(), &mut train, &mut holdout, 0);
        assert!(refused.is_err(), "{what} was extracted");
    }
}

/// A record's result labels every position it gives, so moves that end otherwise, or go on after
/// the end, are refused; a move after mate was already illegal.
#[test]
fn extraction_refuses_a_false_result_and_moves_after_the_end() {
    let mate = "f2f3:0 e7e5:0 g2g4:0 d8h4:0";
    let shuffle = "g1f3:0 g8f6:0 f3g1:0 f6g8:0";
    let repeated = format!("{shuffle} {shuffle}");
    let record = |moves: &str, result: &str| {
        format!("{}\n0 | 518 518 |  | {moves} | {result} | 0", handmade(1))
    };
    let extracts = |line: &str| {
        let (mut train, mut holdout) = (Vec::new(), Vec::new());
        extract::extract(line.as_bytes(), &mut train, &mut holdout, 0).is_ok()
    };
    for (line, what) in [
        (record("e2e4:12", "1-0 mate"), "a mate after one move"),
        (record(mate, "1-0 mate"), "Black's mate as White's"),
        (
            record(&format!("{mate} e1f2:0"), "0-1 mate"),
            "a move after mate",
        ),
        (
            record(&format!("{repeated} g1f3:0"), "1/2-1/2 repetition"),
            "a move after the third occurrence",
        ),
    ] {
        assert!(!extracts(&line), "{what} was extracted");
    }
    assert!(extracts(&record(mate, "0-1 mate")));
    assert!(extracts(&record(&repeated, "1/2-1/2 repetition")));
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
    let out = Command::new(support::binary())
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
    let out = Command::new(support::binary())
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

/// From the start, each ply the first in generation order to reach a position not seen before, a
/// pawn moving only near the fifty-move limit: up to `quiet` plies none captures or checks.
/// After that no pawn moves, so a rule ends the game within a hundred plies.
fn walked_game(quiet: usize) -> (Vec<Move>, Ending, Outcome) {
    let mut b = Position::new(board(cadence_core::START_FEN));
    let mut seen = vec![b.key()];
    let mut played = Vec::new();
    loop {
        if let Some((end, outcome)) = ending(&b) {
            return (played, end, outcome);
        }
        let in_window = played.len() < quiet;
        let legal = generate_legal(&b);
        let (mut pawn, mut piece, mut any) = (None, None, None);
        for m in legal.iter().filter(|m| !m.is_noisy()) {
            let is_pawn = b.by_type(PieceType::Pawn).contains(m.from_sq());
            b.make_move(m);
            let fresh = !seen.contains(&b.key());
            let clean = !b.in_check() && ending(&b).is_none();
            b.unmake_move(m);
            let first = if is_pawn { &mut pawn } else { &mut piece };
            if fresh && (clean || !in_window) && first.is_none() {
                *first = Some(m);
            }
            if !is_pawn && any.is_none() {
                any = Some(m);
            }
        }
        let next = if in_window && b.halfmove_clock() >= 90 {
            pawn.or(piece)
        } else if in_window {
            piece.or(pawn)
        } else {
            piece.or(any).or_else(|| legal.iter().next())
        };
        let m = next.unwrap_or_else(|| panic!("no move fits at ply {}", played.len()));
        b.play(m);
        seen.push(b.key());
        played.push(m);
    }
}

#[test]
fn the_window_keeps_plies_eight_to_two_hundred_and_forty_inclusive() {
    let (played, end, outcome) = walked_game(8 + extract::LAST_PLY + 2);
    let (plies, moves) = played.split_at(8);
    assert!(
        moves.len() > extract::LAST_PLY + 1,
        "the game ended inside the window"
    );
    let plies: Vec<String> = plies.iter().map(|m| m.to_uci_chess960()).collect();
    let moves: Vec<String> = moves
        .iter()
        .map(|m| format!("{}:0", m.to_uci_chess960()))
        .collect();
    let line = format!(
        "{}\n0 | 518 518 | {} | {} | {} {} | 0\n",
        handmade(1),
        plies.join(" "),
        moves.join(" "),
        outcome.text(),
        end.word()
    );
    let (train, _, stats) = extract(line.as_bytes(), 0);
    let kept = extract::LAST_PLY - extract::FIRST_PLY + 1;
    assert_eq!(positions(&train).len(), kept);
    assert_eq!(stats.outside, (moves.len() - kept) as u64);
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

fn extract_error(file: &str) -> String {
    let (mut train, mut holdout) = (Vec::new(), Vec::new());
    match extract::extract(file.as_bytes(), &mut train, &mut holdout, 0) {
        Ok(_) => panic!("extracted:\n{file}"),
        Err(e) => e,
    }
}

/// Every record stands in a run, and a run holds every game it names by the next header and by the
/// end; a comment that is not a header passes anywhere.
#[test]
fn extraction_refuses_a_record_outside_a_whole_run() {
    let text = String::from_utf8(run(3, 2, 1)).expect("utf-8");
    let [header, first, second] = text.lines().collect::<Vec<_>>()[..] else {
        panic!("a header and two records: {text}");
    };
    let before = extract_error(&[first, header, first].join("\n"));
    let want = "line 1: a record before any provenance line";
    assert!(before.starts_with(want), "{before}");
    let short = extract_error(&[header, first, header, first, second].join("\n"));
    let want = "line 3: the run at line 1 names 2 games and holds 1";
    assert!(short.starts_with(want), "{short}");
    let end = extract_error(&[header, first].join("\n"));
    assert!(
        end.contains("the run at line 1 names 2 games and holds 1"),
        "{end}"
    );
    let noted = ["# a note", header, first, second].join("\n");
    let (mut train, mut holdout) = (Vec::new(), Vec::new());
    extract::extract(noted.as_bytes(), &mut train, &mut holdout, 0).expect("a note first");
}

// Duplicates and provenance
// ---------------------------------------------------------------------------

/// A seed named again repeats its run's games, so the second run is refused at its header; runs of
/// different seeds pass with their games summed.
#[test]
fn extraction_refuses_a_seed_named_twice() {
    let file = run(5, 5, 1);
    let joined = [&file[..], &file[..]].concat();
    let error = extract_error(std::str::from_utf8(&joined).expect("utf-8"));
    let want = "line 7: seed 5 is the seed of the run at line 1, so its games repeat";
    assert!(error.starts_with(want), "{error}");
    let other = [&file[..], &run(6, 2, 1)[..]].concat();
    let (_, _, stats) = extract(&other, 0);
    assert_eq!(stats.games, 7);
}

/// A game copied into another run, that run's provenance untouched, is found by its content.
#[test]
fn extraction_refuses_the_same_game_twice() {
    let (a, b) = (run(5, 3, 1), run(6, 3, 1));
    let a: Vec<&str> = std::str::from_utf8(&a).expect("utf-8").lines().collect();
    let mut b: Vec<&str> = std::str::from_utf8(&b).expect("utf-8").lines().collect();
    b[2] = a[2];
    let error = extract_error(&[a, b].concat().join("\n"));
    assert!(
        error.starts_with("line 7: the same game as line 3"),
        "{error}"
    );
}

/// A provenance line is read field by field and held to this build's constants, and game 0 of a run
/// this build wrote must replay from the seed the line names.
#[test]
fn extraction_holds_a_provenance_line_to_this_build() {
    let text = String::from_utf8(run(5, 2, 1)).expect("utf-8");
    let (header, records) = text.split_once('\n').expect("a header");
    let with = |h: &str| format!("{h}\n{records}");
    let seed = extract_error(&with(&header.replacen(" seed 5 ", " seed 99 ", 1)));
    assert!(
        seed.contains("game 0 does not replay from seed 99"),
        "{seed}"
    );
    let edit = |name: &str| {
        let mut words: Vec<&str> = header.split(' ').collect();
        let at = words.iter().position(|&w| w == name).expect("a field") + 1;
        words[at] = "4000";
        words.join(" ")
    };
    for name in ["nodes", "hash", "plies", "window", "screen"] {
        let error = extract_error(&with(&edit(name)));
        let want = format!("line 1: the provenance names {name} 4000 and this build's is ");
        assert!(error.starts_with(&want), "{error}");
    }
    for bad in [
        header.replacen(" nodes ", " node ", 1),
        header.replacen(" hash 16", "", 1),
        header.replacen(" games 2", " games 2 games 2", 1),
        header.replacen("hash 16 plies 8", "plies 8 hash 16", 1),
        format!("{header} extra 1"),
    ] {
        let error = extract_error(&with(&bad));
        assert!(error.starts_with("line 1: "), "{bad}: {error}");
    }
}

/// The input's SHA-256 is in the report and closes both outputs.
#[test]
fn extraction_records_its_input_digest() {
    let file = run(4, 3, 1);
    let digest = cadence_dataset::sha256::hex(&cadence_dataset::sha256::sha256(&file));
    let (train, held, stats) = extract(&file, 2);
    let line = format!("# cadence extract input-sha256 {digest}");
    assert_eq!(train.lines().last(), Some(line.as_str()));
    assert_eq!(held.lines().last(), Some(line.as_str()));
    let mut report = Vec::new();
    stats.report(&mut report).expect("reports");
    let report = String::from_utf8(report).expect("utf-8");
    let want = format!("input-sha256 {digest}");
    assert!(report.lines().any(|l| l == want), "{report}");
}

/// A run from another build passes with its seed claim named as not examined, and every extraction
/// says duplicates in files it was not given were not.
#[test]
fn extraction_says_what_it_did_not_examine() {
    let (dir, _) = scratch("extract-unexamined");
    let games = String::from_utf8(run(9, 3, 1)).expect("utf-8");
    let foreign = games.replacen(VERSION, "0.0.0-elsewhere", 1);
    std::fs::write(dir.join("games.txt"), foreign).expect("writes");
    let out = Command::new(support::binary())
        .current_dir(&dir)
        .args([
            "datagen",
            "extract",
            "games.txt",
            "--train",
            "t",
            "--holdout",
            "h",
        ])
        .output()
        .expect("runs");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).expect("utf-8");
    for want in [
        "  not examined: duplicates in files not given",
        "  not examined: the seed claims of runs from another build, 0.0.0-elsewhere",
    ] {
        assert!(stdout.contains(want), "{stdout}");
    }
}

/// A run without a seed would repeat every other run's games, so it is refused and writes nothing.
#[test]
fn play_refuses_to_run_without_a_seed() {
    let dir = std::env::temp_dir().join(format!("cadence-datagen-seed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let play = |seed: &[&str]| {
        Command::new(support::binary())
            .args(["datagen", "play", "--games", "2", "--out"])
            .arg(dir.join("games.txt"))
            .args(seed)
            .output()
            .expect("runs")
    };
    let refused = play(&[]);
    let before = listed(&dir);
    let ran = play(&["--seed", "7"]);
    let after = listed(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(err.contains("--seed is required"), "{err}");
    assert_eq!(before, [] as [&str; 0], "written without a seed");
    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(after, ["games.txt"]);
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
    let status = Command::new(support::binary())
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

/// The input carries the name a stage beside an output could take, the output's name and
/// `.partial`, and extraction must still read it whole rather than write over it.
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

/// The report is written before the outputs are published, so an extraction whose report has
/// nowhere to go publishes nothing.
#[test]
fn extraction_publishes_nothing_when_its_report_cannot_be_written() {
    let (dir, _) = scratch("extract-closed");
    let mut child = Command::new(support::binary())
        .current_dir(&dir)
        .args(["datagen", "extract", "games.txt", "--train", "train.txt"])
        .args(["--holdout", "held.txt"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("runs");
    drop(child.stdout.take());
    let status = child.wait().expect("waits");
    let left = listed(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !status.success(),
        "a report nobody read was taken as written"
    );
    assert_eq!(left, ["games.txt"], "published without its report");

    let (dir, _) = scratch("extract-open");
    let (ok, left) = extract_in(&dir, "games.txt --train train.txt --holdout held.txt");
    assert!(ok, "{:?}", names(&left));
    assert_eq!(names(&left), ["games.txt", "held.txt", "train.txt"]);
}

/// The usage answers a command line that does not parse, not work that failed.
#[test]
fn the_usage_is_printed_only_for_an_argument_error() {
    let (dir, _) = scratch("extract-usage");
    std::fs::write(
        dir.join("bad.txt"),
        "0 | 518 518 | e2e5 |  | 1-0 mate | 0\n",
    )
    .expect("writes");
    let stderr = |args: &[&str]| {
        let out = Command::new(support::binary())
            .current_dir(&dir)
            .arg("datagen")
            .args(args)
            .output()
            .expect("runs");
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        String::from_utf8(out.stderr).expect("utf-8")
    };
    let argument = stderr(&["extract", "bad.txt", "--train", "t"]);
    let work = stderr(&["extract", "bad.txt", "--train", "t", "--holdout", "h"]);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(argument.contains("usage:"), "{argument}");
    assert!(!work.contains("usage:"), "{work}");
}
