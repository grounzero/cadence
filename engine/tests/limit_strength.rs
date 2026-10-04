// SPDX-License-Identifier: GPL-3.0-or-later

//! The number cannot reach the search unless the boolean is on, a level changes the move, and the
//! choice is reproducible from the position. Inertness is stated as a node count, because a move
//! can agree by accident and a node count cannot.

mod support;

use cadence_engine::level::{self, MAX_ELO, MIN_ELO};
use support::{Engine, talk};

/// Middlegames, so each root holds enough moves for a candidate set and enough spread for a margin
/// to bind.
const FENS: [&str; 4] = [
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    "r2q1rk1/1b1nbppp/pp1ppn2/8/2PNP3/1PN1B3/P2QBPPP/R4RK1 w - - 0 12",
    "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 0 12",
    "rn1qkbnr/pp2pppp/2p5/3pPb2/3P4/8/PPP2PPP/RNBQKBNR w KQkq - 1 4",
];

/// Four more for the sampling gate alone, taken from the bench list by a rule that reads no
/// evaluation: the first four in file order off the check, with 30 legal moves and most material.
const SAMPLING_FENS: [&str; 4] = [
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R b KQkq - 0 1",
    "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    "r1bqkbnr/pppp1ppp/2n5/1B2p3/4P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 3 3",
];

/// Deep enough that the root lines separate, a fraction of a second in debug.
const GATE_DEPTH: u32 = 7;

/// Several iterations and never enough to finish the one it cuts off, as every limit but fixed
/// depth does.
const GATE_NODES: u64 = 300_000;

fn rungs() -> Vec<u32> {
    level::LADDER.iter().map(|rung| rung.elo).collect()
}

/// Line at a time, because a `quit` queued behind a `go` stops the search before it reports.
fn search(fen: &str, setup: &[&str]) -> Vec<String> {
    let mut lines: Vec<&str> = setup.to_vec();
    let position = format!("position fen {fen}");
    lines.push(&position);
    let (_, out) = Engine::go_within(
        &lines,
        &format!("go depth {GATE_DEPTH}"),
        std::time::Duration::from_secs(60),
    );
    out
}

fn played(out: &[String]) -> String {
    let Some(rest) = out.iter().find_map(|l| l.strip_prefix("bestmove ")) else {
        panic!("no bestmove line in {out:?}")
    };
    rest.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Which aborts its last iteration.
fn search_to_node_limit(fen: &str, setup: &[&str]) -> Vec<String> {
    let mut lines: Vec<&str> = setup.to_vec();
    let position = format!("position fen {fen}");
    lines.push(&position);
    let (_, out) = Engine::go_within(
        &lines,
        &format!("go nodes {GATE_NODES}"),
        std::time::Duration::from_secs(60),
    );
    out
}

fn level_on(elo: u32) -> [String; 2] {
    [
        "setoption name UCI_LimitStrength value true".to_string(),
        format!("setoption name UCI_Elo value {elo}"),
    ]
}

/// Changes the sampler's draw and nothing about the search.
fn with_later_move_number(fen: &str, later: u32) -> String {
    let mut fields: Vec<String> = fen.split_whitespace().map(str::to_string).collect();
    let number: u32 = fields[5].parse().expect("a move number");
    fields[5] = (number + later).to_string();
    fields.join(" ")
}

fn as_refs(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).collect()
}

/// The whole search's count.
fn nodes(out: &[String]) -> u64 {
    out.iter()
        .filter(|l| l.starts_with("info depth"))
        .filter_map(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            let i = parts.iter().position(|p| *p == "nodes")?;
            parts.get(i + 1)?.parse().ok()
        })
        .next_back()
        .unwrap_or_else(|| panic!("no nodes field in {out:?}"))
}

/// The root move of every line of the deepest iteration that reported one.
fn candidate_moves(out: &[String]) -> Vec<String> {
    let deepest = out
        .iter()
        .filter(|l| l.starts_with("info depth") && l.contains(" pv "))
        .filter_map(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            let depth: u32 = parts.get(2)?.parse().ok()?;
            let mv = l.split(" pv ").nth(1)?.split_whitespace().next()?;
            Some((depth, mv.to_string()))
        })
        .collect::<Vec<_>>();
    let max = deepest.iter().map(|(d, _)| *d).max().unwrap_or(0);
    deepest
        .into_iter()
        .filter(|(d, _)| *d == max)
        .map(|(_, mv)| mv)
        .collect()
}

/// A boolean gate and a number, deliberately different kinds of thing.
#[test]
fn uci_advertises_the_strength_pair() {
    let out = talk("uci\nquit\n");
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines.contains(&"option name UCI_LimitStrength type check default false"),
        "no UCI_LimitStrength line in {lines:?}"
    );
    assert!(
        lines.contains(
            &format!("option name UCI_Elo type spin default {MAX_ELO} min {MIN_ELO} max {MAX_ELO}")
                .as_str()
        ),
        "no UCI_Elo line in {lines:?}"
    );
}

/// With the boolean false no value of the number may move the node count or the move.
#[test]
fn the_number_is_inert_at_every_value_while_the_gate_is_off() {
    for fen in FENS {
        let base = search(fen, &[]);
        let (want_nodes, want_move) = (nodes(&base), played(&base));
        for elo in rungs() {
            let got = search(fen, &[&format!("setoption name UCI_Elo value {elo}")]);
            assert_eq!(
                nodes(&got),
                want_nodes,
                "UCI_Elo {elo} moved the node count with the gate off, on {fen}"
            );
            assert_eq!(
                played(&got),
                want_move,
                "UCI_Elo {elo} moved the move with the gate off, on {fen}"
            );
        }
    }
}

/// The number defaults to the top of the ladder, two lines at the narrowest margin.
#[test]
fn the_gate_alone_takes_the_top_of_the_ladder() {
    let out = search(FENS[0], &["setoption name UCI_LimitStrength value true"]);
    let top = level::policy(MAX_ELO);
    assert_eq!(
        candidate_moves(&out).len(),
        top.candidates,
        "the gate alone did not take the top rung's candidate count"
    );
}

/// In both orders, and spoken: a silent refusal looks like a level that is on.
#[test]
fn a_level_refuses_while_more_than_one_line_is_reported() {
    // MultiPV can change the best move at a fixed depth, so a refused level is measured against it.
    let plain = search(FENS[0], &["setoption name MultiPV value 4"]);
    let on = level_on(MIN_ELO);

    let mut level_last = vec!["setoption name MultiPV value 4".to_string()];
    level_last.extend(on.iter().cloned());
    let out = search(FENS[0], &as_refs(&level_last));
    assert!(
        out.iter().any(|l| l.starts_with("info string")
            && l.contains("UCI_Elo")
            && l.contains("MultiPV")),
        "no spoken refusal when the level arrived second, in {out:?}"
    );
    assert_eq!(
        played(&out),
        played(&plain),
        "a refused level still moved the move"
    );

    let mut multipv_last: Vec<String> = on.to_vec();
    multipv_last.push("setoption name MultiPV value 4".to_string());
    let out = search(FENS[0], &as_refs(&multipv_last));
    assert!(
        out.iter()
            .any(|l| l.starts_with("info string") && l.contains("MultiPV")),
        "no spoken refusal when MultiPV arrived second, in {out:?}"
    );
    assert_eq!(
        candidate_moves(&out).len(),
        level::policy(MIN_ELO).candidates,
        "MultiPV took effect against a standing level"
    );
}

/// Above one thread the search is not reproducible, so a level there loses the property the option
/// is for. This is the configuration the bot runs, hence a refusal rather than a caveat.
#[test]
fn a_level_refuses_beside_more_than_one_thread() {
    let on = level_on(MIN_ELO);

    let mut level_last = vec!["setoption name Threads value 2".to_string()];
    level_last.extend(on.iter().cloned());
    let out = search(FENS[0], &as_refs(&level_last));
    assert!(
        out.iter()
            .any(|l| l.starts_with("info string") && l.contains("Threads")),
        "no spoken refusal when the level arrived second, in {out:?}"
    );
    assert_eq!(
        candidate_moves(&out).len(),
        1,
        "a refused level still took the root's line count"
    );

    let mut threads_last: Vec<String> = on.to_vec();
    threads_last.push("setoption name Threads value 2".to_string());
    let out = search(FENS[0], &as_refs(&threads_last));
    assert!(
        out.iter()
            .any(|l| l.starts_with("info string") && l.contains("Threads")),
        "no spoken refusal when Threads arrived second, in {out:?}"
    );
    assert_eq!(
        candidate_moves(&out).len(),
        level::policy(MIN_ELO).candidates,
        "Threads took effect against a standing level"
    );
}

/// An option accepted and ignored passes every other gate here.
#[test]
fn the_lowest_level_plays_a_move_the_search_did_not_prefer() {
    let on = level_on(MIN_ELO);
    let moved = FENS.iter().filter(|fen| {
        let best = played(&search(fen, &[]));
        played(&search(fen, &as_refs(&on))) != best
    });
    assert!(
        moved.count() > 0,
        "the lowest level played the best move on every position tried"
    );
}

/// The seed is the position, not the process, so a complaint about a move is reproducible.
#[test]
fn the_same_position_and_level_yield_the_same_move() {
    let on = level_on(MIN_ELO);
    for fen in FENS {
        let first = played(&search(fen, &as_refs(&on)));
        for _ in 0..2 {
            assert_eq!(
                played(&search(fen, &as_refs(&on))),
                first,
                "the same position and level played two different moves on {fen}"
            );
        }
    }
}

/// The candidate count bounds how bad the choice can be.
#[test]
fn a_sampled_move_is_always_one_of_the_reported_candidates() {
    for fen in FENS {
        for elo in rungs() {
            let out = search(fen, &as_refs(&level_on(elo)));
            let played = played(&out);
            let seen = candidate_moves(&out);
            assert!(
                seen.contains(&played),
                "level {elo} played {played}, which is in none of its lines, on {fen}"
            );
            assert!(
                seen.len() <= level::policy(elo).candidates,
                "level {elo} reported more lines than its candidate count, on {fen}"
            );
        }
    }
}

/// Every limit a game is played under aborts, and a sampler reading a half-finished iteration would
/// pass every fixed-depth gate. Four move numbers per position give thirty-two draws, since eight
/// once left level 1600 on its best line by a chance under one in a hundred.
#[test]
fn a_level_samples_under_a_node_limit_as_well_as_a_fixed_depth() {
    for elo in rungs() {
        let mut deviated = 0;
        let mut counted = 0;
        for fen in FENS.iter().chain(&SAMPLING_FENS) {
            for later in [0, 10, 20, 30] {
                let fen = with_later_move_number(fen, later);
                let out = search_to_node_limit(&fen, &as_refs(&level_on(elo)));
                let seen = candidate_moves(&out);
                if seen.len() < 2 {
                    continue;
                }
                counted += 1;
                if played(&out) != seen[0] {
                    deviated += 1;
                }
            }
        }
        assert!(
            counted > 0,
            "level {elo} reported fewer than two lines on every draw, so the \
             node limit left it nothing to sample from"
        );
        assert!(
            deviated > 0,
            "level {elo} played its best line on all {counted} draws under a \
             node limit, so the level is inert wherever an iteration is cut short"
        );
    }
}

/// The board shows the candidates and marks the one taken, and a truncated set is not the set the
/// move came from.
#[test]
fn a_level_reports_its_full_candidate_set_under_a_node_limit() {
    for elo in rungs() {
        let wanted = level::policy(elo).candidates;
        for fen in FENS {
            let out = search_to_node_limit(fen, &as_refs(&level_on(elo)));
            assert_eq!(
                candidate_moves(&out).len(),
                wanted,
                "level {elo} reported a short candidate set on {fen}"
            );
        }
    }
}

/// Deliberately not monotone in the halving constant: the ladder's monotone quantity is expected
/// centipawn loss.
#[test]
fn the_policy_table_is_monotone_in_the_level() {
    let rungs = rungs();
    assert!(rungs.len() >= 2, "a ladder needs at least two rungs");
    assert_eq!(rungs.first().copied(), Some(MIN_ELO));
    assert_eq!(rungs.last().copied(), Some(MAX_ELO));
    for pair in rungs.windows(2) {
        let (low, high) = (level::policy(pair[0]), level::policy(pair[1]));
        assert!(pair[0] < pair[1], "the ladder is not ascending at {pair:?}");
        assert!(
            low.candidates >= high.candidates,
            "candidates rise with the level between {pair:?}"
        );
        assert!(
            low.margin >= high.margin,
            "the margin narrows as the level falls between {pair:?}"
        );
        assert!(
            low.halving > 0 && high.halving > 0,
            "a halving constant of zero divides by nothing, between {pair:?}"
        );
        assert!(
            low.candidates >= 2,
            "a rung that samples from one line samples nothing, at {pair:?}"
        );
    }
}

/// The interface takes one number and the table holds five, so every value has to land somewhere.
#[test]
fn every_value_in_the_declared_range_resolves_to_a_rung() {
    for elo in (MIN_ELO..=MAX_ELO).step_by(50) {
        let policy = level::policy(elo);
        assert!(
            level::LADDER.iter().any(|rung| rung.policy == policy),
            "UCI_Elo {elo} resolved to a policy that is on no rung"
        );
    }
}
