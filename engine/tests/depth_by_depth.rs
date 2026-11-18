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
