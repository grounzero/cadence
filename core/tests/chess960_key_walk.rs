// SPDX-License-Identifier: GPL-3.0-or-later

//! Chess960 keys against their recomputation and a fresh parse of both FEN styles, over seeded
//! random games that castle on both sides, take castling rooks and keep two rooks on one side.
//! A hashed perft over the DFRC corpus shares one table across every array, so a key that names
//! two positions changes a count or fails the identity check where its entry is read.

mod support;

use std::collections::HashMap;
use std::io::Write;

use cadence_core::castling::{CastleSide, ci};
use cadence_core::position::Board;
use cadence_core::types::{Colour, Piece, PieceType, Square};
use cadence_core::{FenStyle, Move, attacks, generate_legal, perft};
use support::generative::{self as generate, Rng};

const GAMES: usize = 3000;
const PLIES: usize = 160;
const TWO_ROOK_WALKS: usize = 40;
const SEED: u64 = 0x0960_0960_0960_0960;

/// Two rooks on each side of each king, the extra two promoted from the missing pawns; the rights
/// field picks which rook holds each right.
const TWO_ROOKS: &str = "rr1qk1rr/1pppppp1/8/8/8/8/1PPPPPP1/RR1QK1RR w";

#[derive(Default)]
struct Coverage {
    nodes: usize,
    castles: [usize; 4],
    rook_captures: usize,
    two_rook_rights: usize,
}

fn board(fen: &str) -> Board {
    Board::from_fen(fen).unwrap_or_else(|e| panic!("{fen}: {e:?}"))
}

/// Every choice of inner or outer rook for each of the four rights.
fn two_rook_fens() -> Vec<String> {
    let mut out = Vec::new();
    for wk in ['G', 'H'] {
        for wq in ['B', 'A'] {
            for bk in ['g', 'h'] {
                for bq in ['b', 'a'] {
                    out.push(format!("{TWO_ROOKS} {wk}{wq}{bk}{bq} - 0 1"));
                }
            }
        }
    }
    out
}

/// The position a key stands for: the FEN without its clocks, and without an en passant square no
/// pawn of the side to move attacks, which the key leaves out.
fn identity(b: &Board) -> String {
    let fen = b.to_fen(FenStyle::Shredder);
    let mut fields: Vec<&str> = fen.split(' ').take(4).collect();
    if let Some(ep) = b.ep_square() {
        let us = b.side_to_move();
        let takers = attacks::pawn_attacks(us.flip(), ep) & b.pieces(us, PieceType::Pawn);
        if !takers.any() {
            fields[3] = "-";
        }
    }
    fields.join(" ")
}

fn takes_a_castling_rook(b: &Board, m: Move) -> bool {
    let them = b.side_to_move().flip();
    if !m.is_capture() {
        return false;
    }
    for s in CastleSide::ALL {
        let rook = b.layout().rook_from[ci(them, s)].get();
        if b.castling_rights().has(them, s) && rook == Some(m.to_sq()) {
            return true;
        }
    }
    false
}

/// The rooks of colour `c` on the rank and the side of the king where right `s`'s rook stands.
fn rooks_beside(b: &Board, c: Colour, s: CastleSide) -> usize {
    let Some(from) = b.layout().rook_from[ci(c, s)].get() else {
        return 0;
    };
    let (from, king) = (from.index(), b.king_square(c).index());
    let rook = Piece::new(c, PieceType::Rook);
    Square::all()
        .filter(|&sq| b.piece_at(sq) == Some(rook))
        .map(Square::index)
        .filter(|&i| i / 8 == from / 8 && (i < king) == (from < king))
        .count()
}

fn two_rooks_on_a_held_side(b: &Board) -> bool {
    for c in Colour::ALL {
        for s in CastleSide::ALL {
            if b.castling_rights().has(c, s) && rooks_beside(b, c, s) >= 2 {
                return true;
            }
        }
    }
    false
}

fn check_node(label: &str, b: &Board, seen: &mut HashMap<u64, String>) {
    let key = b.key();
    let shredder = b.to_fen(FenStyle::Shredder);
    assert_eq!(
        key,
        b.recompute_key(),
        "{label}: key vs recomputation\n  {shredder}"
    );
    for fen in [shredder, b.to_fen(FenStyle::XFen)] {
        let fresh = board(&fen);
        assert_eq!(fresh.key(), key, "{label}: key vs a fresh parse\n  {fen}");
        assert_eq!(
            fresh.castling_rights(),
            b.castling_rights(),
            "{label}: rights after a fresh parse\n  {fen}"
        );
    }
    let id = identity(b);
    if let Some(other) = seen.insert(key, id.clone()) {
        assert_eq!(
            other, id,
            "{label}: two positions share the key 0x{key:016x}"
        );
    }
}

fn choose(b: &Board, legal: &[Move], rng: &mut Rng) -> Move {
    let (mut castles, mut takes) = (Vec::new(), Vec::new());
    for &m in legal {
        if m.is_castle() {
            castles.push(m);
        } else if takes_a_castling_rook(b, m) {
            takes.push(m);
        }
    }
    if !castles.is_empty() && rng.below(2) == 0 {
        return castles[rng.below(castles.len())];
    }
    if !takes.is_empty() && rng.below(2) == 0 {
        return takes[rng.below(takes.len())];
    }
    legal[rng.below(legal.len())]
}

fn count(b: &Board, cov: &mut Coverage) {
    cov.nodes += 1;
    if two_rooks_on_a_held_side(b) {
        cov.two_rook_rights += 1;
    }
}

fn play(
    label: &str,
    fen: &str,
    rng: &mut Rng,
    seen: &mut HashMap<u64, String>,
    cov: &mut Coverage,
) {
    let mut b = board(fen);
    let mut keys = vec![b.key()];
    let mut played = Vec::new();
    check_node(label, &b, seen);
    count(&b, cov);
    for ply in 0..PLIES {
        let legal = generate::legal(&b);
        if legal.is_empty() {
            break;
        }
        let m = choose(&b, &legal, rng);
        if m.is_castle() {
            cov.castles[ci(b.side_to_move(), m.castle_side())] += 1;
        }
        if takes_a_castling_rook(&b, m) {
            cov.rook_captures += 1;
        }
        let _dirty = b.make_move(m);
        played.push(m);
        keys.push(b.key());
        check_node(&format!("{label}, ply {ply}"), &b, seen);
        count(&b, cov);
    }
    while let Some(m) = played.pop() {
        b.unmake_move(m);
        keys.pop();
        assert_eq!(
            Some(&b.key()),
            keys.last(),
            "{label}: key after unmaking {m:?}"
        );
    }
}

#[test]
fn chess960_keys_follow_every_move_of_random_games() {
    let mut rng = Rng::new(SEED);
    let mut seen = HashMap::new();
    let mut cov = Coverage::default();
    for game in 0..GAMES {
        let (w, b) = (rng.below(960), rng.below(960));
        let fen = generate::dfrc_start_fen(
            u32::try_from(w).expect("fits"),
            u32::try_from(b).expect("fits"),
        );
        play(
            &format!("game {game}, {w}/{b}"),
            &fen,
            &mut rng,
            &mut seen,
            &mut cov,
        );
    }
    let two_rooks = two_rook_fens();
    let keys: Vec<u64> = two_rooks.iter().map(|f| board(f).key()).collect();
    let mut distinct = keys.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), keys.len(), "two rook choices share a key");
    for (i, fen) in two_rooks.iter().enumerate() {
        for walk in 0..TWO_ROOK_WALKS {
            play(
                &format!("two rooks {i}, walk {walk}"),
                fen,
                &mut rng,
                &mut seen,
                &mut cov,
            );
        }
    }
    let _ = writeln!(
        std::io::stderr(),
        "chess960 key walk: {} nodes, {} keys, castles {:?}, {} rooks taken, {} two-rook nodes",
        cov.nodes,
        seen.len(),
        cov.castles,
        cov.rook_captures,
        cov.two_rook_rights
    );
    assert!(
        cov.castles.iter().all(|&n| n > 0),
        "castles by right {:?}",
        cov.castles
    );
    assert!(cov.rook_captures > 0, "no castling rook was taken");
    assert!(
        cov.two_rook_rights > 0,
        "no node held a right beside a second rook"
    );
}

type Table = HashMap<(u64, u32), (u64, String)>;

/// Perft through a table keyed by the position's key, stored from depth 2 with the position's
/// identity, so a shared key either changes a count or fails where its entry is read.
fn hashed(b: &mut Board, depth: u32, table: &mut Table) -> u64 {
    if depth == 0 {
        return 1;
    }
    let moves = generate_legal(b);
    if depth == 1 {
        return moves.len() as u64;
    }
    let key = b.key();
    if let Some((nodes, id)) = table.get(&(key, depth)) {
        assert_eq!(*id, identity(b), "two positions share the key 0x{key:016x}");
        return *nodes;
    }
    let mut nodes = 0;
    for m in moves.iter() {
        let _dirty = b.make_move(m);
        nodes += hashed(b, depth - 1, table);
        b.unmake_move(m);
    }
    table.insert((key, depth), (nodes, identity(b)));
    nodes
}

fn hashed_against(label: &str, fen: &str, nodes: &[(u32, u64)], table: &mut Table) -> usize {
    let mut b = board(fen);
    let mut read = 0;
    for &(depth, want) in nodes {
        if depth <= support::FAST_DFRC_MAX_DEPTH {
            assert_eq!(
                hashed(&mut b, depth, table),
                want,
                "{label}, depth {depth}\n  {fen}"
            );
            read += 1;
        }
    }
    read
}

#[test]
fn a_hashed_perft_over_the_dfrc_corpus_matches_its_counts() {
    let mut table = Table::new();
    let mut read = 0;
    for a in support::dfrc_arrays() {
        let label = format!("array {}/{}", a.wid, a.bid);
        read += hashed_against(&label, &a.fen, &a.nodes, &mut table);
    }
    for c in support::immediate_castles() {
        let label = format!("immediate castle {}/{}", c.wid, c.bid);
        read += hashed_against(&label, &c.fen, &c.nodes, &mut table);
    }
    for c in support::castling_cases() {
        read += hashed_against(&c.reason, &c.fen, &c.nodes, &mut table);
    }
    for r in support::rights_captures() {
        read += hashed_against(&r.reason, &r.fen, &r.nodes, &mut table);
    }
    for fen in two_rook_fens() {
        let mut b = board(&fen);
        for depth in 1..=4 {
            let plain = perft(&mut b, depth);
            assert_eq!(
                hashed(&mut b, depth, &mut table),
                plain,
                "{fen}, depth {depth}"
            );
            read += 1;
        }
    }
    let _ = writeln!(
        std::io::stderr(),
        "chess960 hashed perft: {read} counts matched, {} table entries",
        table.len()
    );
}
