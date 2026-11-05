// SPDX-License-Identifier: GPL-3.0-or-later

//! Every rule `Board::from_fen` applies, one row each: what it reads, what it drops and what it
//! refuses. A rule the parser gains or loses without a row here fails one of these tests.

use cadence_core::fen::FenStyle;
use cadence_core::position::Board;

#[derive(Debug)]
enum Outcome {
    /// Parsed, and written back in X-FEN as this.
    Reads(&'static str),
    /// Refused with the `FenError` of this name.
    Refused(&'static str),
}

use Outcome::{Reads, Refused};

const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
const BARE: &str = "4k3/8/8/8/8/8/8/4K3 w - - 0 1";

/// The rule, an input, and what the parser makes of it.
const RULES: &[(&str, &str, Outcome)] = &[
    // The fields.
    ("six fields are read", START, Reads(START)),
    (
        "four fields read the counters as 0 and 1",
        "4k3/8/8/8/8/8/8/4K3 w - -",
        Reads(BARE),
    ),
    (
        "five fields read the move number as 1",
        "4k3/8/8/8/8/8/8/4K3 w - - 7",
        Reads("4k3/8/8/8/8/8/8/4K3 w - - 7 1"),
    ),
    (
        "fields are split on any run of whitespace, at either end too",
        " 4k3/8/8/8/8/8/8/4K3\tw  -\n- 0 1 ",
        Reads(BARE),
    ),
    ("fewer than four fields are refused", "", Refused("Fields")),
    (
        "three fields are refused",
        "4k3/8/8/8/8/8/8/4K3 w -",
        Refused("Fields"),
    ),
    (
        "more than six fields are refused",
        "4k3/8/8/8/8/8/8/4K3 w - - 0 1 x",
        Refused("Fields"),
    ),
    // The placement.
    (
        "seven ranks are refused",
        "4k3/8/8/8/8/8/4K3 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "nine ranks are refused",
        "4k3/8/8/8/8/8/8/8/4K3 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a rank of nine squares is refused",
        "4k3/8/8/8/8/8/8/4K4 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a rank of seven squares is refused",
        "4k3/8/8/8/8/8/8/4K2 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a 0 digit is refused",
        "4k3/8/8/8/8/8/8/04K3 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a 9 digit is refused",
        "4k3/9/8/8/8/8/8/4K3 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "two digits in a row are two runs of empty squares",
        "4k3/44/8/8/8/8/8/4K3 w - - 0 1",
        Reads(BARE),
    ),
    (
        "a letter naming no piece is refused",
        "4k3/8/8/8/8/8/8/4K2X w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a character outside ASCII is refused",
        "4k3/8/8/8/8/8/8/4K2\u{e9} w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a White pawn on the first rank is refused",
        "4k3/8/8/8/8/8/8/P3K3 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a White pawn on the eighth rank is refused",
        "P3k3/8/8/8/8/8/8/4K3 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a Black pawn on the first rank is refused",
        "4k3/8/8/8/8/8/8/p3K3 w - - 0 1",
        Refused("Placement"),
    ),
    (
        "a Black pawn on the eighth rank is refused",
        "p3k3/8/8/8/8/8/8/4K3 w - - 0 1",
        Refused("Placement"),
    ),
    // The kings.
    (
        "no king of either colour is refused",
        "8/8/8/8/8/8/8/8 w - - 0 1",
        Refused("Kings"),
    ),
    (
        "no White king is refused",
        "4k3/8/8/8/8/8/8/8 w - - 0 1",
        Refused("Kings"),
    ),
    (
        "two White kings are refused",
        "4k3/8/8/8/8/8/8/4K1K1 w - - 0 1",
        Refused("Kings"),
    ),
    (
        "two Black kings are refused",
        "4k2k/8/8/8/8/8/8/4K3 w - - 0 1",
        Refused("Kings"),
    ),
    (
        "a side not to move in check is read",
        "4k3/4Q3/8/8/8/8/8/4K3 w - - 0 1",
        Reads("4k3/4Q3/8/8/8/8/8/4K3 w - - 0 1"),
    ),
    // The material.
    (
        "nine White pawns are refused",
        "4k3/8/8/8/8/P7/PPPPPPPP/4K3 w - - 0 1",
        Refused("Material"),
    ),
    (
        "nine Black pawns are refused",
        "4k3/pppppppp/p7/8/8/8/8/4K3 w - - 0 1",
        Refused("Material"),
    ),
    (
        "a second queen beside eight pawns is refused",
        "4k3/Q7/Q7/8/8/8/PPPPPPPP/4K3 w - - 0 1",
        Refused("Material"),
    ),
    (
        "a second queen beside seven pawns is read",
        "4k3/Q7/Q7/8/8/8/PPPPPPP1/4K3 w - - 0 1",
        Reads("4k3/Q7/Q7/8/8/8/PPPPPPP1/4K3 w - - 0 1"),
    ),
    (
        "a third bishop beside eight pawns is refused",
        "4k3/8/8/8/8/BBB5/PPPPPPPP/4K3 w - - 0 1",
        Refused("Material"),
    ),
    (
        "a third Black knight beside eight pawns is refused",
        "4k3/pppppppp/nnn5/8/8/8/8/4K3 w - - 0 1",
        Refused("Material"),
    ),
    (
        "a third rook beside eight pawns is refused",
        "4k3/8/8/8/8/8/PPPPPPPP/RRR1K3 w - - 0 1",
        Refused("Material"),
    ),
    (
        "nine queens and no pawns are read",
        "QQQQQQQQ/Q7/8/8/8/8/8/k6K w - - 0 1",
        Reads("QQQQQQQQ/Q7/8/8/8/8/8/k6K w - - 0 1"),
    ),
    (
        "ten queens are refused",
        "QQQQQQQQ/QQ6/8/8/8/8/8/k6K w - - 0 1",
        Refused("Material"),
    ),
    (
        "sixteen pieces, every pawn promoted, are read",
        "k6K/8/8/8/8/8/qrrbbnn1/qqqqqqqq w - - 0 1",
        Reads("k6K/8/8/8/8/8/qrrbbnn1/qqqqqqqq w - - 0 1"),
    ),
    (
        "twenty-eight queens are refused",
        "QQQQQQBk/Q5RB/Q6Q/Q6Q/Q6Q/Q6Q/Q6Q/KQQQQQQQ w - - 0 1",
        Refused("Material"),
    ),
    // The side to move.
    ("w is White to move", START, Reads(START)),
    (
        "b is Black to move",
        "4k3/8/8/8/8/8/8/4K3 b - - 0 1",
        Reads("4k3/8/8/8/8/8/8/4K3 b - - 0 1"),
    ),
    (
        "any other side is refused",
        "4k3/8/8/8/8/8/8/4K3 x - - 0 1",
        Refused("SideToMove"),
    ),
    (
        "an upper-case side is refused",
        "4k3/8/8/8/8/8/8/4K3 W - - 0 1",
        Refused("SideToMove"),
    ),
    // Castling.
    ("KQkq names the outermost rooks", START, Reads(START)),
    (
        "file letters name the castling rook directly",
        "r3k2r/8/8/8/8/8/8/R3K2R w HAha - 0 1",
        Reads("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1"),
    ),
    (
        "a lower-case letter is Black's",
        "r3k3/8/8/8/8/8/8/4K3 w a - 0 1",
        Reads("r3k3/8/8/8/8/8/8/4K3 w q - 0 1"),
    ),
    (
        "a file is named in X-FEN only where another rook stands outside the castling rook",
        "rkr3r1/8/8/8/8/8/8/RKR3R1 w CAca - 0 1",
        Reads("rkr3r1/8/8/8/8/8/8/RKR3R1 w CQcq - 0 1"),
    ),
    (
        "a letter given twice is read once",
        "r3k2r/8/8/8/8/8/8/R3K2R w KQkqKQkq - 0 1",
        Reads("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1"),
    ),
    (
        "a letter with no rook is dropped",
        "4k3/8/8/8/8/8/8/4K3 w KQkq - 0 1",
        Reads(BARE),
    ),
    (
        "a stale letter is dropped and the live ones are kept",
        "r3k2r/8/8/8/8/8/8/R3K3 w KQkq - 0 1",
        Reads("r3k2r/8/8/8/8/8/8/R3K3 w Qkq - 0 1"),
    ),
    (
        "K with a rook only on the queen's side is dropped",
        "4k3/8/8/8/8/8/8/R3K3 w K - 0 1",
        Reads("4k3/8/8/8/8/8/8/R3K3 w - - 0 1"),
    ),
    (
        "a letter for a king off its back rank is dropped",
        "4k3/8/8/8/8/8/4K3/R7 w Q - 0 1",
        Reads("4k3/8/8/8/8/8/4K3/R7 w - - 0 1"),
    ),
    (
        "a Black letter for a king off its back rank is dropped",
        "8/4k3/8/8/8/8/8/R3K2R b KQk - 0 1",
        Reads("8/4k3/8/8/8/8/8/R3K2R b KQ - 0 1"),
    ),
    (
        "a file letter naming an empty square is dropped",
        "4k3/8/8/8/8/8/8/R3K3 w H - 0 1",
        Reads("4k3/8/8/8/8/8/8/R3K3 w - - 0 1"),
    ),
    (
        "a file letter naming the king's own file is dropped",
        "4k3/8/8/8/8/8/8/R3K3 w E - 0 1",
        Reads("4k3/8/8/8/8/8/8/R3K3 w - - 0 1"),
    ),
    (
        "a file letter naming the other colour's rook is dropped",
        "4k3/8/8/8/8/8/8/r3K3 w A - 0 1",
        Reads("4k3/8/8/8/8/8/8/r3K3 w - - 0 1"),
    ),
    (
        "a rook off its back rank holds no right",
        "4k3/8/8/8/8/8/R7/4K3 w Q - 0 1",
        Reads("4k3/8/8/8/8/8/R7/4K3 w - - 0 1"),
    ),
    (
        "two letters naming different rooks for one right are refused",
        "4k3/8/8/8/8/8/8/4K1RR w KG - 0 1",
        Refused("Castling"),
    ),
    (
        "a character that is not a letter is refused",
        "4k3/8/8/8/8/8/8/R3K3 w KQ- - 0 1",
        Refused("Castling"),
    ),
    (
        "a letter that is neither K, Q nor a file is refused",
        "4k3/8/8/8/8/8/8/R3K3 w X - 0 1",
        Refused("Castling"),
    ),
    (
        "a character outside ASCII is refused in the castling field",
        "4k3/8/8/8/8/8/8/R3K3 w \u{c9} - 0 1",
        Refused("Castling"),
    ),
    // En passant.
    (
        "a square behind a pawn that has just double-pushed is kept, takers or not",
        "4k3/8/8/4p3/8/8/8/4K3 w - e6 0 1",
        Reads("4k3/8/8/4p3/8/8/8/4K3 w - e6 0 1"),
    ),
    (
        "Black to move reads a third-rank square",
        "4k3/8/8/8/4P3/8/8/4K3 b - e3 0 1",
        Reads("4k3/8/8/8/4P3/8/8/4K3 b - e3 0 1"),
    ),
    (
        "a square that is not a square is refused",
        "4k3/8/8/4p3/8/8/8/4K3 w - zz 0 1",
        Refused("EnPassant"),
    ),
    (
        "a square outside ASCII is refused",
        "4k3/8/8/4p3/8/8/8/4K3 w - \u{e9}6 0 1",
        Refused("EnPassant"),
    ),
    (
        "a square on the wrong rank for the side to move is refused",
        "4k3/8/8/4p3/8/8/8/4K3 b - e6 0 1",
        Refused("EnPassant"),
    ),
    (
        "a square with no pawn of the side not to move in front of it is refused",
        "4k3/8/8/3P4/8/8/8/4K3 w - e6 0 1",
        Refused("EnPassant"),
    ),
    (
        "a square with the side to move's own pawn in front of it is refused",
        "4k3/8/8/3PP3/8/8/8/4K3 w - e6 0 1",
        Refused("EnPassant"),
    ),
    (
        "an occupied square is refused",
        "4k3/8/4n3/3Pp3/8/8/8/4K3 w - e6 0 1",
        Refused("EnPassant"),
    ),
    (
        "an occupied double push origin is refused",
        "4k3/4p3/8/3Pp3/8/8/8/4K3 w - e6 0 1",
        Refused("EnPassant"),
    ),
    // The counters.
    (
        "a halfmove clock above 255 reads as 255",
        "4k3/8/8/8/8/8/8/4K3 w - - 300 1",
        Reads("4k3/8/8/8/8/8/8/4K3 w - - 255 1"),
    ),
    (
        "a halfmove clock too long for any integer reads as 255",
        "4k3/8/8/8/8/8/8/4K3 w - - 99999999999999999999999 1",
        Reads("4k3/8/8/8/8/8/8/4K3 w - - 255 1"),
    ),
    (
        "a leading plus sign is read with the number",
        "4k3/8/8/8/8/8/8/4K3 w - - +5 1",
        Reads("4k3/8/8/8/8/8/8/4K3 w - - 5 1"),
    ),
    (
        "a negative halfmove clock is refused",
        "4k3/8/8/8/8/8/8/4K3 w - - -1 1",
        Refused("Counter"),
    ),
    (
        "a halfmove clock that is not a number is refused",
        "4k3/8/8/8/8/8/8/4K3 w - - x 1",
        Refused("Counter"),
    ),
    (
        "a move number above 65535 reads as 65535",
        "4k3/8/8/8/8/8/8/4K3 w - - 0 70000",
        Reads("4k3/8/8/8/8/8/8/4K3 w - - 0 65535"),
    ),
    (
        "a move number of 0 is read as given",
        "4k3/8/8/8/8/8/8/4K3 w - - 0 0",
        Reads("4k3/8/8/8/8/8/8/4K3 w - - 0 0"),
    ),
    (
        "a move number that is not a number is refused",
        "4k3/8/8/8/8/8/8/4K3 w - - 0 0x",
        Refused("Counter"),
    ),
    (
        "digits outside ASCII are refused",
        "4k3/8/8/8/8/8/8/4K3 w - - 0 \u{ff11}",
        Refused("Counter"),
    ),
];

#[test]
fn every_rule_holds() {
    let mut broken = Vec::new();
    for (rule, fen, want) in RULES {
        let got = Board::from_fen(fen);
        let held = match (want, &got) {
            (Reads(out), Ok(board)) => board.to_fen(FenStyle::XFen) == *out,
            (Refused(name), Err(e)) => format!("{e:?}") == *name,
            _ => false,
        };
        if !held {
            let got = match got {
                Ok(board) => format!("Reads({:?})", board.to_fen(FenStyle::XFen)),
                Err(e) => format!("Refused({e:?})"),
            };
            let line = format!("{rule}: {fen:?}\n    wanted {want:?}\n    got    {got}");
            broken.push(line);
        }
    }
    assert!(
        broken.is_empty(),
        "{} of {} rules broken:\n  {}",
        broken.len(),
        RULES.len(),
        broken.join("\n  ")
    );
}

/// So an error the parser can return and no row exercises is noticed when it is added.
#[test]
fn every_refusal_has_a_row() {
    for name in [
        "Fields",
        "Placement",
        "SideToMove",
        "Castling",
        "EnPassant",
        "Counter",
        "Kings",
        "Material",
    ] {
        assert!(
            RULES
                .iter()
                .any(|(_, _, outcome)| matches!(outcome, Refused(n) if *n == name)),
            "no row is refused with {name}"
        );
    }
}
