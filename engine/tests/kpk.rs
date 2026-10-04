// SPDX-License-Identifier: GPL-3.0-or-later

//! The table against a second solver that knows nothing of its shortcuts: legal moves from `core`,
//! real promotions to every piece, and every pawn file, so the mirror is checked too.

use cadence_core::position::Board;
use cadence_core::{Colour, PieceType, Square, attacks, generate_legal};
use cadence_engine::kpk::wins;

const N: usize = 2 * 64 * 64 * 48;

fn slot(white_to_move: bool, wk: Square, bk: Square, pawn: Square) -> usize {
    let p = (pawn.rank().index() - 1) * 8 + pawn.file().index();
    ((usize::from(!white_to_move) * 64 + wk.index()) * 64 + bk.index()) * 48 + p
}

fn fen(white_to_move: bool, wk: Square, bk: Square, pawn: Square) -> String {
    let mut rows = Vec::new();
    for rank in (0..8).rev() {
        let mut row = String::new();
        let mut empty = 0;
        for file in 0..8 {
            let sq = Square::new(rank * 8 + file);
            let c = if sq == wk {
                'K'
            } else if sq == bk {
                'k'
            } else if sq == pawn {
                'P'
            } else {
                empty += 1;
                continue;
            };
            if empty > 0 {
                row.push_str(&empty.to_string());
                empty = 0;
            }
            row.push(c);
        }
        if empty > 0 {
            row.push_str(&empty.to_string());
        }
        rows.push(row);
    }
    let stm = if white_to_move { "w" } else { "b" };
    format!("{} {stm} - - 0 1", rows.join("/"))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum V {
    Unknown,
    Draw,
    Win,
}

/// What a position that has left the ending is worth, or the slot it moved to.
enum Next {
    Done(V),
    Slot(usize),
}

/// A queen or rook that survives Black's reply wins; a minor, a capture or a stalemate draws.
fn after(child: &Board) -> Next {
    let white = child.by_colour(Colour::White);
    if white.count() == 1 {
        return Next::Done(V::Draw);
    }
    let pawns = child.pieces(Colour::White, PieceType::Pawn);
    if let Some(pawn) = pawns.lsb() {
        let wk = child.king_square(Colour::White);
        let bk = child.king_square(Colour::Black);
        let white_to_move = child.side_to_move() == Colour::White;
        return Next::Slot(slot(white_to_move, wk, bk, pawn));
    }
    let replies = generate_legal(child);
    if replies.is_empty() {
        return Next::Done(if child.in_check() { V::Win } else { V::Draw });
    }
    let heavy = child.pieces(Colour::White, PieceType::Queen)
        | child.pieces(Colour::White, PieceType::Rook);
    if heavy.is_empty() {
        return Next::Done(V::Draw);
    }
    let taken = replies.as_slice().iter().any(|&m| {
        let mut b = child.duplicate();
        b.play(m);
        b.by_colour(Colour::White).count() == 1
    });
    Next::Done(if taken { V::Draw } else { V::Win })
}

struct Node {
    white_to_move: bool,
    terminal: Option<V>,
    next: Vec<Next>,
}

fn solve() -> Vec<Option<V>> {
    let mut nodes: Vec<Option<Node>> = (0..N).map(|_| None).collect();
    for white_to_move in [true, false] {
        for wk in Square::all() {
            for bk in Square::all() {
                for pawn in Square::all().filter(|s| (1..7).contains(&s.rank().index())) {
                    let distinct = wk != bk && wk != pawn && bk != pawn;
                    let apart = wk.file().index().abs_diff(bk.file().index()) > 1
                        || wk.rank().index().abs_diff(bk.rank().index()) > 1;
                    if !distinct || !apart {
                        continue;
                    }
                    let Ok(board) = Board::from_fen(&fen(white_to_move, wk, bk, pawn)) else {
                        continue;
                    };
                    // Black in check with White to move cannot arise.
                    if white_to_move && attacks::pawn_attacks(Colour::White, pawn).contains(bk) {
                        continue;
                    }
                    let moves = generate_legal(&board);
                    let mated = !white_to_move && board.in_check();
                    let terminal = moves
                        .is_empty()
                        .then_some(if mated { V::Win } else { V::Draw });
                    let next = moves
                        .as_slice()
                        .iter()
                        .map(|&m| {
                            let mut child = board.duplicate();
                            child.play(m);
                            after(&child)
                        })
                        .collect();
                    nodes[slot(white_to_move, wk, bk, pawn)] = Some(Node {
                        white_to_move,
                        terminal,
                        next,
                    });
                }
            }
        }
    }
    let mut value: Vec<V> = vec![V::Unknown; N];
    loop {
        let mut changed = false;
        for i in 0..N {
            let Some(node) = &nodes[i] else { continue };
            if value[i] != V::Unknown {
                continue;
            }
            let v = node.terminal.unwrap_or_else(|| {
                let of = |n: &Next| match *n {
                    Next::Done(v) => v,
                    Next::Slot(s) => value[s],
                };
                let (good, bad) = if node.white_to_move {
                    (V::Win, V::Draw)
                } else {
                    (V::Draw, V::Win)
                };
                if node.next.iter().any(|n| of(n) == good) {
                    good
                } else if node.next.iter().all(|n| of(n) == bad) {
                    bad
                } else {
                    V::Unknown
                }
            });
            if v != V::Unknown {
                value[i] = v;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    nodes
        .iter()
        .zip(&value)
        .map(|(n, &v)| {
            n.as_ref()
                .map(|_| if v == V::Win { V::Win } else { V::Draw })
        })
        .collect()
}

fn mirrored(sq: Square) -> Square {
    Square::new(u8::try_from(sq.index() ^ 7).expect("square"))
}

#[test]

fn the_table_agrees_with_a_solver_over_legal_moves_on_every_position() {
    let solved = solve();
    let mut checked = 0;
    let mut won = 0;
    for white_to_move in [true, false] {
        for wk in Square::all() {
            for bk in Square::all() {
                for pawn in Square::all().filter(|s| (1..7).contains(&s.rank().index())) {
                    let Some(v) = solved[slot(white_to_move, wk, bk, pawn)] else {
                        continue;
                    };
                    let (wk2, bk2, pawn2) = if pawn.file().index() < 4 {
                        (wk, bk, pawn)
                    } else {
                        (mirrored(wk), mirrored(bk), mirrored(pawn))
                    };
                    let table = wins(white_to_move, wk2, bk2, pawn2);
                    assert_eq!(table, v == V::Win, "{}", fen(white_to_move, wk, bk, pawn));
                    checked += 1;
                    won += usize::from(table);
                }
            }
        }
    }
    assert!(checked > 300_000, "only {checked} positions");
    assert!(won > checked / 2, "only {won} of {checked} won");
}

fn table(fen: &str) -> bool {
    let b = Board::from_fen(fen).expect("fen");
    let pawn = b
        .pieces(Colour::White, PieceType::Pawn)
        .lsb()
        .expect("pawn");
    let (wk, bk) = (b.king_square(Colour::White), b.king_square(Colour::Black));
    let white_to_move = b.side_to_move() == Colour::White;
    if pawn.file().index() < 4 {
        wins(white_to_move, wk, bk, pawn)
    } else {
        wins(white_to_move, mirrored(wk), mirrored(bk), mirrored(pawn))
    }
}

#[test]
fn the_textbook_positions_read_as_the_textbook_says() {
    // The king on a key square wins whoever is to move.
    assert!(table("4k3/8/4K3/8/4P3/8/8/8 w - - 0 1"));
    assert!(table("4k3/8/4K3/8/4P3/8/8/8 b - - 0 1"));
    // Kings in opposition with the pawn behind its own king: the side to move loses the opposition.
    assert!(!table("8/4k3/8/4K3/4P3/8/8/8 w - - 0 1"));
    assert!(table("8/4k3/8/4K3/4P3/8/8/8 b - - 0 1"));
    // The defending king in the rook pawn's corner draws.
    assert!(!table("k7/8/1K6/P7/8/8/8/8 w - - 0 1"));
    assert!(!table("k7/8/1K6/P7/8/8/8/8 b - - 0 1"));
    // Stalemated in the corner, and the same position with White to move wins.
    assert!(!table("7k/5K2/6P1/8/8/8/8/8 b - - 0 1"));
    assert!(table("7k/5K2/6P1/8/8/8/8/8 w - - 0 1"));
    // A pawn the defending king can take, unguarded, draws at once.
    assert!(!table("8/8/8/8/8/k7/P7/7K b - - 0 1"));
}
