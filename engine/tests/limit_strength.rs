// SPDX-License-Identifier: GPL-3.0-or-later

//! Playing down: a target rating picks a candidate count, a margin and a halving
//! constant from a compiled-in table, and the move emitted is sampled from the
//! root lines that survive the margin rather than being the best one.
//!
//! What these gates demonstrate is that the number **cannot reach the search**
//! unless the boolean gate is on, that a level once on **changes the move**
//! rather than being accepted and ignored, and that the choice is **reproducible
//! from the position**, which is the property an investigation needs and the one
//! a process-seeded generator would destroy.
//!
//! The inertness gate is the one the whole option is built under, and it is
//! stated as a node count rather than a move, because a move can agree by
//! accident and a node count cannot.

mod support;

use cadence_engine::level::{self, MAX_ELO, MIN_ELO};
use support::{Engine, talk};

/// Positions the gates search. Middlegames rather than the start, so each root
/// holds enough moves for a candidate set and enough spread for a margin to bind.
const FENS: [&str; 4] = [
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    "r2q1rk1/1b1nbppp/pp1ppn2/8/2PNP3/1PN1B3/P2QBPPP/R4RK1 w - - 0 12",
    "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 0 12",
    "rn1qkbnr/pp2pppp/2p5/3pPb2/3P4/8/PPP2PPP/RNBQKBNR w KQkq - 1 4",
];

/// The depth every gate searches to. Seven: deep enough that the root lines
/// separate, and a fraction of a second in debug.
const GATE_DEPTH: u32 = 7;

/// The node budget the abort gate searches under. Enough to pass several
/// iterations and never enough to finish the one it is cut off in, which is the
/// condition every limit but a fixed depth puts the search under.
const GATE_NODES: u64 = 300_000;

/// Every rung of the compiled-in ladder, which is what a gate walks when it has
/// to hold at every value rather than at one.
fn rungs() -> Vec<u32> {
    level::LADDER.iter().map(|rung| rung.elo).collect()
}

/// One search of `fen` to [`GATE_DEPTH`], with `setup` sent before it. Through
/// the line-at-a-time driver rather than one write, because a `quit` queued
/// behind a `go` stops the search before it has reported anything.
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

/// The move on the `bestmove` line of one search.
fn played(out: &[String]) -> String {
    let Some(rest) = out.iter().find_map(|l| l.strip_prefix("bestmove ")) else {
        panic!("no bestmove line in {out:?}")
    };
    rest.split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One search of `fen` under [`GATE_NODES`], which aborts its last iteration.
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

/// The setup that turns a level on, which every behaviour gate below sends.
fn level_on(elo: u32) -> [String; 2] {
    [
        "setoption name UCI_LimitStrength value true".to_string(),
        format!("setoption name UCI_Elo value {elo}"),
    ]
}

fn as_refs(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).collect()
}

/// The `nodes` field of the last iteration line, which is the whole search's count.
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

/// Both options, declared the way a GUI and the bridge already understand: a
/// boolean gate and a number, which are deliberately different kinds of thing.
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

/// **The gate the whole option is built under.** With the boolean false, no value
/// of the number may move the node count or the move, so the search a rating
/// list and the regression detector play is the search this file cannot reach.
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

/// The gate on its own is not a level either: it needs a number, and the number
/// it defaults to is the top of the ladder, which samples from two lines at the
/// narrowest margin the table holds.
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

/// A level engages only where the root reports one line, in both orders, and the
/// refusal is spoken rather than silent. A silent refusal is indistinguishable
/// from a level that is on, which is the failure a GUI would never see.
#[test]
fn a_level_refuses_while_more_than_one_line_is_reported() {
    let plain = search(FENS[0], &[]);
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

/// A level engages only on one thread, in both orders, and says so. Above one
/// the search is not reproducible run to run, so a level there would keep the
/// option and lose the property the option is for. **This is the configuration
/// the bot actually runs**, which is why it is a refusal and not a caveat.
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

/// A level must change what the engine plays. An option accepted and ignored
/// passes every other gate here, so this is the one that says it does something.
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

/// The same position at the same level yields the same move, because the seed is
/// the position rather than the process. This is what makes a complaint about a
/// move reproducible, and it is why a process-seeded generator is refused.
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

/// A sampled move is one the search scored, never one it did not look at. The
/// candidate count bounds how bad the choice can be, which is the whole of the
/// worst case this policy has.
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

/// **A level samples under a limit that aborts, which is every limit a game is
/// played under.** Fixed depth is the one that completes its last iteration, and
/// it is the only one the other gates here use, so a sampler that read a
/// half-finished iteration would pass all of them and fire in no real game.
///
/// Stated as a rate over positions rather than as one move, because sampling is
/// allowed to return the best line and often should.
#[test]
fn a_level_samples_under_a_node_limit_as_well_as_a_fixed_depth() {
    for elo in rungs() {
        let mut deviated = 0;
        let mut counted = 0;
        for fen in FENS {
            let out = search_to_node_limit(fen, &as_refs(&level_on(elo)));
            let seen = candidate_moves(&out);
            if seen.len() < 2 {
                continue;
            }
            counted += 1;
            if played(&out) != seen[0] {
                deviated += 1;
            }
        }
        assert!(
            counted > 0,
            "level {elo} reported fewer than two lines on every position, so the \
             node limit left it nothing to sample from"
        );
        assert!(
            deviated > 0,
            "level {elo} played its best line on all {counted} positions under a \
             node limit, so the level is inert wherever an iteration is cut short"
        );
    }
}

/// A level must report the lines it sampled among, whatever cut the search
/// short. The board reads them to show the candidates and mark the one taken,
/// and a truncated set is a different set from the one the move came from.
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

/// The table is monotone in what the engine can see, which is the candidate count
/// and the margin. **It is deliberately not monotone in the halving constant**:
/// the ladder's monotone quantity is expected centipawn loss, and the count and
/// the margin move underneath it, so a rung that samples from fewer lines needs a
/// flatter distribution to lose the same amount.
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

/// A number between two rungs resolves to a rung rather than to nothing. The
/// interface takes one number and the table holds five, so every value in the
/// declared range has to land somewhere.
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
