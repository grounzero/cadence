// SPDX-License-Identifier: GPL-3.0-or-later

//! The search constants a tune may move, and the three places they surface: the options `uci`
//! declares, the block `cadence spsa` prints, and the values a search reads.
//!
//! A tuner delivers every value as `setoption name <name> value <v>`, and an engine ignores an
//! option it does not know. So a misspelled name, a float that does not parse, or an option that
//! is accepted and reaches nothing all fail in silence, as a tune that runs to its end and moves
//! nothing. Each is gated here instead: the two printed forms are parsed independently and
//! compared, every malformed spelling is shown to be refused with the value kept, and every
//! option is shown to change a search through the pipe a tuner uses.

mod support;

use std::process::Command;

use cadence_engine::search::{Limits, REDUCTION_INDEX, lmp_count};
use cadence_engine::tune::{self, Kind, PARAMS, Param, Tunable, Tunables};
use cadence_engine::uci::Session;
use support::{Engine, talk};

/// A quiet middlegame, the position the pruning gates use, where every rule the table reaches
/// has nodes to act on at the depth below.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// Deep enough that each rule fires many times over, shallow enough that a subprocess per
/// setting stays cheap.
const DEPTH: u32 = 8;

/// The options the engine declared before any of these existed. A tunable spelled like one of
/// them would be answered by the wrong handler.
const FIXED_OPTIONS: [&str; 5] = ["UCI_Chess960", "Hash", "Threads", "MultiPV", "Ponder"];

fn param(which: Tunable) -> &'static Param {
    &PARAMS[which as usize]
}

/// `cadence spsa`'s stdout and exit code.
fn spsa(args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(env!("CARGO_BIN_EXE_cadence"))
        .arg("spsa")
        .args(args)
        .output()
        .expect("run cadence spsa");
    (
        String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        out.status.code(),
    )
}

/// The `option` lines of a `uci` reply, by name, as the protocol's name and the rest of the
/// line's tokens.
fn declared() -> Vec<(String, Vec<String>)> {
    let out = talk("uci\nquit\n");
    let uciok = out.lines().position(|l| l == "uciok").expect("uciok");
    out.lines()
        .take(uciok)
        .filter_map(|l| l.strip_prefix("option name "))
        .map(|rest| {
            let toks: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
            let at = toks.iter().position(|t| t == "type").expect("a type");
            (toks[..at].join(" "), toks[at..].to_vec())
        })
        .collect()
}

/// Two readings of one decimal as the same number, which is the question and not bitwise
/// equality.
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

/// The value after `key` in a declaration's tokens.
fn token<'a>(toks: &'a [String], key: &str) -> Option<&'a str> {
    let at = toks.iter().position(|t| t == key)?;
    toks.get(at + 1).map(String::as_str)
}

/// The `nodes` of the last iteration line of a fixed-depth search from [`MIDDLEGAME`], after
/// `setup`.
fn nodes_after(setup: &[&str]) -> u64 {
    let position = format!("position fen {MIDDLEGAME}");
    let mut lines: Vec<&str> = setup.to_vec();
    lines.push(&position);
    let out = Engine::go(&lines, &format!("go depth {DEPTH}"));
    let prefix = format!("info depth {DEPTH} ");
    let line = out
        .iter()
        .rfind(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no `{prefix}` line after {setup:?}: {out:?}"));
    let toks: Vec<&str> = line.split_whitespace().collect();
    let at = toks.iter().position(|t| *t == "nodes").expect("nodes");
    toks[at + 1].parse().expect("a node count")
}

// ---------------------------------------------------------------------------
// The declaration, and the one fact it shares with the tune input
// ---------------------------------------------------------------------------

/// Every parameter is declared before `uciok`, exactly as its row renders it.
#[test]
fn every_parameter_is_declared_before_uciok() {
    let out = talk("uci\nquit\n");
    let uciok = out.lines().position(|l| l == "uciok").expect("uciok");
    for p in PARAMS {
        let at = out
            .lines()
            .position(|l| l == p.uci_option())
            .unwrap_or_else(|| panic!("no `{}` in {out:?}", p.uci_option()));
        assert!(at < uciok, "{} is declared after uciok", p.name);
    }
}

/// The names a tuner will send: no whitespace, because a tuner's option string is split on it;
/// no longer than a tuner stores; distinct without regard to case, and distinct from every
/// option declared before them.
#[test]
fn every_name_survives_the_trip_through_a_tuner() {
    for (i, p) in PARAMS.iter().enumerate() {
        assert!(!p.name.is_empty() && p.name.len() <= 64, "{}", p.name);
        assert!(
            p.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "`{}` is not lowercase ASCII, digits and underscores",
            p.name
        );
        for q in &PARAMS[i + 1..] {
            assert!(!p.name.eq_ignore_ascii_case(q.name), "{} twice", p.name);
        }
        for fixed in FIXED_OPTIONS {
            assert!(
                !p.name.eq_ignore_ascii_case(fixed),
                "{} shadows {fixed}",
                p.name
            );
        }
        assert_eq!(
            tune::find(&p.name.to_ascii_uppercase()).map(|f| f.name),
            Some(p.name),
            "UCI names are matched without regard to case"
        );
    }
}

/// **The drift gate.** The block `cadence spsa` prints and the options `uci` declares are
/// parsed separately, the way a tuner and a GUI would read them, and have to agree on every
/// name, kind, default and bound.
///
/// Both are rendered from one table today, so this passes by construction; it is what fails if
/// either rendering stops reading the table, which is the one way two copies of a fact appear.
#[test]
fn the_spsa_block_and_the_uci_declaration_agree() {
    let (block, code) = spsa(&[]);
    assert_eq!(code, Some(0));
    let options = declared();
    let lines: Vec<&str> = block.lines().collect();
    assert_eq!(
        lines.len(),
        PARAMS.len(),
        "one line per parameter: {block:?}"
    );
    assert!(
        block.ends_with('\n') && !block.ends_with("\n\n"),
        "a blank line in the block is a parameter the tuner cannot parse: {block:?}"
    );
    for line in lines {
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        assert_eq!(fields.len(), 7, "seven fields in `{line}`");
        let [name, kind, current, min, max, c_end, r_end] = fields[..] else {
            unreachable!()
        };
        let number = |s: &str| -> f64 {
            s.parse()
                .unwrap_or_else(|_| panic!("`{s}` in `{line}` is not a number"))
        };
        let (current, min, max) = (number(current), number(min), number(max));
        assert!(
            min <= current && current <= max,
            "`{line}` starts outside its range"
        );
        assert!(number(c_end) > 0.0 && number(r_end) > 0.0, "`{line}`");

        let (_, toks) = options
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("`{name}` is in the block and not declared: {options:?}"));
        let declared_default: f64 = token(toks, "default")
            .expect("a default")
            .parse()
            .expect("a numeric default");
        assert!(
            same(declared_default, current),
            "{name}: the two defaults differ"
        );
        match (kind, token(toks, "type")) {
            ("int", Some("spin")) => {
                let bound = |key| -> f64 { token(toks, key).expect(key).parse().expect(key) };
                assert!(same(bound("min"), min), "{name}: the two minimums differ");
                assert!(same(bound("max"), max), "{name}: the two maximums differ");
            }
            ("float", Some("string")) => {}
            (kind, declared) => panic!("{name}: `{kind}` in the block, {declared:?} declared"),
        }
    }
}

/// The subcommand takes no arguments, for `bench`'s reason: what it prints is fixed in the
/// source.
#[test]
fn the_spsa_subcommand_refuses_arguments() {
    let (out, code) = spsa(&["--iterations", "2000"]);
    assert_eq!(code, Some(2));
    assert!(out.is_empty(), "printed a block anyway: {out:?}");
}

// ---------------------------------------------------------------------------
// What a value does on the way in
// ---------------------------------------------------------------------------

/// A float takes every spelling a tuner's float formatting produces, exponents included,
/// and a value outside the range is clamped into it rather than refused.
#[test]
fn a_float_takes_every_spelling_a_tuner_sends() {
    let p = param(Tunable::LmpDivisor);
    assert_eq!(p.kind, Kind::Float);
    for (text, stored) in [
        ("2.5", 2500),
        ("2.5000000000000004", 2500),
        ("2.4999999999999996", 2500),
        ("3e0", 3000),
        ("3.5E+00", 3500),
        ("+2.25", 2250),
        (" 2.0 ", 2000),
        ("2", 2000),
        ("2.0005", 2001),
        ("0.5", 1000),
        ("-3", 1000),
        ("1e-05", 1000),
        ("7", 6000),
        ("1e300", 6000),
    ] {
        assert_eq!(p.parse(text), Some(stored), "`{text}`");
    }
    for stored in p.min..=p.max {
        assert_eq!(
            p.parse(&p.spell(stored)),
            Some(stored),
            "{stored} round trip"
        );
    }
}

/// An integer takes an integer and nothing else, which is what a tuner sends for one.
#[test]
fn an_integer_takes_an_integer() {
    let p = param(Tunable::FutilityMargin);
    assert_eq!(p.kind, Kind::Int);
    assert_eq!(p.parse("150"), Some(150));
    assert_eq!(p.parse(" 151 "), Some(151));
    assert_eq!(p.parse("+120"), Some(120));
    assert_eq!(p.parse("-5"), Some(p.min));
    assert_eq!(p.parse("100000"), Some(p.max));
    for bad in ["150.0", "1e2", "0x96", "15O", "", "one fifty"] {
        assert_eq!(p.parse(bad), None, "`{bad}`");
    }
}

/// **A malformed value is refused, said so, and changes nothing.** The session names the value
/// and what it kept, answers the next command, and searches exactly the tree it searched before.
///
/// This is the no-panic half of the float path made observable: every string here is one that
/// either fails to parse or parses to something that is not a finite number.
#[test]
fn a_malformed_value_is_refused_and_the_search_is_untouched() {
    let untouched = nodes_after(&[]);
    for p in PARAMS {
        let bad: &[&str] = match p.kind {
            Kind::Float => &[
                "", "abc", "nan", "NaN", "inf", "-inf", "infinity", "1e999", "2.0.0", "2,5", "0x2",
                "--2", "2 .5",
            ],
            Kind::Int => &["", "abc", "150.0", "1e2", "0x96", "15O", "1 50"],
        };
        let setup: Vec<String> = bad
            .iter()
            .map(|v| format!("setoption name {} value {v}", p.name))
            .collect();
        let script = format!("{}\nisready\nquit\n", setup.join("\n"));
        let out = talk(&script);
        assert!(out.lines().any(|l| l == "readyok"), "{}: {out:?}", p.name);
        let refusals = out
            .lines()
            .filter(|l| l.starts_with(&format!("info string setoption {}: ", p.name)))
            .count();
        assert_eq!(
            refusals,
            bad.len(),
            "{}: one refusal per value in {out:?}",
            p.name
        );
        let kept = format!("keeping {}", p.spell(p.default));
        assert!(
            out.lines()
                .filter(|l| l.contains(" is not a number, "))
                .all(|l| l.ends_with(&kept)),
            "{}: a refusal that did not keep the default: {out:?}",
            p.name
        );

        let setup: Vec<&str> = setup.iter().map(String::as_str).collect();
        assert_eq!(
            nodes_after(&setup),
            untouched,
            "{}: a refused value reached the search",
            p.name
        );
    }
}

// ---------------------------------------------------------------------------
// What a value does to the search
// ---------------------------------------------------------------------------

/// **Every option changes the search**, at each end of its range, through the pipe a tuner
/// uses. An option that is declared and accepted and reaches nothing is the failure a tune
/// cannot report, because it runs to its end and moves nothing.
#[test]
fn every_option_changes_the_search_at_both_ends_of_its_range() {
    let untouched = nodes_after(&[]);
    for p in PARAMS {
        for end in [p.min, p.max] {
            let set = format!("setoption name {} value {}", p.name, p.spell(end));
            let nodes = nodes_after(&[&set]);
            assert_ne!(nodes, untouched, "`{set}` searched the default tree");
        }
    }
}

/// Setting every option to its default is the same search as setting none, line for line: the
/// point a tune starts from is the engine the tree already holds.
#[test]
fn every_option_at_its_default_is_the_untouched_search() {
    let position = format!("position fen {MIDDLEGAME}");
    let go = format!("go depth {DEPTH}");
    let untouched = Engine::go(&[&position], &go);
    let sets: Vec<String> = PARAMS
        .iter()
        .map(|p| format!("setoption name {} value {}", p.name, p.spell(p.default)))
        .collect();
    let mut setup: Vec<&str> = sets.iter().map(String::as_str).collect();
    setup.push(&position);
    let asked = Engine::go(&setup, &go);
    // Everything on an iteration line but the two wall-clock readings.
    let strip = |out: &[String]| -> Vec<String> {
        out.iter()
            .filter(|l| l.starts_with("info depth "))
            .map(|l| {
                let t: Vec<&str> = l.split_whitespace().collect();
                let clocked = |i: usize| {
                    ["nps", "time"].contains(&t[i])
                        || (i > 0 && ["nps", "time"].contains(&t[i - 1]))
                };
                (0..t.len())
                    .filter(|&i| !clocked(i))
                    .map(|i| t[i])
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    };
    assert_eq!(strip(&untouched).len(), DEPTH as usize);
    assert_eq!(strip(&untouched), strip(&asked));
}

/// The divisor is held in thousandths, and at its default it divides exactly as the integer it
/// replaced did, at every depth and not only the ones the rule reads.
#[test]
fn the_default_divisor_is_the_integer_it_replaced() {
    for depth in 0..=u32::from(u8::MAX) {
        assert_eq!(
            lmp_count(&Tunables::DEFAULT, depth),
            REDUCTION_INDEX + (depth * depth / 2) as usize,
            "depth {depth}"
        );
    }
}

/// **A setting cannot reach `bench`.** Every option is moved to an end of its range in a live
/// session, and `bench` in the same process still reads the count `bench.txt` declares.
///
/// It passes today because each search starts from the compiled-in values and only a session
/// hands it others; it is what fails if a tunable is ever made process-wide.
#[test]
fn a_setting_in_a_session_cannot_reach_bench() {
    let mut session = Session::new();
    for p in PARAMS {
        session.handle_line(&format!(
            "setoption name {} value {}",
            p.name,
            p.spell(p.max)
        ));
        assert_eq!(session.tunables().get(p.tunable), p.max, "{}", p.name);
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../bench.txt");
    let declared: u64 = std::fs::read_to_string(path)
        .expect("bench.txt")
        .trim()
        .parse()
        .expect("a count");
    assert_eq!(cadence_engine::bench::bench().nodes, declared);
    // And the session's values are the ones its own searches read, which is the other half.
    let stop = std::sync::atomic::AtomicBool::new(false);
    let tt = support::table();
    let mut moved = support::search(Limits::depth(DEPTH), &stop, &tt);
    moved.set_tunables(*session.tunables());
    let mut pos = support::position(MIDDLEGAME);
    let _ = moved.run(&mut pos, &mut std::io::sink());
    tt.clear();
    let mut plain = support::search(Limits::depth(DEPTH), &stop, &tt);
    let mut pos = support::position(MIDDLEGAME);
    let _ = plain.run(&mut pos, &mut std::io::sink());
    assert_ne!(moved.nodes(), plain.nodes());
}
