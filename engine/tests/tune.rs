// SPDX-License-Identifier: GPL-3.0-or-later

//! A misspelled name, an unparsed float or an option reaching nothing all fail in silence, as a
//! tune that moves nothing. So both printed forms are parsed and compared, malformed spellings
//! shown refused, every option shown to change a net's search through the tuner's pipe, and none to
//! reach the handcrafted evaluation's.

mod support;

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use cadence_engine::position::Position;
use cadence_engine::search::{Limits, REDUCTION_INDEX, lmp_count};
use cadence_engine::tune::{self, Kind, PARAMS, Param, Tunable, Tunables};
use cadence_engine::uci::Session;
use cadence_nnue::{Network, Provenance};
use support::{Engine, talk};

/// Every rule the table reaches has nodes to act on at `DEPTH`.
const MIDDLEGAME: &str = "2rq1rk1/pb2bppp/1pn1pn2/8/2BP4/2N1PN2/PPQ2PPP/2R2RK1 w - - 4 14";

/// Deep enough that each rule fires many times over, shallow enough that a subprocess per
/// setting stays cheap.
const DEPTH: u32 = 8;

/// A net's search at `DEPTH` costs a subprocess many times the handcrafted one's.
const NET_DEPTH: u32 = 6;

/// One past the deepest node the singular question is asked at by default, which `NET_DEPTH` is
/// too shallow to reach.
const SINGULAR_NET_DEPTH: u32 = 9;

/// Where `p` acts on a net's search.
fn net_depth(p: &Param) -> u32 {
    match p.tunable {
        Tunable::SingularDepth | Tunable::SingularMargin => SINGULAR_NET_DEPTH,
        _ => NET_DEPTH,
    }
}

/// A tunable spelled like one of these would be answered by the wrong handler.
const FIXED_OPTIONS: [&str; 5] = ["UCI_Chess960", "Hash", "Threads", "MultiPV", "Ponder"];

fn param(which: Tunable) -> &'static Param {
    &PARAMS[which as usize]
}

fn spsa(args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(support::binary())
        .arg("spsa")
        .args(args)
        .output()
        .expect("run cadence spsa");
    (
        String::from_utf8(out.stdout).expect("stdout is UTF-8"),
        out.status.code(),
    )
}

/// By name, as the protocol's name and the rest of the line's tokens.
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

/// The question is the number, not bitwise equality.
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9
}

fn token<'a>(toks: &'a [String], key: &str) -> Option<&'a str> {
    let at = toks.iter().position(|t| t == key)?;
    toks.get(at + 1).map(String::as_str)
}

/// A seeded net, written once for every test here, and the two lines that make it score.
fn net_options() -> [String; 2] {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    let path = PATH.get_or_init(|| {
        let path =
            std::env::temp_dir().join(format!("cadence-tune-net-{}.cdnn", std::process::id()));
        let bytes = Network::seeded(7, Provenance::SelfPlay).to_bytes();
        std::fs::write(&path, bytes).expect("write the net file");
        path
    });
    [
        format!("setoption name EvalFile value {}", path.display()),
        "setoption name Use NNUE value true".to_string(),
    ]
}

/// Of the handcrafted evaluation's search from `MIDDLEGAME` at `DEPTH`, after `setup`; switched to
/// explicitly, since a build that ships a net scores with it by default.
fn nodes_after(setup: &[&str]) -> u64 {
    let mut lines = vec!["setoption name Use NNUE value false"];
    lines.extend_from_slice(setup);
    nodes_at(&lines, DEPTH)
}

/// The same with the seeded net scoring, at `NET_DEPTH`.
fn net_nodes_after(setup: &[&str]) -> u64 {
    net_nodes_at(setup, NET_DEPTH)
}

fn net_nodes_at(setup: &[&str], depth: u32) -> u64 {
    let net = net_options();
    let mut lines: Vec<&str> = net.iter().map(String::as_str).collect();
    lines.extend_from_slice(setup);
    nodes_at(&lines, depth)
}

/// Of the last iteration line at `depth`.
fn nodes_at(setup: &[&str], depth: u32) -> u64 {
    let position = format!("position fen {MIDDLEGAME}");
    let mut lines: Vec<&str> = setup.to_vec();
    lines.push(&position);
    let out = Engine::go(&lines, &format!("go depth {depth}"));
    let prefix = format!("info depth {depth} ");
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

/// No whitespace, since a tuner splits on it; no longer than a tuner stores; distinct without
/// regard to case and from every earlier option.
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

/// Parsed separately, as a tuner and a GUI would read them. Both render from one table, so this
/// fails only if a rendering stops reading it.
#[test]
fn the_spsa_block_and_the_uci_declaration_agree() {
    let (block, code) = spsa(&[]);
    assert_eq!(code, Some(0));
    let options = declared();
    // Split exactly as the tuner's form validator splits, on the newline alone, so that a final
    // newline shows up as the empty piece that validator refuses as a malformed parameter.
    let lines: Vec<&str> = block.split('\n').collect();
    assert_eq!(
        lines.len(),
        PARAMS.len(),
        "one piece per parameter and no empty piece, a final newline included: {block:?}"
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

/// For `bench`'s reason: what it prints is fixed in the source.
#[test]
fn the_spsa_subcommand_refuses_arguments() {
    let (out, code) = spsa(&["--iterations", "2000"]);
    assert_eq!(code, Some(2));
    assert!(out.is_empty(), "printed a block anyway: {out:?}");
}

/// A float's `type string` declaration carries no bounds, so the drift gate has nothing to compare;
/// a block edited by hand is clamped by the engine in silence, which nothing here can see.
#[test]
fn a_floats_bounds_are_in_the_block_because_its_declaration_has_none() {
    for p in PARAMS.iter().filter(|p| p.kind == Kind::Float) {
        let declared = p.uci_option();
        let toks: Vec<&str> = declared.split_whitespace().collect();
        assert!(
            !toks.contains(&"min") && !toks.contains(&"max"),
            "a string option cannot declare bounds: {declared}"
        );
        let line = p.spsa_line();
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        assert_eq!(
            fields[3],
            p.spell(p.min),
            "the block's minimum is the table's: {line}"
        );
        assert_eq!(
            fields[4],
            p.spell(p.max),
            "the block's maximum is the table's: {line}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a value does on the way in
// ---------------------------------------------------------------------------

/// A value outside the range is clamped rather than refused.
#[test]
fn a_float_takes_every_spelling_a_tuner_sends() {
    let p = param(Tunable::LmpMultiplier);
    assert_eq!(p.kind, Kind::Float);
    for (text, stored) in [
        ("0.5", 500),
        ("0.5000000000000004", 500),
        ("0.4999999999999996", 500),
        ("1e0", 1000),
        ("1.5E+00", 1500),
        ("+0.25", 250),
        (" 0.5 ", 500),
        ("1", 1000),
        ("0.5006", 501),
        ("0", 0),
        ("-3", 0),
        ("1e-05", 0),
        ("3", 2000),
        ("1e300", 2000),
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

#[test]
fn an_integer_takes_an_integer() {
    let p = param(Tunable::ReverseFutilityMargin);
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

/// The session names the value and what it kept, answers the next command, and searches the same
/// tree.
#[test]
fn a_malformed_value_is_refused_and_the_search_is_untouched() {
    let untouched = net_nodes_after(&[]);
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
            net_nodes_after(&setup),
            untouched,
            "{}: a refused value reached the search",
            p.name
        );
    }
}

// ---------------------------------------------------------------------------
// What a value does to the search
// ---------------------------------------------------------------------------

/// A declared, accepted option that reaches nothing is the failure a tune cannot report.
#[test]
fn a_nets_search_reads_every_option_at_both_ends_of_its_range() {
    let untouched = |depth| net_nodes_at(&[], depth);
    let (shallow, deep) = (untouched(NET_DEPTH), untouched(SINGULAR_NET_DEPTH));
    for p in PARAMS {
        let untouched = if net_depth(p) == NET_DEPTH {
            shallow
        } else {
            deep
        };
        for end in [p.min, p.max] {
            let set = format!("setoption name {} value {}", p.name, p.spell(end));
            let nodes = net_nodes_at(&[&set], net_depth(p));
            assert_ne!(nodes, untouched, "`{set}` searched the default tree");
        }
    }
}

/// The handcrafted evaluation searches with its own set, frozen, so no tune reaches it.
#[test]
fn the_handcrafted_search_reads_no_option_at_either_end_of_its_range() {
    let untouched = nodes_after(&[]);
    for p in PARAMS {
        for end in [p.min, p.max] {
            let set = format!("setoption name {} value {}", p.name, p.spell(end));
            let nodes = nodes_after(&[&set]);
            assert_eq!(nodes, untouched, "`{set}` moved the handcrafted search");
        }
    }
}

/// The point a tune starts from is the engine the tree already holds.
#[test]
fn every_option_at_its_default_is_the_untouched_search() {
    let net = net_options();
    let position = format!("position fen {MIDDLEGAME}");
    let go = format!("go depth {NET_DEPTH}");
    let mut plain: Vec<&str> = net.iter().map(String::as_str).collect();
    plain.push(&position);
    let untouched = Engine::go(&plain, &go);
    let sets: Vec<String> = PARAMS
        .iter()
        .map(|p| format!("setoption name {} value {}", p.name, p.spell(p.default)))
        .collect();
    let mut setup: Vec<&str> = net.iter().map(String::as_str).collect();
    setup.extend(sets.iter().map(String::as_str));
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
    let (last, lines) = (format!("info depth {NET_DEPTH} "), strip(&untouched));
    assert!(
        lines.last().is_some_and(|l| l.starts_with(&last)),
        "{untouched:?}"
    );
    assert_eq!(strip(&untouched), strip(&asked));
}

/// `3 + d * d * 500 / 1000` is `3 + d * d / 2` at every depth, so the tree cannot have moved.
#[test]
fn the_default_multiplier_is_the_divisor_it_replaced() {
    for depth in 0..=u32::from(u8::MAX) {
        assert_eq!(
            lmp_count(&Tunables::DEFAULT, depth),
            REDUCTION_INDEX + (depth * depth / 2) as usize,
            "depth {depth}"
        );
    }
}

/// Each search starts from the compiled-in values and only a session hands it others; this fails if
/// a tunable is made process-wide.
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
    // And the session's values are the ones a net's search reads, which is the other half.
    let net = Arc::new(Network::seeded(7, Provenance::SelfPlay));
    let netted = || Position::with_net(support::position(MIDDLEGAME).into_board(), net.clone());
    let stop = AtomicBool::new(false);
    let tt = support::table();
    let mut moved = support::search(Limits::depth(NET_DEPTH), &stop, &tt);
    moved.set_tunables(*session.tunables());
    let _ = moved.run(&mut netted(), &mut std::io::sink());
    tt.clear();
    let mut plain = support::search(Limits::depth(NET_DEPTH), &stop, &tt);
    let _ = plain.run(&mut netted(), &mut std::io::sink());
    assert_ne!(moved.nodes(), plain.nodes());
}

/// The handcrafted evaluation's set, which a change to a net's defaults must leave where it is.
#[test]
fn the_handcrafted_set_holds_its_frozen_values() {
    assert_eq!(Tunables::HCE.get(Tunable::ReverseFutilityMargin), 150);
    assert_eq!(Tunables::HCE.get(Tunable::LmpMultiplier), 500);
    assert_eq!(Tunables::HCE.get(Tunable::SingularDepth), 0);
    assert_eq!(Tunables::HCE.get(Tunable::SingularMargin), 2);
}

/// Bench, datagen and every `go` build their searches apart, so the frozen set is held where the
/// search starts and not by each caller.
#[test]
fn a_handcrafted_search_reads_its_frozen_set_whatever_it_is_given() {
    let mut extreme = Tunables::DEFAULT;
    for p in PARAMS {
        p.set(&mut extreme, p.max);
    }
    let stop = AtomicBool::new(false);
    let tt = support::table();
    let mut given = support::search(Limits::depth(DEPTH), &stop, &tt);
    given.set_tunables(extreme);
    let _ = given.run(&mut support::position(MIDDLEGAME), &mut std::io::sink());
    tt.clear();
    let mut plain = support::search(Limits::depth(DEPTH), &stop, &tt);
    let _ = plain.run(&mut support::position(MIDDLEGAME), &mut std::io::sink());
    assert_eq!(given.nodes(), plain.nodes());
}
