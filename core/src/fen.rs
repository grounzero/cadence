// SPDX-License-Identifier: GPL-3.0-or-later

//! A position remembers which castling spelling it was given: round-tripped through the other, it
//! is a different position.

use alloc::string::String;
use core::fmt::Write as _;

use crate::bitboard::Bitboard;
use crate::castling::{CastleSide, CastlingLayout, CastlingRights, ci};
use crate::position::{Board, Setup};
use crate::types::{Colour, File, OptSquare, Piece, PieceType, Rank, Square};

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum FenError {
    /// Not four to six fields.
    Fields,
    Placement,
    SideToMove,
    /// The castling field named a rook that is not there, or a right that cannot exist given
    /// the king's square.
    Castling,
    /// Not `-` or a rank 3 or 6 square.
    EnPassant,
    Counter,
    /// Not exactly one king of each colour.
    Kings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FenStyle {
    /// `KQkq` names the outermost rook; the file is named only when another rook stands outside it.
    XFen,
    /// Always the rook's file.
    Shredder,
}

impl Board {
    /// # Errors
    ///
    /// The field that was rejected; a castling right without its rook is refused here rather than
    /// panicking in move generation.
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

    /// The ep field follows every double push, as the specification says.
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

    /// Read from the position, not the layout: a rook can arrive by promotion.
    fn rook_outside(&self, c: Colour, s: CastleSide, rf: Square) -> bool {
        let rooks = self.pieces(c, PieceType::Rook) & Bitboard::rank(rf.rank());
        rooks.into_iter().any(|sq| match s {
            CastleSide::King => sq.file() > rf.file(),
            CastleSide::Queen => sq.file() < rf.file(),
        })
    }
}

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

/// `K` and `k` mean the outermost rook on the king's side.
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
