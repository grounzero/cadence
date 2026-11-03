// SPDX-License-Identifier: GPL-3.0-or-later

//! The ignored tests read the tables `tests/fixtures/syzygy.sha256` names from the directory
//! `CADENCE_SYZYGY_PATH` gives, which CI fetches by checksum. The rest need no table.

mod support;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use cadence_core::position::Board;
use cadence_core::{MAX_PLY, generate_legal, parse_uci};
use cadence_engine::corrhist::MAX_CORRECTION;
use cadence_engine::score::{
    MAX_EVAL, TB_WIN, TB_WIN_IN_MAX_PLY, from_tt, is_decisive, is_mate, mate_in, mated_in,
    tb_loss_in, tb_win_in, to_tt,
};
use cadence_engine::search::{Limits, futile_node, probcut_bound};
use cadence_engine::syzygy::{Rank, Tables, Wdl};
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

/// A castling right, too many pieces, a missing table and a spoilt one are each no result, and
/// none panics.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn outside_the_tables_there_is_no_result_and_no_panic() {
    let tables = loaded(&fixtures());
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
}

/// A path loads the tables and prints how many; an empty one turns probing off, and every line a
/// search prints without tables is the line it was.
#[test]
#[ignore = "reads the Syzygy fixtures from CADENCE_SYZYGY_PATH"]
fn a_path_set_over_uci_probes_and_an_empty_one_stops() {
    let mut e = Engine::spawn();
    e.send(&format!("setoption name SyzygyPath value {}", fixtures()));
    let said = e.sync();
    assert!(
        said.iter()
            .any(|l| l == "info string SyzygyPath: 12 table files, probing up to 4 pieces"),
        "{said:?}"
    );
    e.send("setoption name SyzygyProbeLimit value 3");
    let said = e.sync();
    assert!(
        said.iter()
            .any(|l| l == "info string SyzygyPath: 12 table files, probing up to 3 pieces"),
        "{said:?}"
    );
    e.send("position fen 8/8/8/8/8/8/1b2R3/K5k1 w - - 0 1");
    e.send("go depth 4");
    let lines = e.read_until("bestmove ");
    assert!(lines.iter().any(|l| l.contains(" tbhits ")), "{lines:?}");
    e.send("setoption name SyzygyPath value <empty>");
    e.send("go depth 4");
    let lines = e.read_until("bestmove ");
    assert!(lines.iter().all(|l| !l.contains("tbhits")), "{lines:?}");
    e.quit();
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
