// SPDX-License-Identifier: GPL-3.0-or-later

//! Castling legality written out by hand from the Chess960 rules and from no library, so this file
//! can disagree with the generator that produced the perft corpus, which no other test can.
//! Each position isolates one clause, and the black cases are the white ones mirrored.

use cadence_core::generate_legal;
use cadence_core::position::Board;

/// A position, every castling move the rules allow in it in king-takes-rook notation, and the
/// clause that decides it.
struct Case {
    fen: &'static str,
    legal: &'static [&'static str],
    rule: &'static str,
}

/// The rules: the king ends on g1 (rook side h) or c1 (rook side a) and the rook beside it on f1
/// or d1; every square either piece crosses or lands on is empty but for those two; and the king
/// is not in check, crosses no attacked square and does not end in check, while the rook may.
const CASES: &[Case] = &[
    // Where the pieces go, and what must be empty.
    Case {
        fen: "4k3/8/8/8/8/8/8/RK6 w A - 0 1",
        legal: &["b1a1"],
        rule: "a-side: king b1 to c1, rook a1 to d1, both empty",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/RK1N4 w A - 0 1",
        legal: &[],
        rule: "the rook's destination d1 holds a knight",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/6KR w H - 0 1",
        legal: &["g1h1"],
        rule: "h-side with the king already on g1: only the rook moves",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/5BKR w H - 0 1",
        legal: &[],
        rule: "the rook's destination f1 holds a bishop, though the king does not move",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/5KR1 w G - 0 1",
        legal: &["f1g1"],
        rule: "h-side with the rook on g1: king and rook exchange squares",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/2RK4 w C - 0 1",
        legal: &["d1c1"],
        rule: "a-side with the rook on c1: king and rook exchange squares",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/1RK5 w B - 0 1",
        legal: &["c1b1"],
        rule: "a-side with the king already on c1: only the rook moves, to d1",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/5K1R w H - 0 1",
        legal: &["f1h1"],
        rule: "h-side: king f1 to g1, rook h1 to f1, the king's own square",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/5KNR w H - 0 1",
        legal: &[],
        rule: "a knight on g1 is on both the king's path and the rook's",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/RKR5 w AC - 0 1",
        legal: &["b1c1"],
        rule: "a-side is blocked by the other rook on c1; h-side may cross its own rook's square",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/RKR5 w KQ - 0 1",
        legal: &["b1c1"],
        rule: "the same rights in the outer-rook spelling: K is the c1 rook, Q the a1 rook",
    },
    // What may be attacked, and what may not.
    Case {
        fen: "1r2k3/8/8/8/8/8/8/RK6 w A - 0 1",
        legal: &[],
        rule: "the king is in check on b1",
    },
    Case {
        fen: "2r1k3/8/8/8/8/8/8/RK6 w A - 0 1",
        legal: &[],
        rule: "the king's destination c1 is attacked",
    },
    Case {
        fen: "3rk3/8/8/8/8/8/8/RK6 w A - 0 1",
        legal: &["b1a1"],
        rule: "d1 is attacked, and only the rook crosses it",
    },
    Case {
        fen: "r3k3/8/8/8/8/8/8/RK6 w A - 0 1",
        legal: &["b1a1"],
        rule: "the castling rook itself stands attacked on a1",
    },
    Case {
        fen: "1r2k3/8/8/8/8/8/8/R2K4 w A - 0 1",
        legal: &["d1a1"],
        rule: "b1 is attacked, and only the rook crosses it",
    },
    Case {
        fen: "k3r3/8/8/8/8/8/8/1K5R w H - 0 1",
        legal: &[],
        rule: "the king's path from b1 to g1 crosses the attacked e1",
    },
    Case {
        fen: "k7/8/8/8/8/8/8/1K5R w H - 0 1",
        legal: &["b1h1"],
        rule: "the same long path with nothing attacking it",
    },
    Case {
        fen: "4k1r1/8/8/8/8/8/8/5KR1 w G - 0 1",
        legal: &[],
        rule: "g1 is attacked through its own rook, which the king takes the place of",
    },
    Case {
        fen: "4k3/8/8/8/8/8/8/rRK5 w B - 0 1",
        legal: &[],
        rule: "the rook leaving b1 uncovers the a1 rook, so the king would end in check on c1",
    },
];

fn castles(fen: &str) -> Vec<String> {
    let board = Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"));
    let mut out: Vec<String> = generate_legal(&board)
        .as_slice()
        .iter()
        .filter(|m| m.is_castle())
        .map(|m| m.to_uci_chess960())
        .collect();
    out.sort();
    out
}

/// The position with the board turned over and the colours exchanged, so every rule above applies
/// to black on the eighth rank.
fn mirror_fen(fen: &str) -> String {
    let fields: Vec<&str> = fen.split(' ').collect();
    let swap = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect()
    };
    let ranks: Vec<String> = fields[0].split('/').rev().map(swap).collect();
    let side = if fields[1] == "w" { "b" } else { "w" };
    format!(
        "{} {side} {} {} {} {}",
        ranks.join("/"),
        swap(fields[2]),
        fields[3],
        fields[4],
        fields[5]
    )
}

fn mirror_uci(uci: &str) -> String {
    uci.chars()
        .map(|c| match c {
            '1'..='8' => char::from(b'1' + (b'8' - c as u8)),
            other => other,
        })
        .collect()
}

#[test]
fn white_castling_follows_the_rules() {
    for case in CASES {
        let mut want: Vec<String> = case.legal.iter().map(|&s| String::from(s)).collect();
        want.sort();
        assert_eq!(castles(case.fen), want, "{}\n  {}", case.rule, case.fen);
    }
}

#[test]
fn black_castling_follows_the_same_rules() {
    for case in CASES {
        let fen = mirror_fen(case.fen);
        let mut want: Vec<String> = case.legal.iter().map(|&s| mirror_uci(s)).collect();
        want.sort();
        assert_eq!(castles(&fen), want, "{}, mirrored\n  {fen}", case.rule);
    }
}

#[test]
fn the_mirror_is_its_own_inverse() {
    for case in CASES {
        assert_eq!(mirror_fen(&mirror_fen(case.fen)), case.fen);
    }
    assert_eq!(mirror_uci("b1a1"), "b8a8");
}
