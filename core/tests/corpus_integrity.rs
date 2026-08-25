// SPDX-License-Identifier: GPL-3.0-or-later

//! Checks on the public corpus fixture itself.
//!
//! These are the only tests in this directory that pass today. They assert
//! nothing about the engine: they assert that the fixture was read the way
//! the other tests assume, which is what makes the other tests' failures
//! meaningful rather than a parser artefact.
//!
//! They also catch the fixture contradicting itself. The standard values
//! appear once in a readable summary table and once in the named TSV block,
//! and there is nothing but these tests stopping the two from drifting apart.

mod support;

/// Section 1 gives six positions, each with a FEN and a run of depths from 1.
#[test]
fn corpus_section_1_is_six_positions_with_contiguous_depths() {
    let positions = support::standard_positions();
    assert_eq!(positions.len(), 6, "Section 1 should hold six positions");
    for p in &positions {
        assert!(!p.fen.is_empty(), "{} has no FEN", p.name);
        let depths: Vec<u32> = p.nodes.iter().map(|(d, _)| *d).collect();
        let expected: Vec<u32> = (1..=u32::try_from(depths.len()).expect("fits")).collect();
        assert_eq!(depths, expected, "{} has a gap in its depths", p.name);
        assert!(depths.len() >= 6, "{} stops at d{}", p.name, depths.len());
    }
}

/// Every summary row has one cell per depth column, empty cells included.
///
/// The absent values are empty cells, and an empty cell has to keep the space
/// between its pipes. The reader trims repeated pipes from both ends of a row
/// before splitting it, so a row ending `033 ||` is one column short rather
/// than eight columns with the last one blank. Today the column that would
/// vanish is the empty one, so the values read the same and nothing else would
/// notice; that stops being true the moment an absent value is not the last in
/// its row. Counting the columns here makes the space a checked property of the
/// file instead of a note somebody has to have read.
#[test]
fn corpus_section_1_summary_rows_all_have_eight_columns() {
    let names: Vec<String> = support::standard_positions()
        .into_iter()
        .map(|p| p.name)
        .collect();
    let mut rows = 0;
    for line in support::FIXTURE.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if !names.contains(&cells[0].to_ascii_lowercase()) {
            continue;
        }
        rows += 1;
        assert_eq!(
            cells.len(),
            8,
            "the summary row for {} has {} columns, not the name and seven depths: \
             an empty cell needs its space, `|   |` and not `||`",
            cells[0],
            cells.len()
        );
    }
    assert_eq!(rows, 6, "expected one summary row per position");
}

/// Section 1 states every node count twice in the fixture: in the readable summary
/// table with thousands separators, and in the TSV block without. They agree.
#[test]
fn corpus_section_1_summary_table_agrees_with_the_tsv_block() {
    let tsv: std::collections::BTreeMap<(String, u32), u64> = support::standard_positions()
        .into_iter()
        .flat_map(|p| {
            p.nodes
                .into_iter()
                .map(move |(d, n)| ((p.name.clone(), d), n))
        })
        .collect();
    let summary = support::standard_summary_table();

    assert_eq!(
        summary.len(),
        tsv.len(),
        "the two section 1 tables list different numbers of values"
    );
    for (name, depth, nodes) in summary {
        let from_tsv = tsv.get(&(name.clone(), depth)).unwrap_or_else(|| {
            panic!("Section 1 table has {name} d{depth}, the TSV block does not")
        });
        assert_eq!(
            *from_tsv, nodes,
            "Section 1 disagrees with itself on {name} d{depth}"
        );
    }
}

/// The two values the sign-off gate is recorded against have to be in here.
#[test]
fn corpus_holds_the_completion_gate_values() {
    let startpos = support::standard("startpos");
    let kiwipete = support::standard("kiwipete");
    assert!(
        startpos.nodes.iter().any(|(d, _)| *d == 7),
        "the gate needs startpos to depth 7"
    );
    assert!(
        kiwipete.nodes.iter().any(|(d, _)| *d == 6),
        "the gate needs Kiwipete to depth 6"
    );
}

/// Section 2 is twenty arrays, each to depth 5, with distinct index pairs.
#[test]
fn corpus_section_2_is_twenty_distinct_arrays_to_depth_5() {
    let arrays = support::dfrc_arrays();
    assert_eq!(
        arrays.len(),
        20,
        "Section 2 should hold twenty start arrays"
    );
    let mut seen = std::collections::BTreeSet::new();
    for a in &arrays {
        assert!(
            seen.insert((a.wid, a.bid)),
            "Section 2 lists {}/{} twice",
            a.wid,
            a.bid
        );
        assert_eq!(
            a.nodes.len(),
            5,
            "{}/{} does not run to depth 5",
            a.wid,
            a.bid
        );
        assert!(
            a.wid < 960 && a.bid < 960,
            "{}/{} is not a Scharnagl index",
            a.wid,
            a.bid
        );
    }
}

/// The standard start array must be present as a control, and its numbers
/// must be the standard start position's. That single row is what shows the
/// Chess960 path and the standard path agree.
#[test]
fn corpus_dfrc_control_array_matches_the_standard_start_position() {
    let control = support::dfrc(518, 518);
    let startpos = support::standard("startpos");
    for (depth, nodes) in &control.nodes {
        let expected = startpos
            .nodes
            .iter()
            .find(|(d, _)| d == depth)
            .expect("startpos runs at least this deep")
            .1;
        assert_eq!(
            *nodes, expected,
            "array 518/518 in section 2 disagrees with startpos in section 1 \
             at d{depth}"
        );
    }
}

/// The castling block is thirty cases (fifteen rules, each mirrored), and its
/// verdict column agrees with its move column.
#[test]
fn corpus_castling_block_is_thirty_consistent_cases() {
    let cases = support::castling_cases();
    assert_eq!(
        cases.len(),
        30,
        "the castling block should hold fifteen rules x two colours"
    );
    let white = cases
        .iter()
        .filter(|c| support::Stm::of_case(c) == support::Stm::White)
        .count();
    assert_eq!(
        white, 15,
        "the two colours should be evenly split, got {white} White"
    );
    for c in &cases {
        assert_eq!(
            c.legal,
            c.castling.is_some(),
            "Section 3 verdict and castling column disagree: {}",
            c.reason
        );
        assert_eq!(c.nodes.len(), 3, "Section 3 rows run to depth 3");
        if let Some(m) = &c.castling {
            assert_eq!(
                m.len(),
                4,
                "Section 3 castling move `{m}` is not king-takes-rook"
            );
        }
    }
}

/// Each ambiguity-proof move list must have exactly as many moves as its own
/// row in the castling block says the position has at depth 1, and must
/// contain both of the two moves the proof turns on.
#[test]
fn corpus_ambiguity_proofs_agree_with_their_own_perft_rows() {
    let proofs = support::ambiguity_proofs();
    assert_eq!(
        proofs.len(),
        2,
        "the proof should be stated for both colours"
    );
    let rows = support::castling_pair("THE AMBIGUITY PROOF");

    for (fen, moves) in proofs {
        let case = rows
            .iter()
            .find(|c| c.fen == fen)
            .unwrap_or_else(|| panic!("no castling row for the proof position {fen}"));
        let d1 = case.nodes[0].1;
        assert_eq!(
            moves.len() as u64,
            d1,
            "{fen}: the proof lists {} moves, its row says d1 = {d1}",
            moves.len()
        );
        let rank = if fen.contains(" w ") { '1' } else { '8' };
        for m in [format!("f{rank}g{rank}"), format!("f{rank}h{rank}")] {
            assert!(
                moves.contains(&m),
                "{fen}: the proof needs `{m}` in its move list"
            );
        }
    }
}

/// Every edge case has a full depth-1 move list whose length agrees with the
/// `d1` column and with its own stated count.
#[test]
fn corpus_move_lists_agree_with_their_node_counts() {
    let mut cases = support::edge_cases();
    assert_eq!(
        cases.len(),
        11,
        "the edge-case block should hold eleven positions"
    );
    cases.push(support::move_capacity());
    assert_eq!(
        support::expected_move_lists().len(),
        cases.len(),
        "every edge case and the capacity position need an expected move list"
    );

    for c in &cases {
        let e = support::expected_moves(&c.fen);
        let d1 = c.nodes[0].1;
        assert_eq!(
            e.stated_count as u64, d1,
            "Section 4 {}: the list says ({}), the d1 column says {d1}",
            c.reason, e.stated_count
        );
        assert_eq!(
            e.moves.len() as u64,
            d1,
            "Section 4 {}: {} moves listed, d1 says {d1}",
            c.reason,
            e.moves.len()
        );
        let unique: std::collections::BTreeSet<&String> = e.moves.iter().collect();
        assert_eq!(
            unique.len(),
            e.moves.len(),
            "Section 4 {}: a move is listed twice",
            c.reason
        );
    }
}

/// The two positions whose annotations name checking moves must name moves
/// that are actually in their own move lists.
#[test]
fn corpus_check_annotations_name_moves_from_their_own_lists() {
    let annotated: Vec<_> = support::expected_move_lists()
        .into_iter()
        .filter(|e| !e.checking_moves().is_empty())
        .collect();
    assert_eq!(
        annotated.len(),
        2,
        "expected two positions annotated with `give check`, found {}",
        annotated.len()
    );
    for e in annotated {
        for m in e.checking_moves() {
            assert!(
                e.moves.contains(&m),
                "`{m}` is claimed to give check but is not in the position's move list\n  {}",
                e.fen
            );
        }
    }
}

/// Block names are the addressing scheme, so they have to be unique.
#[test]
fn corpus_block_names_are_unique() {
    let names = support::block_names();
    let unique: std::collections::BTreeSet<&String> = names.iter().collect();
    assert_eq!(
        unique.len(),
        names.len(),
        "duplicate block name in {names:?}"
    );
    assert!(
        names.len() >= 12,
        "expected at least twelve named blocks, found {names:?}"
    );
}

/// Every rule in the castling block exists for both colours, and the mirrored
/// row has the same node counts: mirroring is an exact symmetry, so a
/// difference is a transcription error in the corpus, not a chess fact.
#[test]
fn corpus_castling_mirrors_agree_with_their_originals() {
    let cases = support::castling_cases();
    let mut pairs: std::collections::BTreeMap<String, Vec<&support::CastlingCase>> =
        std::collections::BTreeMap::new();
    for c in &cases {
        let rule = c
            .reason
            .trim_start_matches("MIRRORED (rank 8): ")
            .to_string();
        pairs.entry(rule).or_default().push(c);
    }
    assert_eq!(pairs.len(), 15, "expected fifteen distinct castling rules");
    for (rule, group) in pairs {
        assert_eq!(group.len(), 2, "rule `{rule}` is not mirrored");
        assert_eq!(
            group[0].nodes, group[1].nodes,
            "mirrors of `{rule}` disagree on node counts"
        );
        assert_eq!(
            group[0].legal, group[1].legal,
            "mirrors of `{rule}` disagree on the verdict"
        );
        let sides: std::collections::BTreeSet<_> =
            group.iter().map(|c| support::Stm::of_case(c)).collect();
        assert_eq!(sides.len(), 2, "both rows of `{rule}` are the same colour");
    }
}

/// The immediate-castle rows must name arrays that exist in the DFRC block,
/// and the White rows must be those arrays' own start FENs.
#[test]
fn corpus_immediate_castles_reference_real_arrays() {
    let rows = support::immediate_castles();
    assert_eq!(
        rows.len(),
        5,
        "the corpus prose names five immediate castles"
    );
    for r in &rows {
        let array = support::dfrc(r.wid, r.bid);
        if r.stm == support::Stm::White {
            assert_eq!(
                r.fen, array.fen,
                "{}/{} White row is not the array's start FEN",
                r.wid, r.bid
            );
        }
        assert_eq!(
            r.castling.len(),
            4,
            "`{}` is not a king-takes-rook move",
            r.castling
        );
        assert_eq!(r.nodes.len(), 4);
    }
}

/// The two spellings of a notation row must differ in the castling field and
/// nowhere else; otherwise the row is comparing two different positions.
#[test]
fn corpus_fen_notation_rows_differ_only_in_the_castling_field() {
    let rows = support::fen_notations();
    assert_eq!(rows.len(), 2, "expected two notation rows");
    for r in &rows {
        let a: Vec<&str> = r.shredder.split_whitespace().collect();
        let b: Vec<&str> = r.xfen.split_whitespace().collect();
        assert_eq!(a.len(), 6, "malformed Shredder FEN: {}", r.shredder);
        assert_eq!(b.len(), 6, "malformed X-FEN: {}", r.xfen);
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            if i == 2 {
                assert_ne!(
                    x, y,
                    "the row proves nothing if both spellings are identical"
                );
            } else {
                assert_eq!(x, y, "field {i} differs, so these are different positions");
            }
        }
    }
}

/// A capture can only ever remove rights, never grant them.
#[test]
fn corpus_rights_captures_only_remove_rights() {
    let rows = support::rights_captures();
    assert_eq!(rows.len(), 2, "expected two rights-capture rows");
    for r in &rows {
        assert_ne!(
            r.before, r.after,
            "`{}` removes no right, so it tests nothing",
            r.mv
        );
        for c in r.after.chars() {
            assert!(
                r.before.contains(c),
                "`{}` grants `{c}`: rights are only ever removed",
                r.mv
            );
        }
    }
}

/// The ep-evasion row must be one of the edge cases, and its whole point is
/// that the move's destination is outside the naive target mask.
#[test]
fn corpus_ep_evasion_destination_is_outside_the_mask() {
    let rows = support::ep_evasions();
    assert_eq!(rows.len(), 1);
    for r in &rows {
        support::edge_case("EN PASSANT AS A CHECK EVASION");
        let case = support::edge_cases()
            .into_iter()
            .find(|c| c.fen == r.fen)
            .expect("the ep-evasion FEN must also be an edge case");
        assert_eq!(
            case.nodes[0].1, 8,
            "the ep-evasion position should have 8 legal moves"
        );
        let dest = r.mv[2..4].to_string();
        assert!(!r.mask.contains(&dest), "the mask already contains {dest}");
        assert!(
            r.mask.contains(&r.checker),
            "the mask must contain the checker"
        );
    }
}

/// The capacity position must actually be at the bound.
#[test]
fn corpus_capacity_position_is_at_the_move_bound() {
    let c = support::move_capacity();
    assert_eq!(
        c.nodes[0].1, 218,
        "the capacity position must have 218 legal moves"
    );
    let moves = support::expected_moves(&c.fen);
    assert_eq!(moves.moves.len(), 218, "its move list must hold all 218");
    let unique: std::collections::BTreeSet<&String> = moves.moves.iter().collect();
    assert_eq!(unique.len(), 218, "a move is listed twice");
}

/// Each divide must sum to that position's perft, and list exactly its root
/// moves. This is the check that makes the divide data usable as a reference:
/// a divide that does not sum to the total is worse than no divide.
#[test]
fn corpus_divides_sum_to_their_perft_totals() {
    let rows = support::divides();
    assert!(!rows.is_empty(), "no divide data");
    let mut groups: std::collections::BTreeMap<(String, u32), Vec<&support::DivideRow>> =
        std::collections::BTreeMap::new();
    for r in &rows {
        groups.entry((r.name.clone(), r.depth)).or_default().push(r);
    }
    assert_eq!(groups.len(), 3, "expected three divides");

    for ((name, depth), group) in groups {
        let pos = support::standard(&name);
        let total: u64 = group.iter().map(|r| r.nodes).sum();
        let want = pos
            .nodes
            .iter()
            .find(|(d, _)| *d == depth)
            .unwrap_or_else(|| panic!("{name} has no d{depth} perft value"))
            .1;
        assert_eq!(
            total, want,
            "{name} d{depth}: divide sums to {total}, perft says {want}"
        );

        let root = pos.nodes[0].1;
        assert_eq!(
            group.len() as u64,
            root,
            "{name} d{depth}: {} root moves listed, d1 says {root}",
            group.len()
        );
        let unique: std::collections::BTreeSet<&String> = group.iter().map(|r| &r.mv).collect();
        assert_eq!(
            unique.len(),
            group.len(),
            "{name} d{depth}: a root move is listed twice"
        );
    }
}
