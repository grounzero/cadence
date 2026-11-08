// SPDX-License-Identifier: GPL-3.0-or-later

//! The ignored tests read the tables `tests/fixtures/syzygy.sha256` names from the directory
//! `CADENCE_SYZYGY_PATH` gives, which CI fetches by checksum. The rest need no table.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{MAX_PLY, Move, generate_legal, parse_uci};
use cadence_engine::corrhist::MAX_CORRECTION;
use cadence_engine::position::Position;
use cadence_engine::score::{
    MAX_EVAL, Score, TB_WIN, TB_WIN_IN_MAX_PLY, from_tt, is_decisive, is_mate, mate_in, mated_in,
    tb_loss_in, tb_win_in, to_tt,
};
use cadence_engine::search::{Limits, futile_node, probcut_bound};
use cadence_engine::syzygy::{Rank, Ranked, Tables, Wdl};
use cadence_engine::tt::Bound;
use shakmaty_syzygy::{AmbiguousWdl, Dtz, MaybeRounded};
use support::{Engine, table};

fn fixtures() -> String {
    std::env::var("CADENCE_SYZYGY_PATH").expect("CADENCE_SYZYGY_PATH names the fixtures")
}

fn loaded(dir: &str) -> Tables {
    let (tables, problems) = Tables::load(dir, 7);
    assert!(problems.is_empty(), "{dir}: {problems:?}");
    tables
}

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// A fresh directory holding the named fixtures, so a gate can leave a table out or spoil one.
fn scratch(name: &str, files: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadence-syzygy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for file in files {
        std::fs::copy(PathBuf::from(fixtures()).join(file), dir.join(file)).unwrap();
    }
    dir
}

/// The fixture set's six WDL files and none of its distances.
fn wdl_only(name: &str) -> PathBuf {
    scratch(
        name,
        &[
            "KBvK.rtbw",
            "KNvK.rtbw",
            "KPvK.rtbw",
            "KQvK.rtbw",
            "KRvK.rtbw",
            "KRvKB.rtbw",
        ],
    )
}

/// The three- and four-piece tables `tests/fixtures/syzygy.sha256` names.
fn fixture_files() -> Vec<&'static str> {
    include_str!("../../tests/fixtures/syzygy.sha256")
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        // A name's letters before the dot are its pieces and a `v`.
        .filter(|f| f.find('.').is_some_and(|dot| dot <= 5))
        .collect()
}

/// Only those tables, for a gate that counts what loads and so must not see a fuller set installed
/// beside them.
fn fixture_set(name: &str) -> PathBuf {
    scratch(name, &fixture_files())
}

/// Mate in one, the side to move's loss, a capture into a won ending, a pawn about to queen, and
/// three draws: each outcome is the game's and not the table's, so a table read wrongly fails here.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn named_positions_probe_to_their_known_outcome() {
    let tables = loaded(&fixtures());
    let cases: [(&str, Wdl, Option<i32>); 7] = [
        ("k7/8/1K6/8/8/8/7Q/8 w - - 0 1", Wdl::Win, Some(1)),
        ("k7/8/2K5/8/8/8/7Q/8 b - - 0 1", Wdl::Loss, None),
        ("8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1", Wdl::Win, Some(1)),
        ("8/P7/8/8/8/8/8/K6k w - - 0 1", Wdl::Win, Some(1)),
        ("8/8/8/8/8/8/2B5/K1k5 w - - 0 1", Wdl::Draw, Some(0)),
        ("k7/8/8/8/8/8/P7/K7 w - - 0 1", Wdl::Draw, Some(0)),
        ("4k3/8/8/8/8/8/8/1N2K3 w - - 0 1", Wdl::Draw, Some(0)),
    ];
    for (fen, wdl, dtz) in cases {
        let b = board(fen);
        assert_eq!(tables.probe_wdl(&b), Some(wdl), "{fen}");
        let found = tables
            .probe_dtz(&b)
            .unwrap_or_else(|| panic!("{fen}: no distance"));
        match dtz {
            Some(plies) => assert_eq!(found, plies, "{fen}"),
            None => assert!(found < 0, "{fen}: {found}"),
        }
    }
    assert_eq!(tables.failures(), 0);
}

/// No move here mates or zeroes, so at a counter of 99 every win runs past the hundredth half-move:
/// a cursed win, still ranked above every draw.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_win_the_fifty_move_counter_forbids_ranks_as_no_win() {
    let tables = loaded(&fixtures());
    let mut fresh = board("8/8/8/3k4/8/8/7Q/K7 w - - 0 1");
    let ranked = tables.rank_root(&mut fresh, &[]).unwrap();
    assert_eq!(ranked.best, Rank::Win);
    assert!(ranked.decided);
    let mut late = board("8/8/8/3k4/8/8/7Q/K7 w - - 99 80");
    let ranked = tables.rank_root(&mut late, &[]).unwrap();
    assert_eq!(ranked.best, Rank::CursedWin);
    // A queen given away is a draw, which a cursed win still outranks.
    let hanging = parse_uci(&generate_legal(&late), "h2d6").unwrap();
    assert!(!ranked.moves.contains(&hanging));
}

/// The root keeps only its best rank, inside a `searchmoves` list where one is given.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn the_root_keeps_its_best_rank_within_the_list_it_is_given() {
    let tables = loaded(&fixtures());
    let mut b = board("8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1");
    let legal = generate_legal(&b);
    let rxb2 = parse_uci(&legal, "e2b2").unwrap();
    let kxb2 = parse_uci(&legal, "a1b2").unwrap();
    let kb1 = parse_uci(&legal, "a1b1").unwrap();
    let ranked = tables.rank_root(&mut b, &[]).unwrap();
    assert_eq!(ranked.best, Rank::Win);
    assert!(ranked.moves.contains(&rxb2) && ranked.moves.contains(&kxb2));
    assert!(!ranked.moves.contains(&kb1));
    let ranked = tables.rank_root(&mut b, &[kb1]).unwrap();
    assert_eq!(ranked.moves, vec![kb1]);
}

/// The board keeps an en passant square after every double push, takers or not, and the crate is
/// handed it as it is, so a push with no capturer must still be ranked and not refused.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_double_push_with_no_capturer_is_ranked() {
    let tables = loaded(&fixtures());
    let mut b = board("k7/8/8/8/8/8/P7/K7 w - - 0 1");
    let a2a4 = parse_uci(&generate_legal(&b), "a2a4").unwrap();
    assert!(
        tables.rank_root(&mut b, &[]).is_some(),
        "the root is unranked"
    );
    let ranked = tables
        .rank_root(&mut b, &[a2a4])
        .expect("the push is ranked");
    assert_eq!(ranked.moves, vec![a2a4]);
}

/// With the ranking settled no node probes, and with distances missing every node after a zeroing
/// move does: either way the mate is found and reported as a mate, above the band.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_mate_outranks_a_tablebase_win() {
    let wdl_only = scratch(
        "wdl",
        &[
            "KBvK.rtbw",
            "KNvK.rtbw",
            "KPvK.rtbw",
            "KQvK.rtbw",
            "KRvK.rtbw",
            "KRvKB.rtbw",
        ],
    );
    let fen = "k7/8/1K6/8/8/8/7Q/8 w - - 0 1";
    for (dir, decided) in [(fixtures(), true), (wdl_only.display().to_string(), false)] {
        let tables = loaded(&dir);
        let mut b = board(fen);
        let ranked = tables.rank_root(&mut b, &[]).unwrap();
        assert_eq!(ranked.decided, decided, "{dir}");
        let stop = AtomicBool::new(false);
        let tt = table();
        let mut search = support::search(Limits::depth(4), &stop, &tt);
        search.set_root_filter(&ranked.moves);
        search.set_tables(&tables, ranked.decided);
        let best = search.run(&mut support::position(fen), &mut Vec::new());
        assert_eq!(Some(best), parse_uci(&generate_legal(&b), "h2h8"), "{dir}");
        assert_eq!(search.score(), mate_in(1), "{dir}");
    }
    let _ = std::fs::remove_dir_all(wdl_only);
}

/// Every capture of the bishop leaves a won rook ending, which the search reads from the table at
/// the node after the capture rather than searching out.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn the_search_probes_after_a_capture_and_scores_the_band() {
    let wdl_only = scratch(
        "probe",
        &["KBvK.rtbw", "KRvK.rtbw", "KRvKB.rtbw", "KQvK.rtbw"],
    );
    let tables = loaded(&wdl_only.display().to_string());
    let fen = "8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1";
    let mut b = board(fen);
    let ranked = tables.rank_root(&mut b, &[]).unwrap();
    assert!(!ranked.decided);
    tables.reset_hits();
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut search = support::search(Limits::depth(5), &stop, &tt);
    search.set_root_filter(&ranked.moves);
    search.set_tables(&tables, ranked.decided);
    search.run(&mut support::position(fen), &mut Vec::new());
    assert!(tables.hits() > 0, "no probe answered");
    let score = search.score();
    assert!(score > 0 && is_decisive(score), "{score}");
    let _ = std::fs::remove_dir_all(wdl_only);
}

/// A table's cutoff keeps the move the transposition table held for the node, so a later visit
/// still tries it first: a drawn ending cuts exactly, and a won one cuts above beta.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_table_cutoff_keeps_the_move_the_transposition_table_held() {
    let only = fixture_set("tt-move");
    let tables = loaded(&only.display().to_string());
    for (fen, mv) in [
        ("8/8/8/4k3/8/8/2B5/4K3 w - - 0 1", "c2h7"),
        ("8/8/8/3k4/8/8/8/R3K3 w - - 0 1", "a1a4"),
    ] {
        let b = board(fen);
        let m = parse_uci(&generate_legal(&b), mv).unwrap();
        let tt = table();
        tt.store(b.key(), m, 0, 0, Bound::Upper, false);
        let stop = AtomicBool::new(false);
        let mut search = support::search(Limits::depth(5), &stop, &tt);
        search.set_tables(&tables, false);
        let _ = search.node_window(&mut support::position(fen), 5, 1, 99, 100);
        let hit = tt.probe(b.key()).expect("the node stored its entry");
        assert_eq!((hit.mv, hit.depth), (m, 5), "{fen}");
    }
    let _ = std::fs::remove_dir_all(only);
}

/// A castling right, too many pieces, a missing table and a spoilt one are each no result, and
/// none panics.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn outside_the_tables_there_is_no_result_and_no_panic() {
    let only = fixture_set("cardinality");
    let tables = loaded(&only.display().to_string());
    assert_eq!(tables.cardinality(), 4);
    let mut castling = board("4k3/8/8/8/8/8/8/R3K3 w Q - 0 1");
    assert!(!tables.covers(&castling));
    assert!(tables.rank_root(&mut castling, &[]).is_none());
    let mut crowded = board("4k3/8/8/8/8/8/PP6/R3K3 w - - 0 1");
    assert!(!tables.covers(&crowded));
    assert!(tables.rank_root(&mut crowded, &[]).is_none());
    let missing = board("3rk3/8/8/8/8/8/8/3QK3 w - - 0 1");
    assert!(tables.covers(&missing));
    assert_eq!(tables.probe_wdl(&missing), None);
    assert_eq!(tables.failures(), 1);

    let spoilt = scratch("spoilt", &["KQvK.rtbw", "KQvK.rtbz"]);
    let path = spoilt.join("KQvK.rtbw");
    let mut bytes = std::fs::read(&path).unwrap();
    for b in &mut bytes[..4] {
        *b ^= 0xff;
    }
    std::fs::write(&path, bytes).unwrap();
    let tables = loaded(&spoilt.display().to_string());
    assert_eq!(
        tables.probe_wdl(&board("k7/8/1K6/8/8/8/7Q/8 w - - 0 1")),
        None
    );
    assert_eq!(tables.failures(), 1);
    let _ = std::fs::remove_dir_all(spoilt);
    let _ = std::fs::remove_dir_all(only);
}

/// Two directories in one path read as one set, split at the platform's separator, `:` or `;` on
/// Windows, and the other separator is part of a name.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn two_directories_split_at_the_platform_s_separator_read_as_one_set() {
    let wdl = wdl_only("dirs-wdl");
    let dtz = scratch(
        "dirs-dtz",
        &[
            "KBvK.rtbz",
            "KNvK.rtbz",
            "KPvK.rtbz",
            "KQvK.rtbz",
            "KRvK.rtbz",
            "KRvKB.rtbz",
        ],
    );
    let (separator, other) = if cfg!(windows) {
        (';', ':')
    } else {
        (':', ';')
    };
    let tables = loaded(&format!("{}{separator}{}", wdl.display(), dtz.display()));
    assert_eq!(tables.files(), 12);
    let mut won = board("8/8/8/3k4/8/8/8/R3K3 w - - 0 1");
    assert!(tables.rank_root(&mut won, &[]).unwrap().decided);
    let (tables, problems) = Tables::load(&format!("{}{other}{}", wdl.display(), dtz.display()), 7);
    assert_eq!(tables.files(), 0);
    assert_eq!(problems.len(), 1, "{problems:?}");
    let _ = std::fs::remove_dir_all(wdl);
    let _ = std::fs::remove_dir_all(dtz);
}

/// A table that fails to open is not opened again at the next probe, so it stays unread when it
/// reappears until the path is set again.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_table_that_failed_to_open_is_not_opened_again() {
    let dir = scratch("reopen", &["KQvK.rtbw", "KQvK.rtbz"]);
    let tables = loaded(&dir.display().to_string());
    let away = dir.join("KQvK.rtbw.away");
    std::fs::rename(dir.join("KQvK.rtbw"), &away).unwrap();
    let won = board("k7/8/1K6/8/8/8/7Q/8 w - - 0 1");
    assert_eq!(tables.probe_wdl(&won), None);
    std::fs::rename(&away, dir.join("KQvK.rtbw")).unwrap();
    assert_eq!(tables.probe_wdl(&won), None);
    assert_eq!(tables.failures(), 2);
    assert_eq!(
        loaded(&dir.display().to_string()).probe_wdl(&won),
        Some(Wdl::Win)
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A truncated file is named, and the rest of its directory loads past it.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_bad_file_is_named_and_the_rest_of_its_directory_loads() {
    let only = fixture_set("truncated");
    let path = only.join("KQvK.rtbz");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..1000]).unwrap();
    let (tables, problems) = Tables::load(&only.display().to_string(), 7);
    assert_eq!(
        problems,
        vec![format!("{}: unexpected file size", path.display())]
    );
    assert_eq!(tables.files(), fixture_files().len() - 1);
    assert_eq!(
        tables.probe_wdl(&board("k7/8/1K6/8/8/8/7Q/8 w - - 0 1")),
        Some(Wdl::Win)
    );
    let _ = std::fs::remove_dir_all(only);
}

/// A table gone after loading fails on whichever thread probes it first, inside the search, and is
/// named once in an `info string` however many probes fail on it.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_table_gone_after_loading_is_named_once_whichever_thread_probes_it() {
    let only = fixture_set("gone");
    let mut e = Engine::spawn();
    e.send(&format!(
        "setoption name SyzygyPath value {}",
        only.display()
    ));
    // The root's four pieces are past the limit, so only the search's probes reach the rook ending.
    e.send("setoption name SyzygyProbeLimit value 3");
    e.send("setoption name Threads value 2");
    e.sync();
    std::fs::remove_file(only.join("KRvK.rtbw")).unwrap();
    let fen = "8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1";
    e.send(&format!("position fen {fen}"));
    e.send("go depth 6");
    let lines = e.read_until("bestmove ");
    e.quit();
    let named: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("info string Syzygy: "))
        .collect();
    assert_eq!(named.len(), 1, "{lines:?}");
    assert!(named[0].contains("KRvK"), "{named:?}");
    let best = lines.last().and_then(|l| l.split_whitespace().nth(1));
    assert!(
        parse_uci(&generate_legal(&board(fen)), best.unwrap_or_default()).is_some(),
        "{lines:?}"
    );
    let _ = std::fs::remove_dir_all(only);
}

/// A path loads the tables and prints how many; an empty one turns probing off, and every line a
/// search prints without tables is the line it was.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_path_set_over_uci_probes_and_an_empty_one_stops() {
    let only = fixture_set("uci");
    let mut e = Engine::spawn();
    e.send(&format!(
        "setoption name SyzygyPath value {}",
        only.display()
    ));
    let said = e.sync();
    let files = fixture_files().len();
    let loaded = |pieces: u32| {
        format!("info string SyzygyPath: {files} table files, probing up to {pieces} pieces")
    };
    assert!(said.iter().any(|l| *l == loaded(4)), "{said:?}");
    e.send("setoption name SyzygyProbeLimit value 3");
    let said = e.sync();
    assert!(said.iter().any(|l| *l == loaded(3)), "{said:?}");
    e.send("position fen 8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1");
    e.send("go depth 4");
    let lines = e.read_until("bestmove ");
    assert!(lines.iter().any(|l| l.contains(" tbhits ")), "{lines:?}");
    e.send("setoption name SyzygyPath value <empty>");
    e.send("go depth 4");
    let lines = e.read_until("bestmove ");
    assert!(lines.iter().all(|l| !l.contains("tbhits")), "{lines:?}");
    e.quit();
    let _ = std::fs::remove_dir_all(only);
}

/// Every score the iterations print from `fen` at `depth`, with the tables in `dir`, and the move.
fn scores(dir: &Path, fen: &str, depth: u32) -> (Vec<String>, String) {
    scores_after(dir, &[], fen, &format!("go depth {depth}"))
}

/// The same after the options given, for the `go` line given.
fn scores_after(dir: &Path, options: &[&str], fen: &str, go: &str) -> (Vec<String>, String) {
    let mut e = Engine::spawn();
    e.send(&format!(
        "setoption name SyzygyPath value {}",
        dir.display()
    ));
    for option in options {
        e.send(option);
    }
    e.sync();
    e.send(&format!("position fen {fen}"));
    e.send(go);
    let lines = e.read_until("bestmove ");
    e.quit();
    let scores = lines
        .iter()
        .filter(|l| l.starts_with("info depth"))
        .map(|l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            let at = words.iter().position(|&w| w == "score").expect("a score");
            format!("{} {}", words[at + 1], words[at + 2])
        })
        .collect();
    let best = lines.last().and_then(|l| l.split_whitespace().nth(1));
    (scores, best.unwrap_or_default().to_string())
}

/// The score the last finished iteration prints from `fen` at `depth`, with the tables in `dir`.
fn reported_score(dir: &Path, fen: &str, depth: u32) -> String {
    let (scores, _) = scores(dir, fen, depth);
    scores
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("{fen}: no info line"))
}

fn centipawns(score: &str) -> Score {
    score
        .strip_prefix("cp ")
        .and_then(|cp| cp.parse().ok())
        .unwrap_or_else(|| panic!("not a centipawn score: {score}"))
}

/// A lone minor piece cannot mate, so the table reads a draw that the evaluation, a piece up, does
/// not; the root reports the table's 0.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_drawn_root_the_tables_rank_reports_a_draw() {
    let only = fixture_set("draw-report");
    for fen in [
        "8/8/8/4k3/8/8/2B5/4K3 w - - 0 1",
        "8/8/8/4k3/8/8/2N5/4K3 b - - 0 1",
    ] {
        assert_eq!(reported_score(&only, fen, 6), "cp 0", "{fen}");
    }
    let _ = std::fs::remove_dir_all(only);
}

/// A rook against a bare king is won and too long to mate out at depth 6, so the root reports the
/// table's win and the side being mated its loss, in the band and short of a mate.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_won_root_the_tables_rank_reports_the_table_s_win_and_a_lost_one_its_loss() {
    let only = fixture_set("win-report");
    let won = centipawns(&reported_score(&only, "8/8/8/3k4/8/8/8/R3K3 w - - 0 1", 6));
    assert!((TB_WIN_IN_MAX_PLY..=TB_WIN).contains(&won), "won: {won}");
    let lost = centipawns(&reported_score(&only, "8/8/8/3k4/8/8/8/R3K3 b - - 0 1", 6));
    assert!(
        (-TB_WIN..=-TB_WIN_IN_MAX_PLY).contains(&lost),
        "lost: {lost}"
    );
    let _ = std::fs::remove_dir_all(only);
}

/// A mate the search finds outranks the table's win, so a ranked root still reports it as a mate.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_mate_the_search_finds_at_a_ranked_root_is_reported_as_the_mate() {
    let only = fixture_set("mate-report");
    assert_eq!(
        reported_score(&only, "k7/8/1K6/8/8/8/7Q/8 w - - 0 1", 4),
        "mate 1"
    );
    let _ = std::fs::remove_dir_all(only);
}

/// At a counter of 99 no move here mates or zeroes, so every win runs past the hundredth half-move:
/// the WDL files alone must rank and report the draw the counter makes, for either side.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_wdl_only_root_at_the_counter_s_end_ranks_and_reports_the_draw() {
    let dir = wdl_only("counter-end");
    let tables = loaded(&dir.display().to_string());
    let fen = "8/8/8/3k4/8/8/7Q/K7 w - - 99 80";
    let mut b = board(fen);
    let ranked = tables.rank_root(&mut b, &[]).unwrap();
    assert!(!ranked.decided);
    assert_eq!(ranked.best, Rank::CursedWin);
    let hanging = parse_uci(&generate_legal(&b), "h2d6").unwrap();
    assert!(!ranked.moves.contains(&hanging));
    for fen in [fen, "8/8/8/3k4/8/8/7q/K7 w - - 99 80"] {
        let (scores, best) = scores(&dir, fen, 6);
        assert!(
            !scores.is_empty() && scores.iter().all(|s| s == "cp 0"),
            "{fen}: {scores:?}"
        );
        assert_ne!(best, "h2d6", "{fen}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// The fixtures' rook endings store rounded distances, so a move whose lands on the hundredth
/// half-move ranks above every move certain to be drawn or lost beyond it, for either side.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_rounded_distance_on_the_hundredth_half_move_outranks_the_certain_outcomes_beyond_it() {
    let only = fixture_set("rounded");
    let tables = loaded(&only.display().to_string());
    // Ra5 and Ke2 read 26 plies to zeroing and every other move 28.
    let won = "8/8/8/3k4/8/8/8/R3K3 w - - 73 80";
    let mut b = board(won);
    let ranked = tables.rank_root(&mut b, &[]).unwrap();
    assert_eq!(ranked.best, Rank::MaybeWin);
    let legal = generate_legal(&b);
    assert_eq!(ranked.moves.len(), 2, "{:?}", ranked.moves);
    for m in ["a1a5", "e1e2"] {
        assert!(ranked.moves.contains(&parse_uci(&legal, m).unwrap()), "{m}");
    }
    // Kb2 reads 17 plies to the rook side's zeroing and Ka2 11, so only Kb2 may hold.
    let lost = "8/8/8/8/8/8/8/K1rk4 w - - 82 80";
    let mut b = board(lost);
    let ranked = tables.rank_root(&mut b, &[]).unwrap();
    assert_eq!(ranked.best, Rank::MaybeLoss);
    assert_eq!(
        ranked.moves,
        vec![parse_uci(&generate_legal(&b), "a1b2").unwrap()]
    );
    for fen in [won, lost] {
        let score = reported_score(&only, fen, 6);
        let banded = score.starts_with("cp ")
            && (TB_WIN_IN_MAX_PLY..=TB_WIN).contains(&centipawns(&score).abs());
        assert!(!banded, "{fen}: {score}");
    }
    let _ = std::fs::remove_dir_all(only);
}

/// A won root keeps only its fastest wins, here the two moves that read 26 plies to zeroing where
/// every other win reads 28, and a lost root keeps every losing move.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_won_root_keeps_only_its_fastest_wins_and_a_lost_root_every_loss() {
    let tables = loaded(&fixtures());
    let mut won = board("8/8/8/3k4/8/8/8/R3K3 w - - 0 1");
    let ranked = tables.rank_root(&mut won, &[]).unwrap();
    assert_eq!(ranked.best, Rank::Win);
    assert!(ranked.decided);
    let legal = generate_legal(&won);
    assert_eq!(ranked.moves.len(), 2, "{:?}", ranked.moves);
    for m in ["a1a5", "e1e2"] {
        assert!(ranked.moves.contains(&parse_uci(&legal, m).unwrap()), "{m}");
    }
    let mut lost = board("8/8/8/3k4/8/8/8/R3K3 b - - 0 1");
    let ranked = tables.rank_root(&mut lost, &[]).unwrap();
    assert_eq!(ranked.best, Rank::Loss);
    assert_eq!(ranked.moves.len(), generate_legal(&lost).len());
}

/// The position `moves` reach from `fen`, its history kept so a repetition reads as one.
fn replay(fen: &str, moves: &[Move]) -> Board {
    let mut b = board(fen);
    for &m in moves {
        b.make_move(m);
    }
    b
}

/// The engine's move at a fixed depth among the moves the root keeps, as `go` gives them.
fn engine_move(tables: &Tables, ranked: &Ranked, fen: &str, moves: &[Move]) -> Move {
    let stop = AtomicBool::new(false);
    let tt = table();
    let mut search = support::search(Limits::depth(6), &stop, &tt);
    search.set_root_filter(&ranked.moves);
    search.set_tables(tables, ranked.decided);
    search.run(&mut Position::new(replay(fen, moves)), &mut Vec::new())
}

fn line(moves: &[Move]) -> String {
    moves
        .iter()
        .map(|m| m.to_uci_chess960())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Among the loser's best-ranked moves, the one that leaves the winner furthest from zeroing.
fn longest_resistance(tables: &Tables, game: &mut Board, moves: &[Move]) -> Move {
    let mut longest = None;
    for &m in moves {
        game.make_move(m);
        let plies = tables
            .probe_dtz(game)
            .expect("a distance after the loser's move");
        game.unmake_move(m);
        if longest.is_none_or(|(_, most)| plies > most) {
            longest = Some((m, plies));
        }
    }
    longest.expect("a move for the loser").0
}

/// Each won position is played out by the engine at a fixed depth against the tables' longest
/// resistance, and mated within the fifty-move rule. Each stretch between zeroing moves takes no
/// more plies than its distance at the start, and one more where a rounded distance wastes one.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_won_ending_is_converted_within_the_fifty_move_rule_making_progress_every_move() {
    let tables = loaded(&fixtures());
    for fen in [
        "8/8/8/3k4/8/8/7Q/K7 w - - 0 1",
        "8/8/8/3k4/8/8/7Q/K7 w - - 60 80",
        "8/8/8/3k4/8/8/8/R3K3 w - - 0 1",
        "4k3/8/4K3/4P3/8/8/8/8 w - - 0 1",
        "8/8/8/4k3/8/8/8/KBN5 w - - 0 1",
    ] {
        let mut moves: Vec<Move> = Vec::new();
        let mut stretch: Option<(i32, i32)> = None;
        loop {
            let mut game = replay(fen, &moves);
            if generate_legal(&game).is_empty() {
                assert!(game.in_check(), "{fen}: stalemate after {}", line(&moves));
                assert!(game.halfmove_clock() <= 100, "{fen}: {}", line(&moves));
                break;
            }
            assert!(moves.len() < 300, "{fen}: no mate in {}", line(&moves));
            let ranked = tables.rank_root(&mut game, &[]).unwrap();
            let winner = moves.len().is_multiple_of(2);
            let m = if winner {
                assert_eq!(ranked.best, Rank::Win, "{fen}: {}", line(&moves));
                assert!(ranked.decided, "{fen}");
                if stretch.is_none() {
                    stretch = Some((tables.probe_dtz(&game).unwrap(), 0));
                }
                engine_move(&tables, &ranked, fen, &moves)
            } else {
                longest_resistance(&tables, &mut game, &ranked.moves)
            };
            game.make_move(m);
            moves.push(m);
            if let Some((_, plies)) = stretch.as_mut() {
                *plies += 1;
            }
            let ended = game.halfmove_clock() == 0 || generate_legal(&game).is_empty();
            if winner && ended {
                let (distance, plies) = stretch.take().unwrap();
                assert!(
                    plies <= distance + 1,
                    "{fen}: {plies} plies from a distance of {distance} in {}",
                    line(&moves)
                );
            }
        }
    }
}

/// Part-way through the counter the WDL files cannot say whether a win outlasts it, so the root
/// reports the search's own score; at a zeroed counter, or through a zeroing move, the table's.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_wdl_only_root_reports_the_table_only_where_the_counter_leaves_it_exact() {
    let dir = wdl_only("counter-part");
    let won = centipawns(&reported_score(&dir, "8/8/8/3k4/8/8/8/R3K3 w - - 40 60", 6));
    assert!(won > 0 && won < TB_WIN_IN_MAX_PLY, "won: {won}");
    let lost = centipawns(&reported_score(&dir, "8/8/8/3k4/8/8/8/R3K3 b - - 40 60", 6));
    assert!(lost < 0 && lost > -TB_WIN_IN_MAX_PLY, "lost: {lost}");
    for fen in [
        "8/8/8/3k4/8/8/8/R3K3 w - - 0 1",
        "8/8/8/8/8/8/1b2R3/K5k1 w - - 40 60",
    ] {
        let score = centipawns(&reported_score(&dir, fen, 6));
        assert!(
            (TB_WIN_IN_MAX_PLY..=TB_WIN).contains(&score),
            "{fen}: {score}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// Every thread is given the primary's root filter and its probing, whether the tables rank the
/// root or the root is past the limit and only the search below it probes.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn every_search_thread_is_given_the_primary_s_filter_and_probing_with_tables() {
    let only = fixture_set("threads");
    let path = format!("setoption name SyzygyPath value {}", only.display());
    for (limit, probes) in [(7, false), (3, true)] {
        let limit = format!("setoption name SyzygyProbeLimit value {limit}");
        let (_, lines) = Engine::go_within(
            &[
                &path,
                &limit,
                "setoption name Threads value 3",
                "position fen 8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1",
            ],
            "go depth 4",
            std::time::Duration::from_secs(20),
        );
        let given: Vec<&str> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("info string search thread "))
            .filter_map(|l| l.split_once(" given ").map(|(_, g)| g))
            .collect();
        assert_eq!(given.len(), 3, "{lines:?}");
        assert!(given.iter().all(|g| *g == given[0]), "{given:?}");
        assert!(
            given[0].ends_with(&format!(" probes {probes}")),
            "{given:?}"
        );
        // A ranked root keeps its two captures; past the limit the root is unfiltered.
        assert_eq!(given[0].contains(" filter [] "), probes, "{given:?}");
    }
    let _ = std::fs::remove_dir_all(only);
}

/// `searchmoves` narrows the tables' ranking too, so a won root searched only through moves that
/// draw reports the draw and plays one of them.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn searchmoves_with_tables_ranks_and_reports_only_the_moves_named() {
    let only = fixture_set("searchmoves");
    let fen = "8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1";
    let mut b = board(fen);
    let legal = generate_legal(&b);
    let named = [
        parse_uci(&legal, "a1b1").unwrap(),
        parse_uci(&legal, "a1a2").unwrap(),
    ];
    let tables = loaded(&only.display().to_string());
    assert_eq!(tables.rank_root(&mut b, &[]).unwrap().best, Rank::Win);
    assert_eq!(tables.rank_root(&mut b, &named).unwrap().best, Rank::Draw);
    let (scores, best) = scores_after(&only, &[], fen, "go depth 6 searchmoves a1b1 a1a2");
    assert_eq!(
        scores.last().map(String::as_str),
        Some("cp 0"),
        "{scores:?}"
    );
    assert!(["a1b1", "a1a2"].contains(&best.as_str()), "{best}");
    let _ = std::fs::remove_dir_all(only);
}

/// `go mate` stops on the mate the search finds at a ranked root and reports it as the mate, and a
/// lost root the search finds mated reports the mate against it.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_ranked_root_reports_a_mate_for_either_side_under_go_mate_or_depth() {
    let only = fixture_set("mates");
    let (scores, best) = scores_after(&only, &[], "k7/8/1K6/8/8/8/7Q/8 w - - 0 1", "go mate 1");
    assert_eq!(
        scores.last().map(String::as_str),
        Some("mate 1"),
        "{scores:?}"
    );
    assert_eq!(best, "h2h8");
    let (scores, _) = scores_after(&only, &[], "k7/8/2K5/8/8/8/7Q/8 b - - 0 1", "go depth 8");
    let last = scores.last().cloned().unwrap_or_default();
    assert!(last.starts_with("mate -"), "{scores:?}");
    let _ = std::fs::remove_dir_all(only);
}

/// Past the probe limit the root is not ranked, and a win the search reads from a table below it is
/// printed as the band's large centipawn score, short of a mate.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn an_unranked_root_prints_a_table_win_found_below_it_in_the_band() {
    let only = fixture_set("unranked");
    let (scores, _) = scores_after(
        &only,
        &["setoption name SyzygyProbeLimit value 3"],
        "8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1",
        "go depth 6",
    );
    let won = centipawns(scores.last().expect("a score"));
    assert!((TB_WIN_IN_MAX_PLY..=TB_WIN).contains(&won), "{scores:?}");
    let _ = std::fs::remove_dir_all(only);
}

/// A root whose budget is spent before its first probe is not ranked: the search answers alone,
/// with a legal move and its own score rather than the table's.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_root_with_no_time_left_is_not_probed() {
    let only = fixture_set("no-time");
    let fen = "8/8/8/3k4/8/8/8/R3K3 w - - 0 1";
    let (scores, best) = scores_after(&only, &[], fen, "go movetime 0");
    let last = centipawns(scores.last().expect("a score"));
    assert!(last.abs() < TB_WIN_IN_MAX_PLY, "{scores:?}");
    assert!(
        parse_uci(&generate_legal(&board(fen)), &best).is_some(),
        "{best}"
    );
    let _ = std::fs::remove_dir_all(only);
}

/// The ranking asks before each probe and stops at the first no, so a stop or a spent budget waits
/// for one probe at most rather than for every move the root has.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn the_root_ranking_stops_between_probes_when_told() {
    let tables = loaded(&fixtures());
    let mut b = board("8/8/8/3k4/8/8/8/R3K3 w - - 0 1");
    let asked = std::cell::Cell::new(0);
    let go_on = || {
        asked.set(asked.get() + 1);
        asked.get() <= 3
    };
    assert!(tables.rank_root_while(&mut b, &[], &go_on).is_none());
    assert_eq!(asked.get(), 4);
    assert!(tables.rank_root_while(&mut b, &[], &|| true).is_some());
}

/// A drawn rank reads 0 whatever the search found, and a won or lost one the table's score unless
/// the search found a mate for the side the table says wins.
#[test]
fn a_ranked_root_reports_the_table_s_verdict_unless_the_search_mated() {
    for rank in [Rank::Draw, Rank::CursedWin, Rank::BlessedLoss] {
        assert_eq!(rank.reported(304), 0);
        assert_eq!(rank.reported(mate_in(3)), 0);
    }
    assert_eq!(Rank::Win.reported(517), tb_win_in(1));
    assert_eq!(Rank::Win.reported(mate_in(3)), mate_in(3));
    assert_eq!(Rank::Win.reported(mated_in(2)), tb_win_in(1));
    assert_eq!(Rank::Loss.reported(-517), tb_loss_in(1));
    assert_eq!(Rank::Loss.reported(mated_in(2)), mated_in(2));
    assert_eq!(Rank::Loss.reported(mate_in(3)), tb_loss_in(1));
}

/// The side to move's outcome, read for the side that moved into it.
fn for_mover(outcome: AmbiguousWdl) -> Rank {
    match outcome {
        AmbiguousWdl::Loss => Rank::Win,
        AmbiguousWdl::MaybeLoss => Rank::MaybeWin,
        AmbiguousWdl::BlessedLoss => Rank::CursedWin,
        AmbiguousWdl::Draw => Rank::Draw,
        AmbiguousWdl::CursedWin => Rank::BlessedLoss,
        AmbiguousWdl::MaybeWin => Rank::MaybeLoss,
        AmbiguousWdl::Win => Rank::Loss,
    }
}

/// A rounded distance may be a ply longer than it reads, so one landing on the hundredth half-move
/// ranks between the two outcomes it may be, on either side and at any counter.
#[test]
fn a_distance_at_the_hundredth_half_move_ranks_by_whether_it_was_rounded() {
    use MaybeRounded::{Precise, Rounded};
    let cases = [
        (Precise(Dtz(-99)), 1, Rank::Win),
        (Precise(Dtz(-100)), 1, Rank::CursedWin),
        (Rounded(Dtz(-98)), 1, Rank::Win),
        (Rounded(Dtz(-99)), 1, Rank::MaybeWin),
        (Rounded(Dtz(-100)), 1, Rank::CursedWin),
        (Rounded(Dtz(-100)), 0, Rank::MaybeWin),
        (Precise(Dtz(99)), 1, Rank::Loss),
        (Precise(Dtz(100)), 1, Rank::BlessedLoss),
        (Rounded(Dtz(98)), 1, Rank::Loss),
        (Rounded(Dtz(99)), 1, Rank::MaybeLoss),
        (Rounded(Dtz(100)), 1, Rank::BlessedLoss),
        (Rounded(Dtz(100)), 0, Rank::MaybeLoss),
        (Precise(Dtz(0)), 37, Rank::Draw),
    ];
    for (found, counter, rank) in cases {
        assert_eq!(
            Rank::from_distance(found, counter),
            rank,
            "{found:?} at {counter}"
        );
    }
}

/// Wherever the counter has run, a distance ranks as the crate's own outcome for it reads.
#[test]
fn a_distance_ranks_as_the_crate_reads_it_once_the_counter_has_run() {
    for counter in 1..=120 {
        for plies in -130..=130 {
            for found in [
                MaybeRounded::Precise(Dtz(plies)),
                MaybeRounded::Rounded(Dtz(plies)),
            ] {
                assert_eq!(
                    Rank::from_distance(found, counter),
                    for_mover(AmbiguousWdl::from_dtz_and_halfmoves(found, counter)),
                    "{found:?} at {counter}"
                );
            }
        }
    }
}

/// A WDL table answers as though the move zeroed the counter, so a win or a loss it reads is settled
/// only with the counter at zero or at a hundred half-moves.
#[test]
fn an_outcome_without_a_distance_is_settled_only_at_the_counter_s_ends() {
    use shakmaty_syzygy::Wdl as W;
    let cases = [
        (W::Loss, 0, Rank::Win),
        (W::Loss, 1, Rank::MaybeWin),
        (W::Loss, 99, Rank::MaybeWin),
        (W::Loss, 100, Rank::CursedWin),
        (W::Win, 0, Rank::Loss),
        (W::Win, 50, Rank::MaybeLoss),
        (W::Win, 100, Rank::BlessedLoss),
        (W::BlessedLoss, 40, Rank::CursedWin),
        (W::CursedWin, 40, Rank::BlessedLoss),
        (W::Draw, 100, Rank::Draw),
    ];
    for (wdl, counter, rank) in cases {
        assert_eq!(
            Rank::from_outcome(wdl, counter),
            rank,
            "{wdl:?} at {counter}"
        );
    }
}

/// The tables cannot settle an unsettled rank, so the root reports what the search found.
#[test]
fn an_unsettled_rank_reports_the_search_s_own_score() {
    for searched in [517, -517, 0, mate_in(3), mated_in(2)] {
        assert_eq!(Rank::MaybeWin.reported(searched), searched);
        assert_eq!(Rank::MaybeLoss.reported(searched), searched);
    }
}

#[test]
fn uci_declares_the_two_options() {
    let mut e = Engine::spawn();
    e.send("uci");
    let lines = e.read_until("uciok");
    assert!(lines.contains(&"option name SyzygyPath type string default <empty>".to_string()));
    assert!(
        lines.contains(&"option name SyzygyProbeLimit type spin default 7 min 0 max 7".to_string())
    );
    e.quit();
}

/// A directory that cannot be read is named and probes nothing; the session goes on.
#[test]
fn a_path_that_cannot_be_read_is_named_and_probes_nothing() {
    let mut e = Engine::spawn();
    e.send("setoption name SyzygyPath value /no/such/directory/for/cadence");
    let said = e.sync();
    assert!(
        said.iter()
            .any(|l| l.starts_with("info string SyzygyPath: /no/such/directory/for/cadence:")),
        "{said:?}"
    );
    assert!(
        said.iter()
            .any(|l| l == "info string SyzygyPath: 0 table files, probing up to 0 pieces"),
        "{said:?}"
    );
    e.send("position fen 8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1");
    e.send("go depth 3");
    let lines = e.read_until("bestmove ");
    assert!(lines.iter().all(|l| !l.contains("tbhits")), "{lines:?}");
    e.quit();
}

/// Every tablebase win sits above every evaluation and below every mate, and the table keeps it
/// relative to the node it was found at.
#[test]
fn the_band_lies_between_the_evaluations_and_the_mates() {
    assert!(tb_win_in(0) < mate_in(MAX_PLY));
    assert_eq!(tb_win_in(MAX_PLY), TB_WIN_IN_MAX_PLY);
    for ply in 0..=MAX_PLY {
        for score in [tb_win_in(ply), tb_loss_in(ply)] {
            assert!(is_decisive(score) && !is_mate(score), "{score}");
            assert_eq!(from_tt(to_tt(score, ply), ply), score);
        }
        assert_eq!(from_tt(to_tt(tb_win_in(ply), ply), 0), TB_WIN);
        assert!(is_decisive(mate_in(ply)) && is_decisive(mated_in(ply)));
    }
    assert!(!is_decisive(MAX_EVAL + MAX_CORRECTION));
}

/// Futility and the probe cut would fire at a tablebase bound if it were read as an evaluation.
#[test]
fn a_tablebase_bound_refuses_the_margins() {
    for ply in 1..8 {
        for depth in 1..4 {
            assert!(
                !futile_node(Some(0), depth, tb_win_in(ply)),
                "depth {depth}"
            );
        }
        for depth in 5..12 {
            for beta in [tb_win_in(ply), tb_loss_in(ply)] {
                assert!(
                    probcut_bound(Some(0), depth, beta - 1, beta).is_none(),
                    "{beta}"
                );
            }
        }
    }
}

/// The rules that keep a margin or a learner off the decided scale read `is_decisive`; `is_mate`
/// is left to spelling a mate, `go mate` and datagen.
#[test]
fn no_search_rule_reads_the_mate_scale_alone() {
    for (name, source) in [
        ("search.rs", include_str!("../src/search.rs")),
        ("node.rs", include_str!("../src/search/node.rs")),
        ("pruning.rs", include_str!("../src/search/pruning.rs")),
        ("root.rs", include_str!("../src/search/root.rs")),
        ("counters.rs", include_str!("../src/search/counters.rs")),
        ("quiesce.rs", include_str!("../src/search/quiesce.rs")),
        ("depth.rs", include_str!("../src/search/depth.rs")),
    ] {
        assert!(!source.contains("is_mate("), "{name}");
    }
}

/// The moves as `bestmove` spells them, sorted.
fn spelled(moves: &[Move]) -> Vec<String> {
    let mut out: Vec<String> = moves.iter().map(|m| m.to_uci_chess960()).collect();
    out.sort();
    out
}

/// A five-piece pawn ending where each king move wins and the e-pawn's push draws. At a counter of
/// 89 only the wins ten plies from zeroing still end inside the hundredth half-move.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_five_piece_pawn_ending_keeps_only_the_wins_its_counter_allows() {
    let tables = loaded(&fixtures());
    let mut fresh = board("8/8/2k5/3pP3/3P4/8/8/4K3 w - - 0 1");
    assert_eq!(tables.probe_wdl(&fresh), Some(Wdl::Win));
    assert_eq!(tables.probe_dtz(&fresh), Some(11));
    let fastest = ["e1d2", "e1e2", "e1f2"];
    let ranked = tables.rank_root(&mut fresh, &[]).expect("a ranked root");
    assert_eq!(ranked.best, Rank::Win);
    assert_eq!(spelled(&ranked.moves), fastest);
    let mut late = board("8/8/2k5/3pP3/3P4/8/8/4K3 w - - 89 60");
    let ranked = tables.rank_root(&mut late, &[]).expect("a ranked root");
    assert!(
        matches!(ranked.best, Rank::Win | Rank::MaybeWin),
        "{:?}",
        ranked.best
    );
    assert_eq!(spelled(&ranked.moves), fastest);
    let legal = generate_legal(&fresh);
    for king in ["e1d1", "e1d2", "e1e2", "e1f1", "e1f2"] {
        let m = parse_uci(&legal, king).unwrap();
        let fresh_rank = tables
            .rank_root(&mut fresh, &[m])
            .expect("a ranked move")
            .best;
        assert_eq!(fresh_rank, Rank::Win, "{king}");
        let late_rank = tables
            .rank_root(&mut late, &[m])
            .expect("a ranked move")
            .best;
        let allowed = matches!(late_rank, Rank::Win | Rank::MaybeWin);
        assert_eq!(allowed, fastest.contains(&king), "{king}: {late_rank:?}");
    }
}

/// Black's last move was d7d5, so e5 takes en passant, and that wins as e6 does; without the square
/// only e6 wins.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn an_en_passant_capture_in_a_five_piece_ending_is_ranked() {
    let tables = loaded(&fixtures());
    let mut with = board("8/8/8/3pP3/5k2/8/P7/7K w - d6 0 1");
    assert_eq!(tables.probe_wdl(&with), Some(Wdl::Win));
    let ranked = tables.rank_root(&mut with, &[]).expect("a ranked root");
    assert_eq!(ranked.best, Rank::Win);
    assert_eq!(spelled(&ranked.moves), ["e5d6", "e5e6"]);
    let mut without = board("8/8/8/3pP3/5k2/8/P7/7K w - - 0 1");
    let ranked = tables.rank_root(&mut without, &[]).expect("a ranked root");
    assert_eq!(spelled(&ranked.moves), ["e5e6"]);
}

/// Every quiet move into a loss is ranked at the counters putting its end a ply inside, on and a
/// ply past the hundredth half-move. A rounded distance on it may be a ply longer, so it ranks
/// below a certain win, and these five-piece tables must store at least one.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_rounded_five_piece_distance_on_the_hundredth_half_move_is_not_a_certain_win() {
    let tables = loaded(&fixtures());
    let mut rounded = 0;
    for root in [
        "8/8/8/4k3/8/8/r7/RR2K3 w - -",
        "8/8/2k5/3pP3/3P4/8/8/4K3 w - -",
        "8/8/1k6/8/pP6/P7/8/1K6 w - -",
    ] {
        let mut start = board(&format!("{root} 0 1"));
        for m in generate_legal(&start).iter() {
            start.make_move(m);
            let quiet = start.halfmove_clock() > 0 && !generate_legal(&start).is_empty();
            let Some(found) = tables.probe_dtz(&start) else {
                panic!("{root} {m:?}: no distance");
            };
            start.unmake_move(m);
            let plies = found.unsigned_abs();
            if !quiet || found >= 0 || plies > 98 {
                continue;
            }
            let rank_at = |end: u32| {
                let mut b = board(&format!("{root} {} 1", end - plies - 1));
                tables.rank_root(&mut b, &[m]).expect("a ranked move").best
            };
            assert_eq!(rank_at(99), Rank::Win, "{root} {m:?}");
            assert_eq!(rank_at(101), Rank::CursedWin, "{root} {m:?}");
            match rank_at(100) {
                Rank::Win => {}
                Rank::MaybeWin => rounded += 1,
                other => panic!("{root} {m:?}: {other:?}"),
            }
        }
    }
    assert!(rounded > 0, "no quiet move read a rounded distance");
}
