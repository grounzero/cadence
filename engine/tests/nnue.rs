// SPDX-License-Identifier: GPL-3.0-or-later

//! The net's switch: what `EvalFile` loads and refuses, that `Use NNUE` alone decides which
//! evaluation scores, and that a search leaves the accumulators at the root it found.

mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use cadence_core::position::Board;
use cadence_engine::position::Position;
use cadence_engine::search::Limits;
use cadence_nnue::{Network, Provenance};
use support::Engine;

const FEN: &str = "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 3 3";

fn net_file(name: &str, bytes: &[u8]) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("cadence-nnue-{name}-{}.cdnn", std::process::id()));
    std::fs::write(&path, bytes).expect("write the net file");
    path
}

/// Everything on an iteration line but the two wall-clock readings.
fn searched(out: &[String]) -> Vec<String> {
    out.iter()
        .filter(|l| l.starts_with("info depth "))
        .map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            let clocked = |i: usize| {
                ["nps", "time"].contains(&t[i]) || (i > 0 && ["nps", "time"].contains(&t[i - 1]))
            };
            (0..t.len())
                .filter(|&i| !clocked(i))
                .map(|i| t[i])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

#[test]
fn uci_declares_the_net_options() {
    let out = support::talk("uci\nquit\n");
    assert!(
        out.lines()
            .any(|l| l == "option name Use NNUE type check default false"),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l == "option name EvalFile type string default <empty>"),
        "{out}"
    );
}

/// A loaded net scores nothing until `Use NNUE` is on; on, it changes the search and repeats it
/// exactly.
#[test]
fn the_net_scores_only_while_use_nnue_is_on() {
    let file = net_file(
        "switch",
        &Network::seeded(7, Provenance::SelfPlay).to_bytes(),
    );
    let load = format!("setoption name EvalFile value {}", file.display());
    let on = "setoption name Use NNUE value true";
    let position = format!("position fen {FEN}");
    let go = "go depth 6";
    let handcrafted = searched(&Engine::go(&[&position], go));
    let loaded_off = searched(&Engine::go(&[&load, &position], go));
    let with_net = searched(&Engine::go(&[&load, on, &position], go));
    let again = searched(&Engine::go(&[on, &load, &position], go));
    let _ = std::fs::remove_file(file);
    assert_eq!(handcrafted.len(), 6);
    assert_eq!(loaded_off, handcrafted);
    assert_ne!(with_net, handcrafted);
    assert_eq!(with_net, again);
}

/// Each load is reported, a file the loader refuses is named with the reason and leaves no net,
/// and a search asked for a net it does not have says so.
#[test]
fn a_net_file_is_reported_loaded_or_refused() {
    let external = Provenance::External("lichess-big3-resolved".to_string());
    let good = net_file("good", &Network::seeded(7, external).to_bytes());
    let mut bytes = Network::seeded(7, Provenance::SelfPlay).to_bytes();
    let whole = bytes.len();
    bytes.pop();
    let short = net_file("short", &bytes);
    let mut e = Engine::spawn();
    e.send(&format!("setoption name EvalFile value {}", good.display()));
    let said = e.sync();
    let loaded = format!(
        "info string EvalFile: {} loaded, provenance lichess-big3-resolved, trainer {}",
        good.display(),
        "0".repeat(40)
    );
    assert!(said.contains(&loaded), "{said:?}");
    e.send(&format!(
        "setoption name EvalFile value {}",
        short.display()
    ));
    let said = e.sync();
    let refused = format!(
        "info string EvalFile: {}: {} bytes where a net is {whole}",
        short.display(),
        whole - 1
    );
    assert!(said.contains(&refused), "{said:?}");
    e.send("setoption name Use NNUE value true");
    e.send("go depth 2");
    let lines = e.read_until("bestmove ");
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("info string Use NNUE is on and no net is loaded")),
        "{lines:?}"
    );
    e.quit();
    let _ = std::fs::remove_file(good);
    let _ = std::fs::remove_file(short);
}

/// Each helper thread scores from accumulators of its own, so a net search on three threads
/// finishes with a move.
#[test]
fn helper_threads_search_with_the_net() {
    let file = net_file(
        "threads",
        &Network::seeded(9, Provenance::SelfPlay).to_bytes(),
    );
    let load = format!("setoption name EvalFile value {}", file.display());
    let position = format!("position fen {FEN}");
    let (_, lines) = Engine::go_within(
        &[
            &load,
            "setoption name Use NNUE value true",
            "setoption name Threads value 3",
            &position,
        ],
        "go depth 8",
        Duration::from_secs(120),
    );
    let _ = std::fs::remove_file(file);
    let last = lines.last().expect("a bestmove line");
    assert!(
        last.starts_with("bestmove ") && last != "bestmove 0000",
        "{lines:?}"
    );
}

/// A search walks the accumulators down and back, so it leaves them at the root, scoring it as a
/// fresh refresh does; the node limit stops some mid-tree, so the abort path unwinds too.
#[test]
fn a_search_leaves_the_accumulators_at_the_root() {
    let net = Arc::new(Network::seeded(11, Provenance::SelfPlay));
    for fen in support::corpus_fens().iter().take(16) {
        let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
        let fresh = Position::with_net(board.duplicate(), Arc::clone(&net)).net_eval();
        let mut pos = Position::with_net(board, Arc::clone(&net));
        let stop = AtomicBool::new(false);
        let tt = support::table();
        let limits = Limits {
            depth: Some(6),
            nodes: Some(200_000),
            ..Limits::default()
        };
        let mut search = support::search(limits, &stop, &tt);
        let _ = search.run(&mut pos, &mut std::io::sink());
        assert_eq!(pos.net_ply(), Some(0), "{fen}");
        assert_eq!(pos.net_eval(), fresh, "{fen}");
    }
}
