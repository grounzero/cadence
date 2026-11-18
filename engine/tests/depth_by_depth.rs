// SPDX-License-Identifier: GPL-3.0-or-later

//! The bench's king and queen position, searched by the binary that ships and printed depth by
//! depth. It fails on purpose so CI shows the lines, and passes without `CADENCE_BIN`.

mod support;

use support::Engine;

const POSITION: &str = "position fen 8/2k5/8/8/8/8/1Q6/K7 w - - 0 1";

#[test]
fn the_king_and_queen_position_depth_by_depth() {
    if std::env::var_os("CADENCE_BIN").is_none() {
        return;
    }
    let setup = ["setoption name Hash value 16", "ucinewgame", POSITION];
    let lines = Engine::go(&setup, "go depth 13");
    let shown: Vec<&str> = lines
        .iter()
        .map(String::as_str)
        .filter(|l| l.starts_with("info depth") || l.starts_with("bestmove"))
        .collect();
    panic!("depth by depth:\n{}", shown.join("\n"));
}

/// Each counter at depth ten and eleven, in the test build with the shipped net, as the bench runs.
#[test]
fn the_king_and_queen_position_counted_at_ten_and_eleven() {
    let report = std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(|| [10, 11].map(counted).join("\n"))
        .expect("a search thread")
        .join()
        .expect("the search finishes");
    panic!("counted:\n{report}");
}

fn counted(depth: u32) -> String {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use cadence_core::position::Board;
    use cadence_engine::position::Position;
    use cadence_engine::search::{Limits, Search};
    use cadence_engine::tt::Table;
    use cadence_nnue::Network;
    use cadence_nnue::net::EMBEDDED;

    let board = Board::from_fen("8/2k5/8/8/8/8/1Q6/K7 w - - 0 1").expect("the position");
    let net = Network::shipped(EMBEDDED)
        .expect("a shipped net")
        .expect("it loads");
    let mut pos = Position::with_net(board, Arc::new(net));
    let tt = Table::new(16).expect("a table");
    let stop = AtomicBool::new(false);
    let mut s = Search::new(&stop, &tt);
    s.set_limits(Limits::depth(depth));
    let _ = cadence_engine::search::spread_take();
    let best = s.run(&mut pos, &mut std::io::sink());
    let spread = cadence_engine::search::spread_take();
    let at = |i: usize| spread.get(i).copied().unwrap_or(0);
    let by_depth: Vec<u64> = (1..16).map(at).collect();
    let by_ply: Vec<u64> = (32..72).map(at).collect();
    let moves = format!(
        "extended {} unextended {} reopened {} first {} first_checks {} first_quiet {}",
        at(96),
        at(102),
        at(97),
        at(100),
        at(99),
        at(103)
    );
    let cuts = format!(
        "cut by first {} by later {} by a check {} by a quiet move {}",
        at(104),
        at(105),
        at(106),
        at(107)
    );
    let counts = format!(
        "depth {depth} best {best:?} nodes {} evals {} iir {} lmr {} lmr_re {} null {}/{} rfp {} fut {} lmp {} probcut {}/{} hist_less {} hist_more {}",
        s.nodes(),
        s.static_evals(),
        s.iir_nodes(),
        s.lmr_reductions(),
        s.lmr_researches(),
        s.null_cutoffs(),
        s.null_attempts(),
        s.reverse_futility_cutoffs(),
        s.futility_skipped(),
        s.lmp_skipped(),
        s.probcut_cutoffs(),
        s.probcut_attempts(),
        s.history_reduced_less(),
        s.history_reduced_more(),
    );
    format!("{counts}\n  {moves}\n  {cuts}\n  by depth 1..15 {by_depth:?}\n  by ply 0..39 {by_ply:?}")
}
