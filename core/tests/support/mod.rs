// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads `tests/fixtures/perft-corpus.txt` and hands back typed cases.
//!
//! The corpus fixture is the single source of every expected value in these
//! tests. Nothing here transcribes a node count, a FEN or a move list into
//! Rust: the only literals in the test files are *selectors* (a position's
//! name, a Scharnagl index pair, a distinctive phrase from a row's `reason`
//! column) which pick a row out of the fixture. That split is deliberate: a
//! selector that stops matching is a loud failure, whereas a transcribed
//! number that drifts from the fixture is a silent one.
//!
//! The fixture is embedded with `include_str!`, so editing it forces a
//! rebuild and the tests cannot be run against a stale or missing copy.

// Each test binary in this directory uses a different subset of this module.
#![allow(dead_code)]

pub mod generative;
pub mod naive;

use cadence_core::mv::Move;
use cadence_core::position::Board;
use cadence_core::{generate_legal, perft};
use std::collections::BTreeMap;

/// The corpus fixture, embedded at compile time.
pub const FIXTURE: &str = include_str!("../../../tests/fixtures/perft-corpus.txt");

// ---------------------------------------------------------------------------
// Fixture structure
// ---------------------------------------------------------------------------

/// A fenced code block. Blocks are addressed by the **name** in their info
/// string (```` ```tsv edge-cases ````), never by the section they sit under.
///
/// Naming them is what lets a section hold more than one block, and it removes
/// the coupling between a test and a heading number: editing the explanatory
/// document cannot silently repoint a test at different data.
fn blocks() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut fence: Option<(String, Vec<String>)> = None;

    for line in FIXTURE.lines() {
        if let Some((info, body)) = fence.as_mut() {
            if line.trim_start().starts_with("```") {
                out.push((info.clone(), body.join("\n")));
                fence = None;
            } else {
                body.push(line.to_string());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("```") {
            fence = Some((rest.trim().to_string(), Vec::new()));
        }
    }
    assert!(fence.is_none(), "corpus has an unterminated code fence");
    out
}

/// The body of the one block named `name`.
fn block(name: &str) -> String {
    let all = blocks();
    let mut hits: Vec<&(String, String)> = all
        .iter()
        .filter(|(info, _)| info.split_whitespace().nth(1) == Some(name))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected exactly one block named `{name}`, found {}; the corpus has {:?}",
        hits.len(),
        all.iter().map(|(i, _)| i.clone()).collect::<Vec<_>>()
    );
    hits.pop().expect("checked above").1.clone()
}

/// Every block name in the fixture, for the integrity tests.
pub fn block_names() -> Vec<String> {
    blocks()
        .into_iter()
        .filter_map(|(info, _)| info.split_whitespace().nth(1).map(str::to_string))
        .collect()
}

/// The named block, split into tab-separated fields.
fn tsv(name: &str) -> Vec<Vec<String>> {
    block(name)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split('\t').map(|f| f.trim().to_string()).collect())
        .collect()
}

fn number(field: &str) -> u64 {
    field
        .replace(',', "")
        .parse()
        .unwrap_or_else(|e| panic!("`{field}` is not a number: {e}"))
}

// ---------------------------------------------------------------------------
// Section 1: standard perft suite
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct StandardPosition {
    pub name: String,
    pub fen: String,
    /// `(depth, nodes)`, ascending.
    pub nodes: Vec<(u32, u64)>,
}

/// The `| # | Position | FEN |` table in section 1, keyed by lowercased name.
///
/// Lowercased because the fixture retains the display table's title case:
/// the tables say `Kiwipete` and the TSV block says `kiwipete`.
fn standard_fens() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in FIXTURE.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() != 3 {
            continue;
        }
        // A data row is `| <index> | <name> | `<fen>` |`.
        if cells[0].parse::<u32>().is_err() {
            continue;
        }
        let Some(fen) = cells[2].strip_prefix('`').and_then(|f| f.strip_suffix('`')) else {
            continue;
        };
        out.insert(cells[1].to_ascii_lowercase(), fen.to_string());
    }
    out
}

/// The `| Position | d1 | ... | d7 |` table in section 1, as `(name, depth, nodes)`.
///
/// This is the readable summary table, which restates the TSV block's numbers
/// with thousands separators. It is parsed only so the two serializations can
/// be checked against each other; the TSV block is what the tests assert with.
pub fn standard_summary_table() -> Vec<(String, u32, u64)> {
    let mut out = Vec::new();
    for line in FIXTURE.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 7 || cells.len() > 8 {
            continue;
        }
        let name = cells[0].to_ascii_lowercase();
        if !standard_fens().contains_key(&name) {
            continue;
        }
        for (i, cell) in cells[1..].iter().enumerate() {
            // An empty cell is a depth this corpus records no value for.
            if cell.is_empty() {
                continue;
            }
            out.push((
                name.clone(),
                u32::try_from(i).expect("depth fits") + 1,
                number(cell),
            ));
        }
    }
    out
}

pub fn standard_positions() -> Vec<StandardPosition> {
    let fens = standard_fens();
    let mut by_name: BTreeMap<String, Vec<(u32, u64)>> = BTreeMap::new();
    for row in tsv("standard-perft") {
        assert_eq!(
            row.len(),
            3,
            "Section 1 row is not `name<TAB>depth<TAB>nodes`: {row:?}"
        );
        let depth = u32::try_from(number(&row[1])).expect("depth fits in u32");
        by_name
            .entry(row[0].to_ascii_lowercase())
            .or_default()
            .push((depth, number(&row[2])));
    }
    by_name
        .into_iter()
        .map(|(name, mut nodes)| {
            nodes.sort_unstable();
            let fen = fens
                .get(&name)
                .unwrap_or_else(|| panic!("Section 1 has perft rows for `{name}` but no FEN"))
                .clone();
            StandardPosition { name, fen, nodes }
        })
        .collect()
}

pub fn standard(name: &str) -> StandardPosition {
    let key = name.to_ascii_lowercase();
    standard_positions()
        .into_iter()
        .find(|p| p.name == key)
        .unwrap_or_else(|| panic!("Section 1 has no position named `{name}`"))
}

// ---------------------------------------------------------------------------
// Section 2: DFRC start arrays
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DfrcArray {
    /// Scharnagl index of White's back rank.
    pub wid: u32,
    /// Scharnagl index of Black's back rank.
    pub bid: u32,
    pub fen: String,
    pub nodes: Vec<(u32, u64)>,
}

pub fn dfrc_arrays() -> Vec<DfrcArray> {
    tsv("dfrc-arrays")
        .into_iter()
        .map(|row| {
            assert_eq!(
                row.len(),
                8,
                "Section 2 row is not wid/bid/fen/d1..d5: {row:?}"
            );
            DfrcArray {
                wid: u32::try_from(number(&row[0])).expect("index fits"),
                bid: u32::try_from(number(&row[1])).expect("index fits"),
                fen: row[2].clone(),
                nodes: (1..=5).map(|d| (d, number(&row[2 + d as usize]))).collect(),
            }
        })
        .collect()
}

pub fn dfrc(wid: u32, bid: u32) -> DfrcArray {
    dfrc_arrays()
        .into_iter()
        .find(|a| a.wid == wid && a.bid == bid)
        .unwrap_or_else(|| panic!("Section 2 has no array {wid}/{bid}"))
}

// ---------------------------------------------------------------------------
// Section 3: DFRC castling legality
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CastlingCase {
    /// Whether *any* castling move is legal in this position.
    pub legal: bool,
    /// The castling move in king-takes-rook notation, when one is legal.
    pub castling: Option<String>,
    pub fen: String,
    pub nodes: Vec<(u32, u64)>,
    pub reason: String,
}

pub fn castling_cases() -> Vec<CastlingCase> {
    tsv("castling-legality")
        .into_iter()
        .map(|row| {
            assert_eq!(
                row.len(),
                7,
                "Section 3 row is not verdict/castling/fen/d1..d3/reason: {row:?}"
            );
            let legal = match row[0].as_str() {
                "legal" => true,
                "illegal" => false,
                other => panic!("Section 3 verdict is neither `legal` nor `illegal`: `{other}`"),
            };
            let castling = match row[1].as_str() {
                "NONE" => None,
                m => Some(m.to_string()),
            };
            assert_eq!(
                legal,
                castling.is_some(),
                "Section 3 verdict and castling column disagree: {row:?}"
            );
            CastlingCase {
                legal,
                castling,
                fen: row[2].clone(),
                nodes: (1..=3).map(|d| (d, number(&row[2 + d as usize]))).collect(),
                reason: row[6].clone(),
            }
        })
        .collect()
}

/// Side to move, parsed from the FEN.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Stm {
    White,
    Black,
}

impl Stm {
    /// The side to move in a case's FEN.
    #[must_use]
    pub fn of_case(case: &CastlingCase) -> Self {
        Self::of(&case.fen)
    }

    fn of(fen: &str) -> Self {
        match fen.split_whitespace().nth(1) {
            Some("w") => Stm::White,
            Some("b") => Stm::Black,
            other => panic!("FEN has no side-to-move field: {other:?}\n  {fen}"),
        }
    }
}

/// The one castling case whose `reason` contains `keyword` and whose FEN has
/// `stm` to move.
///
/// Every rule in the castling block appears twice, once per colour, because a
/// rank-8 mirroring bug otherwise passes every White position and reports as a
/// wrong node count in an unrelated DFRC start array. The keyword selects the
/// *rule*; `stm` selects which mirror of it.
pub fn castling_case(keyword: &str, stm: Stm) -> CastlingCase {
    let mut hits: Vec<CastlingCase> = castling_cases()
        .into_iter()
        .filter(|c| c.reason.contains(keyword) && Stm::of(&c.fen) == stm)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "`{keyword}` + {stm:?} selects {} castling rows, expected exactly 1",
        hits.len()
    );
    hits.pop().expect("checked above")
}

/// Both mirrors of the rule `keyword` selects.
pub fn castling_pair(keyword: &str) -> Vec<CastlingCase> {
    castling_cases()
        .into_iter()
        .filter(|c| c.reason.contains(keyword))
        .collect()
}

/// Every `FEN:` / `legal:` pair in the block proving that king-to-destination
/// notation is ambiguous. One per colour.
pub fn ambiguity_proofs() -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut fen: Option<String> = None;
    for line in block("ambiguity-proof").lines() {
        if let Some(rest) = line.strip_prefix("FEN:") {
            fen = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("legal:") {
            let moves = rest.split_whitespace().map(str::to_string).collect();
            out.push((
                fen.take()
                    .expect("a `legal:` line without a preceding `FEN:`"),
                moves,
            ));
        }
    }
    assert!(
        !out.is_empty(),
        "the ambiguity-proof block has no FEN:/legal: pair"
    );
    out
}

// ---------------------------------------------------------------------------
// Section 4: check, evasion, promotion and en-passant edge cases
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct EdgeCase {
    pub fen: String,
    pub nodes: Vec<(u32, u64)>,
    pub reason: String,
}

pub fn edge_cases() -> Vec<EdgeCase> {
    tsv("edge-cases")
        .into_iter()
        .map(|row| {
            assert_eq!(
                row.len(),
                6,
                "Section 4 row is not fen/d1..d4/reason: {row:?}"
            );
            EdgeCase {
                fen: row[0].clone(),
                nodes: (1..=4).map(|d| (d, number(&row[d as usize]))).collect(),
                reason: row[5].clone(),
            }
        })
        .collect()
}

/// The one section 4 case whose `reason` contains `keyword`.
pub fn edge_case(keyword: &str) -> EdgeCase {
    let mut hits: Vec<EdgeCase> = edge_cases()
        .into_iter()
        .filter(|c| c.reason.contains(keyword))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "`{keyword}` selects {} section 4 rows, expected exactly 1",
        hits.len()
    );
    hits.pop().expect("checked above")
}

/// One entry of section 4's "Full expected move lists at depth 1" block.
#[derive(Clone, Debug)]
pub struct ExpectedMoves {
    pub fen: String,
    /// The count the fixture states in parentheses.
    pub stated_count: usize,
    pub moves: Vec<String>,
    /// The `--` commentary under the list, joined into one line.
    pub annotation: String,
}

impl ExpectedMoves {
    /// Moves the annotation claims give check.
    ///
    /// Parsed from the one shape the fixture actually uses:
    /// `<move> and <move> give check`.
    pub fn checking_moves(&self) -> Vec<String> {
        let Some(idx) = self.annotation.find("give check") else {
            return Vec::new();
        };
        self.annotation[..idx]
            .split_whitespace()
            .filter(|w| is_uci_move(w))
            .map(str::to_string)
            .collect()
    }

    /// The promotion moves in the list, by their trailing piece character.
    pub fn promotions(&self) -> Vec<String> {
        self.moves
            .iter()
            .filter(|m| m.len() == 5)
            .cloned()
            .collect()
    }
}

fn is_uci_move(w: &str) -> bool {
    let b = w.as_bytes();
    (b.len() == 4 || b.len() == 5)
        && (b'a'..=b'h').contains(&b[0])
        && (b'1'..=b'8').contains(&b[1])
        && (b'a'..=b'h').contains(&b[2])
        && (b'1'..=b'8').contains(&b[3])
        && (b.len() == 4 || matches!(b[4], b'q' | b'r' | b'b' | b'n'))
}

pub fn expected_move_lists() -> Vec<ExpectedMoves> {
    let mut out: Vec<ExpectedMoves> = Vec::new();
    for name in ["edge-case-moves", "move-capacity-moves"] {
        let body = block(name);
        let mut cur: Option<ExpectedMoves> = None;
        let mut in_annotation = false;
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if !line.starts_with(' ') {
                if let Some(done) = cur.take() {
                    out.push(done);
                }
                if line.contains(" w ") || line.contains(" b ") {
                    cur = Some(ExpectedMoves {
                        fen: line.trim().to_string(),
                        stated_count: 0,
                        moves: Vec::new(),
                        annotation: String::new(),
                    });
                    in_annotation = false;
                }
                continue;
            }
            let Some(entry) = cur.as_mut() else { continue };
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("--") {
                in_annotation = true;
                entry.annotation.push_str(rest.trim());
                entry.annotation.push(' ');
            } else if in_annotation {
                entry.annotation.push_str(trimmed);
                entry.annotation.push(' ');
            } else if let Some(rest) = trimmed.strip_prefix('(') {
                let (count, moves) = rest.split_once(')').expect("a move-list line opens `(n)`");
                entry.stated_count = count.trim().parse().expect("move count is a number");
                entry
                    .moves
                    .extend(moves.split_whitespace().map(str::to_string));
            } else {
                // A continuation line of a wrapped move list.
                entry
                    .moves
                    .extend(trimmed.split_whitespace().map(str::to_string));
            }
        }
        if let Some(done) = cur.take() {
            out.push(done);
        }
    }
    for e in &mut out {
        e.annotation = e.annotation.trim().to_string();
    }
    out
}

/// The expected depth-1 move list for `fen`.
pub fn expected_moves(fen: &str) -> ExpectedMoves {
    expected_move_lists()
        .into_iter()
        .find(|e| e.fen == fen)
        .unwrap_or_else(|| panic!("Section 4 has no expected move list for `{fen}`"))
}

// ---------------------------------------------------------------------------
// Immediate-castle claims, FEN notation, rights-by-capture, capacity, divide
// ---------------------------------------------------------------------------

/// A DFRC start array that can castle at the named side's first move.
#[derive(Clone, Debug)]
pub struct ImmediateCastle {
    pub wid: u32,
    pub bid: u32,
    pub stm: Stm,
    pub fen: String,
    /// The castling move, king-takes-rook.
    pub castling: String,
    pub nodes: Vec<(u32, u64)>,
}

pub fn immediate_castles() -> Vec<ImmediateCastle> {
    tsv("immediate-castles")
        .into_iter()
        .map(|r| {
            assert_eq!(
                r.len(),
                9,
                "immediate-castles row is not wid/bid/colour/fen/move/d1..d4: {r:?}"
            );
            ImmediateCastle {
                wid: u32::try_from(number(&r[0])).expect("index fits"),
                bid: u32::try_from(number(&r[1])).expect("index fits"),
                stm: match r[2].as_str() {
                    "w" => Stm::White,
                    "b" => Stm::Black,
                    o => panic!("colour column is `{o}`, expected w or b"),
                },
                fen: r[3].clone(),
                castling: r[4].clone(),
                nodes: (1..=4).map(|d| (d, number(&r[4 + d as usize]))).collect(),
            }
        })
        .collect()
}

pub fn immediate_castle(wid: u32, bid: u32, stm: Stm) -> ImmediateCastle {
    immediate_castles()
        .into_iter()
        .find(|c| c.wid == wid && c.bid == bid && c.stm == stm)
        .unwrap_or_else(|| panic!("no immediate-castle row for {wid}/{bid} {stm:?}"))
}

/// The same position spelled two ways.
#[derive(Clone, Debug)]
pub struct FenNotation {
    pub shredder: String,
    pub xfen: String,
    pub nodes: Vec<(u32, u64)>,
    pub reason: String,
}

pub fn fen_notations() -> Vec<FenNotation> {
    tsv("fen-notation")
        .into_iter()
        .map(|r| {
            assert_eq!(
                r.len(),
                7,
                "fen-notation row is not shredder/xfen/d1..d4/reason: {r:?}"
            );
            FenNotation {
                shredder: r[0].clone(),
                xfen: r[1].clone(),
                nodes: (1..=4).map(|d| (d, number(&r[1 + d as usize]))).collect(),
                reason: r[6].clone(),
            }
        })
        .collect()
}

pub fn fen_notation(keyword: &str) -> FenNotation {
    let mut hits: Vec<FenNotation> = fen_notations()
        .into_iter()
        .filter(|f| f.reason.contains(keyword))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "`{keyword}` selects {} fen-notation rows",
        hits.len()
    );
    hits.pop().expect("checked above")
}

/// A capture that removes a castling right.
#[derive(Clone, Debug)]
pub struct RightsCapture {
    pub fen: String,
    /// King-takes-rook UCI.
    pub mv: String,
    /// The castling field, Shredder notation, before and after `mv`.
    pub before: String,
    pub after: String,
    pub nodes: Vec<(u32, u64)>,
    pub reason: String,
}

pub fn rights_captures() -> Vec<RightsCapture> {
    tsv("castling-rights-capture")
        .into_iter()
        .map(|r| {
            assert_eq!(
                r.len(),
                9,
                "rights row is not fen/move/before/after/d1..d4/reason: {r:?}"
            );
            RightsCapture {
                fen: r[0].clone(),
                mv: r[1].clone(),
                before: r[2].clone(),
                after: r[3].clone(),
                nodes: (1..=4).map(|d| (d, number(&r[3 + d as usize]))).collect(),
                reason: r[8].clone(),
            }
        })
        .collect()
}

pub fn rights_capture(keyword: &str) -> RightsCapture {
    let mut hits: Vec<RightsCapture> = rights_captures()
        .into_iter()
        .filter(|r| r.reason.contains(keyword))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "`{keyword}` selects {} rights rows",
        hits.len()
    );
    hits.pop().expect("checked above")
}

/// The 218-move position.
pub fn move_capacity() -> EdgeCase {
    let r = tsv("move-capacity");
    assert_eq!(r.len(), 1, "move-capacity should hold exactly one position");
    let r = &r[0];
    assert_eq!(
        r.len(),
        6,
        "move-capacity row is not fen/d1..d4/reason: {r:?}"
    );
    EdgeCase {
        fen: r[0].clone(),
        nodes: (1..=4).map(|d| (d, number(&r[d as usize]))).collect(),
        reason: r[5].clone(),
    }
}

/// One root move of a perft divide.
#[derive(Clone, Debug)]
pub struct DivideRow {
    pub name: String,
    pub depth: u32,
    pub mv: String,
    pub nodes: u64,
}

pub fn divides() -> Vec<DivideRow> {
    tsv("perft-divide")
        .into_iter()
        .map(|r| {
            assert_eq!(r.len(), 4, "divide row is not name/depth/move/nodes: {r:?}");
            DivideRow {
                name: r[0].clone(),
                depth: u32::try_from(number(&r[1])).expect("depth fits"),
                mv: r[2].clone(),
                nodes: number(&r[3]),
            }
        })
        .collect()
}

/// The divide for one position at one depth, sorted by move.
pub fn divide(name: &str, depth: u32) -> Vec<(String, u64)> {
    let mut rows: Vec<(String, u64)> = divides()
        .into_iter()
        .filter(|r| r.name == name && r.depth == depth)
        .map(|r| (r.mv, r.nodes))
        .collect();
    assert!(!rows.is_empty(), "no divide data for {name} d{depth}");
    rows.sort();
    rows
}

/// The geometry of an en-passant capture that resolves a check.
#[derive(Clone, Debug)]
pub struct EpEvasion {
    pub fen: String,
    /// The ep capture, king-takes-rook UCI (no castling involved).
    pub mv: String,
    /// The single checker's square.
    pub checker: String,
    /// `checker | BETWEEN[king][checker]`: the target set a naive evasion
    /// generator searches. The point is that `mv`'s destination is not in it.
    pub mask: Vec<String>,
    pub reason: String,
}

pub fn ep_evasions() -> Vec<EpEvasion> {
    tsv("ep-evasion")
        .into_iter()
        .map(|r| {
            assert_eq!(
                r.len(),
                5,
                "ep-evasion row is not fen/move/checker/mask/reason: {r:?}"
            );
            let mut mask: Vec<String> = r[3].split_whitespace().map(str::to_string).collect();
            mask.push(r[2].clone());
            mask.sort();
            mask.dedup();
            EpEvasion {
                fen: r[0].clone(),
                mv: r[1].clone(),
                checker: r[2].clone(),
                mask,
                reason: r[4].clone(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tier selection
// ---------------------------------------------------------------------------

/// The deepest perft the fast tier runs for a standard-suite position.
pub const FAST_STANDARD_MAX_DEPTH: u32 = 5;
/// The deepest perft the fast tier runs for a DFRC start array.
///
/// Five, not four. At four this is twenty near-duplicate tests of
/// non-castling movegen: most start arrays cannot reach a castle before depth
/// five, which is the trap the DFRC section of the corpus exists to warn
/// about. The whole set at d5 is ~98M nodes against the ~469M the standard
/// suite already contributes to this tier.
pub const FAST_DFRC_MAX_DEPTH: u32 = 5;

#[must_use]
pub fn upto(nodes: &[(u32, u64)], max_depth: u32) -> Vec<(u32, u64)> {
    nodes
        .iter()
        .copied()
        .filter(|(d, _)| *d <= max_depth)
        .collect()
}

#[must_use]
pub fn deeper_than(nodes: &[(u32, u64)], max_depth: u32) -> Vec<(u32, u64)> {
    nodes
        .iter()
        .copied()
        .filter(|(d, _)| *d > max_depth)
        .collect()
}

// ---------------------------------------------------------------------------
// Assertions against the engine
// ---------------------------------------------------------------------------

fn board(label: &str, fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{label}: FEN rejected ({e:?})\n  {fen}"))
}

/// Assert every `(depth, nodes)` row, reporting all mismatches rather than
/// stopping at the first. A single wrong depth is a different bug from a
/// whole position being wrong, and the shape of the failure says which.
pub fn assert_perft(label: &str, fen: &str, rows: &[(u32, u64)]) {
    assert!(
        !rows.is_empty(),
        "{label}: no corpus rows selected, so the tier filter is wrong"
    );
    let mut failures = Vec::new();
    for &(depth, expected) in rows {
        let mut b = board(label, fen);
        let got = perft(&mut b, depth);
        if got != expected {
            let delta = i128::from(got) - i128::from(expected);
            failures.push(format!(
                "    d{depth}: expected {expected}, got {got} ({delta:+})"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{label}\n  {fen}\n{}",
        failures.join("\n")
    );
}

/// Every legal move, king-takes-rook spelling, sorted.
#[must_use]
pub fn legal_uci(label: &str, fen: &str) -> Vec<String> {
    let b = board(label, fen);
    let list = generate_legal(&b);
    let mut moves: Vec<String> = list
        .as_slice()
        .iter()
        .map(|m| m.to_uci_chess960())
        .collect();
    moves.sort();
    moves
}

/// Every legal move as a `(uci, is_castle)` pair.
#[must_use]
pub fn legal_moves(label: &str, fen: &str) -> Vec<(String, Move)> {
    let b = board(label, fen);
    let list = generate_legal(&b);
    let mut moves: Vec<(String, Move)> = list
        .as_slice()
        .iter()
        .map(|m| (m.to_uci_chess960(), *m))
        .collect();
    moves.sort_by(|a, b| a.0.cmp(&b.0));
    moves
}

/// Compare a generated move list against the corpus one as a set, naming what
/// is missing and what is spurious. A count alone can be right by
/// cancellation: one missing evasion and one illegal move generated is a
/// perfect total and a broken engine.
pub fn assert_move_list(label: &str, expected: &[String], got: &[String]) {
    let want: std::collections::BTreeSet<&String> = expected.iter().collect();
    let have: std::collections::BTreeSet<&String> = got.iter().collect();
    let missing: Vec<&&String> = want.difference(&have).collect();
    let spurious: Vec<&&String> = have.difference(&want).collect();
    // Length first, and separately from the set difference: a generator that
    // emits every move twice has an identical move *set* and twice the node
    // count, so set equality alone would pass it.
    assert_eq!(
        got.len(),
        expected.len(),
        "{label}: expected {} moves, got {}; a duplicate is not a set difference",
        expected.len(),
        got.len()
    );
    assert!(
        missing.is_empty() && spurious.is_empty(),
        "{label}\n  expected {} moves, got {}\n  missing:  {missing:?}\n  spurious: {spurious:?}",
        expected.len(),
        got.len()
    );
}

/// The full section 3 assertion: node counts, then the castling verdict, then the
/// exact castling move.
pub fn assert_castling_case(selector: &str, stm: Stm) {
    let case = castling_case(selector, stm);
    let label = format!("castling [{selector}] {stm:?}");
    assert_perft(&label, &case.fen, &case.nodes);

    let castles: Vec<String> = legal_moves(&label, &case.fen)
        .into_iter()
        .filter(|(_, m)| m.is_castle())
        .map(|(uci, _)| uci)
        .collect();

    match &case.castling {
        None => assert!(
            castles.is_empty(),
            "{label}: corpus says no castling move is legal, engine generated {castles:?}\n  {}\n  {}",
            case.fen,
            case.reason
        ),
        Some(expected) => assert_eq!(
            castles,
            vec![expected.clone()],
            "{label}: corpus says the only legal castling move is `{expected}`\n  {}\n  {}",
            case.fen,
            case.reason
        ),
    }
}

/// The legal move whose king-takes-rook spelling is `uci`.
///
/// # Panics
///
/// If no legal move has that spelling, which is itself the assertion in
/// several tests.
#[must_use]
pub fn legal_move_named(label: &str, fen: &str, uci: &str) -> Move {
    let moves = legal_moves(label, fen);
    moves
        .iter()
        .find(|(u, _)| u == uci)
        .unwrap_or_else(|| {
            panic!(
                "{label}: `{uci}` is not legal here\n  {fen}\n  generated: {:?}",
                moves.iter().map(|(u, _)| u.clone()).collect::<Vec<_>>()
            )
        })
        .1
}

/// Assert that `selectors` partition `reasons`: every selector matches at
/// least one row, and every row is matched by exactly one selector.
///
/// Checking each selector for uniqueness individually is not enough. Two
/// selectors can both resolve to the same row (leaving a third row with no
/// test at all), and every per-selector assertion still passes. This is the
/// check that the *block* is covered, rather than that each test found
/// something.
pub fn assert_selectors_partition(label: &str, selectors: &[&str], reasons: &[String]) {
    let mut uncovered: Vec<&String> = Vec::new();
    let mut multiply_covered: Vec<(String, Vec<&str>)> = Vec::new();

    for reason in reasons {
        let hits: Vec<&str> = selectors
            .iter()
            .copied()
            .filter(|s| reason.contains(s))
            .collect();
        match hits.len() {
            0 => uncovered.push(reason),
            1 => {}
            _ => multiply_covered.push((reason.clone(), hits)),
        }
    }

    let barren: Vec<&&str> = selectors
        .iter()
        .filter(|s| !reasons.iter().any(|r| r.contains(**s)))
        .collect();

    assert!(
        uncovered.is_empty() && multiply_covered.is_empty() && barren.is_empty(),
        "{label}: selectors do not partition the block\n  \
         rows no selector reaches ({}): {:#?}\n  \
         rows more than one selector reaches ({}): {:#?}\n  \
         selectors matching nothing ({}): {barren:?}",
        uncovered.len(),
        uncovered.iter().map(|r| truncate(r)).collect::<Vec<_>>(),
        multiply_covered.len(),
        multiply_covered
            .iter()
            .map(|(r, s)| (truncate(r), s.clone()))
            .collect::<Vec<_>>(),
        barren.len(),
    );
}

fn truncate(s: &str) -> String {
    s.chars().take(60).collect()
}
