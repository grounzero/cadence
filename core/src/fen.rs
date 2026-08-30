// SPDX-License-Identifier: GPL-3.0-or-later

//! FEN, X-FEN and Shredder-FEN.
//!
//! **Both castling spellings are accepted on input and the position records
//! which it was given**, because a GUI may send either and a position that
//! round-trips through the wrong one is a different position. `KQkq` is read
//! as the outermost rook on that side, which is X-FEN's rule and is what
//! makes standard chess a case of Chess960 rather than a separate parser.
//!
//! Parsing validates: a position this accepts has one king a side, no pawn
//! on a back rank, and castling rights whose rooks exist. What it does not
//! check is reachability by legal play, which is why `movegen` and
//! `position` carry the rules that hold for positions no game can produce.

use alloc::string::String;
use core::fmt::Write as _;

use crate::bitboard::Bitboard;
use crate::castling::{CastleSide, CastlingLayout, CastlingRights, ci};
use crate::position::{Board, Setup};
use crate::types::{Colour, File, OptSquare, Piece, PieceType, Rank, Square};

/// The standard start position, in the notation every GUI sends it in.
pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

/// Why a FEN string was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum FenError {
    /// The string did not have four to six space-separated fields.
    Fields,
    /// The piece-placement field did not describe 8 ranks of 8 files.
    Placement,
    /// The side-to-move field was neither `w` nor `b`.
    SideToMove,
    /// The castling field named a rook that is not there, or a right that
    /// cannot exist given the king's square.
    Castling,
    /// The en-passant field was not `-` or a square on rank 3 or rank 6.
    EnPassant,
    /// A halfmove or fullmove counter was not a number, or was out of range.
    Counter,
    /// Not exactly one king of each colour.
    Kings,
}

/// Which castling-field notation to emit.
///
/// These are two different notations, not a formatting preference. The variant
/// was called `Standard` and has been renamed, because "standard" invited
/// reading it as "plain `KQkq`", which is what X-FEN writes only when the
/// spelling is unambiguous.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FenStyle {
    /// `KQkq`, where each letter denotes the **outermost** rook on that side
    /// of the king, not the a- and h-file rooks. Falls back to naming the
    /// castling rook's file when, and only when, another rook of the same
    /// colour stands outside it on the same side. The two halves fall back
    /// independently, so a mixed field like `CQcq` is well-formed.
    XFen,
    /// Always names the castling rook's file: `HAha`. Never ambiguous, never
    /// mixed.
    Shredder,
}

impl Board {
    /// Parse a FEN. Accepts standard `KQkq` castling fields and the Shredder
    /// rook-file spelling, in both cases with arbitrary rook files. Four to
    /// six fields: the two counters may be omitted and default to `0 1`.
    ///
    /// # Errors
    ///
    /// [`FenError`] describes which field was rejected. In particular a
    /// castling right whose rook is not there, or whose king is not on its
    /// back rank, is [`FenError::Castling`] here rather than a panic in move
    /// generation.
    pub fn from_fen(fen: &str) -> Result<Board, FenError> {
        let fields: [&str; 6] = {
            let mut it = fen.split_whitespace();
            let mut out = ["", "", "", "", "0", "1"];
            let mut n = 0;
            for slot in &mut out {
                match it.next() {
                    Some(f) => {
                        *slot = f;
                        n += 1;
                    }
                    None => break,
                }
            }
            if n < 4 || it.next().is_some() {
                return Err(FenError::Fields);
            }
            out
        };

        let mailbox = parse_placement(fields[0])?;
        let stm = match fields[1] {
            "w" => Colour::White,
            "b" => Colour::Black,
            _ => return Err(FenError::SideToMove),
        };
        let mut kings = [None; 2];
        for c in Colour::ALL {
            let king = Piece::new(c, PieceType::King);
            let mut found = Square::all().filter(|sq| mailbox[sq.index()] == Some(king));
            match (found.next(), found.next()) {
                (Some(sq), None) => kings[c.index()] = Some(sq),
                _ => return Err(FenError::Kings),
            }
        }
        let (rights, layout) = parse_castling(fields[2], &mailbox, kings)?;
        let ep = match fields[3] {
            "-" => OptSquare::NONE,
            s => {
                let sq = Square::from_algebraic(s).ok_or(FenError::EnPassant)?;
                if sq.rank() != Rank::Three && sq.rank() != Rank::Six {
                    return Err(FenError::EnPassant);
                }
                OptSquare::some(sq)
            }
        };
        let halfmove: u8 = fields[4].parse().map_err(|_| FenError::Counter)?;
        let fullmove: u16 = fields[5].parse().map_err(|_| FenError::Counter)?;

        Ok(Board::from_setup(&Setup {
            mailbox,
            stm,
            rights,
            ep,
            halfmove,
            fullmove,
            layout,
        }))
    }

    /// Emit this position as a FEN in the requested notation.
    ///
    /// The en-passant field is emitted whenever the last move was a double
    /// pawn push, which is what the FEN specification says and what the
    /// state holds.
    #[must_use]
    pub fn to_fen(&self, style: FenStyle) -> String {
        let mut out = String::with_capacity(90);
        for r in (0..8).rev() {
            let mut empty = 0;
            for f in 0..8 {
                let sq = Square::from_file_rank(File::new(f), Rank::new(r));
                match self.piece_at(sq) {
                    Some(p) => {
                        if empty > 0 {
                            let _ = write!(out, "{empty}");
                            empty = 0;
                        }
                        out.push(p.to_char());
                    }
                    None => empty += 1,
                }
            }
            if empty > 0 {
                let _ = write!(out, "{empty}");
            }
            if r > 0 {
                out.push('/');
            }
        }
        out.push(' ');
        out.push(match self.side_to_move() {
            Colour::White => 'w',
            Colour::Black => 'b',
        });
        out.push(' ');
        self.write_castling_field(&mut out, style);
        out.push(' ');
        match self.ep_square() {
            Some(sq) => {
                let _ = write!(out, "{sq}");
            }
            None => out.push('-'),
        }
        let _ = write!(out, " {} {}", self.halfmove_clock(), self.fullmove_number());
        out
    }

    /// The castling field in slot order `KQkq`, or `-`.
    fn write_castling_field(&self, out: &mut String, style: FenStyle) {
        let rights = self.castling_rights();
        if rights.is_empty() {
            out.push('-');
            return;
        }
        for c in Colour::ALL {
            for s in CastleSide::ALL {
                if !rights.has(c, s) {
                    continue;
                }
                let rf = self.layout().rook_from[ci(c, s)]
                    .get()
                    .expect("held right has a rook");
                let letter = match style {
                    FenStyle::Shredder => rf.file().to_char(),
                    FenStyle::XFen => {
                        if self.rook_outside(c, s, rf) {
                            rf.file().to_char()
                        } else {
                            match s {
                                CastleSide::King => 'k',
                                CastleSide::Queen => 'q',
                            }
                        }
                    }
                };
                out.push(match c {
                    Colour::White => letter.to_ascii_uppercase(),
                    Colour::Black => letter,
                });
            }
        }
    }

    /// Whether another rook of `c` stands on the back rank outside the
    /// castling rook `rf` on side `s`: the condition under which X-FEN must
    /// name the file. A property of the position, not of the layout: an
    /// extra rook can arrive by promotion at any time.
    fn rook_outside(&self, c: Colour, s: CastleSide, rf: Square) -> bool {
        let rooks = self.pieces(c, PieceType::Rook) & Bitboard::rank(rf.rank());
        rooks.into_iter().any(|sq| match s {
            CastleSide::King => sq.file() > rf.file(),
            CastleSide::Queen => sq.file() < rf.file(),
        })
    }
}

/// The placement field: eight ranks, top first, digits for runs of empties.
fn parse_placement(field: &str) -> Result<[Option<Piece>; 64], FenError> {
    let mut mailbox = [None; 64];
    let mut ranks = field.split('/');
    for r in (0..8u8).rev() {
        let rank = ranks.next().ok_or(FenError::Placement)?;
        let mut f = 0u8;
        for ch in rank.chars() {
            if let Some(d) = ch.to_digit(10) {
                if !(1..=8).contains(&d) {
                    return Err(FenError::Placement);
                }
                f = f.checked_add(d as u8).ok_or(FenError::Placement)?;
            } else {
                let p = Piece::from_char(ch).ok_or(FenError::Placement)?;
                if f >= 8 {
                    return Err(FenError::Placement);
                }
                mailbox[Square::from_file_rank(File::new(f), Rank::new(r)).index()] = Some(p);
                f += 1;
            }
            if f > 8 {
                return Err(FenError::Placement);
            }
        }
        if f != 8 {
            return Err(FenError::Placement);
        }
    }
    if ranks.next().is_some() {
        return Err(FenError::Placement);
    }
    Ok(mailbox)
}

/// The castling field, in either notation, resolved against the placement.
///
/// `K`/`k`: the outermost rook of that colour on the king's side of its
/// king, on the back rank. `Q`/`q`: the outermost on the other side. A file
/// letter: that file on the back rank, which must hold the colour's rook and
/// must not be the king's file; the side follows from the file. Every right
/// requires the king on its back rank.
fn parse_castling(
    field: &str,
    mailbox: &[Option<Piece>; 64],
    kings: [Option<Square>; 2],
) -> Result<(CastlingRights, CastlingLayout), FenError> {
    let mut rights = CastlingRights::NONE;
    let mut rook_from = [OptSquare::NONE; 4];
    let mut king_from = [OptSquare::NONE; 2];
    if field == "-" {
        return Ok((rights, CastlingLayout::new(king_from, rook_from)));
    }
    for ch in field.chars() {
        let c = if ch.is_ascii_uppercase() {
            Colour::White
        } else if ch.is_ascii_lowercase() {
            Colour::Black
        } else {
            return Err(FenError::Castling);
        };
        let back = Rank::One.relative(c);
        let ksq = kings[c.index()].ok_or(FenError::Castling)?;
        if ksq.rank() != back {
            return Err(FenError::Castling);
        }
        let rook = Piece::new(c, PieceType::Rook);
        let rook_on =
            |file: File| mailbox[Square::from_file_rank(file, back).index()] == Some(rook);

        let (side, file) = match ch.to_ascii_lowercase() {
            'k' => {
                let file = File::ALL
                    .iter()
                    .rev()
                    .copied()
                    .find(|&f| f > ksq.file() && rook_on(f))
                    .ok_or(FenError::Castling)?;
                (CastleSide::King, file)
            }
            'q' => {
                let file = File::ALL
                    .iter()
                    .copied()
                    .find(|&f| f < ksq.file() && rook_on(f))
                    .ok_or(FenError::Castling)?;
                (CastleSide::Queen, file)
            }
            other => {
                let file = File::from_char(other).ok_or(FenError::Castling)?;
                if file == ksq.file() || !rook_on(file) {
                    return Err(FenError::Castling);
                }
                let side = if file > ksq.file() {
                    CastleSide::King
                } else {
                    CastleSide::Queen
                };
                (side, file)
            }
        };
        let rf = Square::from_file_rank(file, back);
        let i = ci(c, side);
        // The same right named twice must name the same rook.
        if let Some(prev) = rook_from[i].get()
            && prev != rf
        {
            return Err(FenError::Castling);
        }
        rook_from[i] = OptSquare::some(rf);
        king_from[c.index()] = OptSquare::some(ksq);
        rights = CastlingRights::from_bits(rights.bits() | CastlingRights::bit(c, side));
    }
    Ok((rights, CastlingLayout::new(king_from, rook_from)))
}
