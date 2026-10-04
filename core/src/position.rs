// SPDX-License-Identifier: GPL-3.0-or-later

use crate::attacks;
use crate::bitboard::Bitboard;
use crate::castling::{CastleSide, CastlingLayout, CastlingRights, ci};
use crate::dirty::{DirtyPiece, DirtyPieces};
use crate::mv::Move;
use crate::types::{Colour, OptSquare, Piece, PieceType, PromoPiece, Square};
use crate::zobrist;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use core::mem::{align_of, size_of};

/// The irreversible state, one per ply, so `unmake_move` only decrements a cursor.
#[derive(Clone, Copy)]
pub struct StateInfo {
    pub key: u64,
    pub pawn_key: u64,
    pub rights: CastlingRights,
    /// Set after every double push; the key mixes it in only when a capture is possible.
    pub ep: OptSquare,
    pub halfmove: u8,
    /// The pawn, for en passant.
    pub captured: Option<Piece>,
    /// Bounds the repetition scan: a position across a null move is not a repetition even when the
    /// keys agree.
    pub plies_from_null: u16,
    pub checkers: Bitboard,
    /// Either colour: `c`'s own are pinned, the other side's are discovered-check candidates.
    pub blockers: [Bitboard; 2],
    pub pinners: [Bitboard; 2],
}

impl StateInfo {
    /// `OptSquare` has no `Default`, by design, so the stack starts from this.
    pub const EMPTY: StateInfo = StateInfo {
        key: 0,
        pawn_key: 0,
        rights: CastlingRights::NONE,
        ep: OptSquare::NONE,
        halfmove: 0,
        captured: None,
        plies_from_null: 0,
        checkers: Bitboard::EMPTY,
        blockers: [Bitboard::EMPTY; 2],
        pinners: [Bitboard::EMPTY; 2],
    };
}

// --- layout guards --------------------------------------------------------
// One 64-byte cache line: `state()` is on the hottest path.
const _: () = assert!(size_of::<StateInfo>() == 64);
const _: () = assert!(align_of::<StateInfo>() == 8);

// The stack stays L2-resident at 16 KiB.
const _: () = assert!(size_of::<StateInfo>() * (crate::MAX_PLY + 1) == 16_448);

pub(crate) struct Setup {
    pub mailbox: [Option<Piece>; 64],
    pub stm: Colour,
    pub rights: CastlingRights,
    pub ep: OptSquare,
    pub halfmove: u8,
    pub fullmove: u16,
    pub layout: CastlingLayout,
}

/// No `Clone`: the boxed state stack makes a copy quietly expensive.
pub struct Board {
    by_type: [Bitboard; 6],
    by_colour: [Bitboard; 2],
    mailbox: [Option<Piece>; 64],
    stm: Colour,
    fullmove: u16,
    ply: u16,
    /// Fixed for the life of the position, so never in the undo record.
    layout: CastlingLayout,
    states: Box<[StateInfo; crate::MAX_PLY + 1]>,
    /// Separate from `states` because a game can outgrow `MAX_PLY`.
    history: Vec<u64>,
}

impl Board {
    // --- construction -----------------------------------------------------

    pub(crate) fn from_setup(setup: &Setup) -> Board {
        // Straight onto the heap, not a 16 KiB stack array moved there.
        let states: Box<[StateInfo; crate::MAX_PLY + 1]> =
            match alloc::vec![StateInfo::EMPTY; crate::MAX_PLY + 1]
                .into_boxed_slice()
                .try_into()
            {
                Ok(states) => states,
                Err(_) => unreachable!("the vector has exactly MAX_PLY + 1 entries"),
            };
        let mut board = Board {
            by_type: [Bitboard::EMPTY; 6],
            by_colour: [Bitboard::EMPTY; 2],
            mailbox: [None; 64],
            stm: setup.stm,
            fullmove: setup.fullmove,
            ply: 0,
            layout: setup.layout,
            states,
            history: Vec::new(),
        };
        for sq in Square::all() {
            if let Some(p) = setup.mailbox[sq.index()] {
                board.put_piece(p, sq);
            }
        }
        let st = &mut board.states[0];
        st.rights = setup.rights;
        st.ep = setup.ep;
        st.halfmove = setup.halfmove;
        st.key ^= zobrist::castling(setup.rights);
        if setup.stm as u8 == Colour::Black as u8 {
            st.key ^= zobrist::side();
        }
        if let Some(ep) = setup.ep.get() {
            board.states[0].key ^= board.ep_key(ep, setup.stm);
        }
        board.compute_check_info();
        board
    }

    // --- accessors --------------------------------------------------------

    #[inline]
    #[must_use]
    pub fn state(&self) -> &StateInfo {
        &self.states[self.ply as usize]
    }

    #[inline]
    fn state_mut(&mut self) -> &mut StateInfo {
        &mut self.states[self.ply as usize]
    }

    #[inline]
    #[must_use]
    pub fn key(&self) -> u64 {
        self.state().key
    }

    #[inline]
    #[must_use]
    pub fn pawn_key(&self) -> u64 {
        self.state().pawn_key
    }

    /// From scratch, for the fuzz test and `debug_assert`s.
    #[must_use]
    pub fn recompute_key(&self) -> u64 {
        let mut key = 0;
        for sq in Square::all() {
            if let Some(p) = self.mailbox[sq.index()] {
                key ^= zobrist::piece(p, sq);
            }
        }
        if self.stm as u8 == Colour::Black as u8 {
            key ^= zobrist::side();
        }
        key ^= zobrist::castling(self.state().rights);
        if let Some(ep) = self.state().ep.get() {
            key ^= self.ep_key(ep, self.stm);
        }
        key
    }

    #[must_use]
    pub fn recompute_pawn_key(&self) -> u64 {
        let mut key = 0;
        for c in Colour::ALL {
            for sq in self.pieces(c, PieceType::Pawn) {
                key ^= zobrist::piece(Piece::new(c, PieceType::Pawn), sq);
            }
        }
        key
    }

    #[inline]
    #[must_use]
    pub fn side_to_move(&self) -> Colour {
        self.stm
    }

    #[inline]
    #[must_use]
    pub fn occupied(&self) -> Bitboard {
        self.by_colour[0] | self.by_colour[1]
    }

    #[inline]
    #[must_use]
    pub fn by_colour(&self, c: Colour) -> Bitboard {
        self.by_colour[c.index()]
    }

    #[inline]
    #[must_use]
    pub fn by_type(&self, pt: PieceType) -> Bitboard {
        self.by_type[pt.index()]
    }

    #[inline]
    #[must_use]
    pub fn pieces(&self, c: Colour, pt: PieceType) -> Bitboard {
        self.by_colour[c.index()] & self.by_type[pt.index()]
    }

    #[inline]
    #[must_use]
    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        self.mailbox[sq.index()]
    }

    /// # Panics
    ///
    /// If `c` has no king, which `from_fen` and legal play rule out.
    #[inline]
    #[must_use]
    pub fn king_square(&self, c: Colour) -> Square {
        self.pieces(c, PieceType::King)
            .lsb()
            .expect("a position always holds one king of each colour")
    }

    #[inline]
    #[must_use]
    pub fn checkers(&self) -> Bitboard {
        self.state().checkers
    }

    #[inline]
    #[must_use]
    pub fn in_check(&self) -> bool {
        self.state().checkers.any()
    }

    /// Never true in a position legal play reaches.
    #[must_use]
    pub fn opponent_in_check(&self) -> bool {
        let them = self.stm.flip();
        (self.attackers_to(self.king_square(them), self.occupied()) & self.by_colour(self.stm))
            .any()
    }

    #[inline]
    #[must_use]
    pub fn blockers(&self, c: Colour) -> Bitboard {
        self.state().blockers[c.index()]
    }

    #[inline]
    #[must_use]
    pub fn pinners(&self, c: Colour) -> Bitboard {
        self.state().pinners[c.index()]
    }

    #[inline]
    #[must_use]
    pub fn ep_square(&self) -> Option<Square> {
        self.state().ep.get()
    }

    #[inline]
    #[must_use]
    pub fn castling_rights(&self) -> CastlingRights {
        self.state().rights
    }

    #[inline]
    #[must_use]
    pub fn layout(&self) -> &CastlingLayout {
        &self.layout
    }

    #[inline]
    #[must_use]
    pub fn halfmove_clock(&self) -> u8 {
        self.state().halfmove
    }

    #[inline]
    #[must_use]
    pub fn fullmove_number(&self) -> u16 {
        self.fullmove
    }

    #[inline]
    #[must_use]
    pub fn ply(&self) -> usize {
        self.ply as usize
    }

    /// Oldest first; empty after a FEN.
    #[inline]
    #[must_use]
    pub fn game_history(&self) -> &[u64] {
        &self.history
    }

    #[inline]
    #[must_use]
    pub fn plies_from_null(&self) -> usize {
        self.state().plies_from_null as usize
    }

    /// Twofold inside the search tree, threefold against the game history.
    #[must_use]
    pub fn is_repetition(&self) -> bool {
        let cur = self.key();
        let root = self.history.len();
        let current = root + self.ply as usize;

        // Nothing before an irreversible move recurs, and nothing across a null move counts.
        let bound = core::cmp::min(self.state().halfmove as usize, self.plies_from_null());

        let mut before_root = 0u32;
        // The side to move is in the key, so odd distances never match.
        let mut d = 2;
        while d <= bound && d <= current {
            let i = current - d;
            if self.key_at(i) == cur {
                if i >= root {
                    return true; // twofold inside the tree
                }
                before_root += 1;
                if before_root == 2 {
                    return true; // this one and two before the root: threefold
                }
            }
            d += 2;
        }
        false
    }

    /// KN v KN and KB v KN are not dead: a mate exists, though neither side can force it.
    #[must_use]
    pub fn is_insufficient_material(&self) -> bool {
        const DARK: Bitboard = Bitboard(0xAA55_AA55_AA55_AA55);
        let others = self.occupied() & !self.by_type(PieceType::King);
        if others.count() <= 1 {
            let minors = self.by_type(PieceType::Knight) | self.by_type(PieceType::Bishop);
            return (others & !minors).is_empty();
        }
        let bishops = self.by_type(PieceType::Bishop);
        bishops == others && ((bishops & DARK).is_empty() || (bishops & !DARK).is_empty())
    }

    /// The only code that knows the sequence is two containers.
    #[inline]
    fn key_at(&self, i: usize) -> u64 {
        let h = self.history.len();
        if i < h {
            self.history[i]
        } else {
            self.states[i - h].key
        }
    }

    // --- attacks ----------------------------------------------------------

    /// Both colours, under the caller's occupancy, which castling, evasion and SEE each lift pieces
    /// from.
    #[must_use]
    pub fn attackers_to(&self, sq: Square, occ: Bitboard) -> Bitboard {
        // A White pawn attacks `sq` from the squares a Black pawn on `sq` would attack, and
        // vice versa.
        let pawns = (attacks::pawn_attacks(Colour::Black, sq)
            & self.pieces(Colour::White, PieceType::Pawn))
            | (attacks::pawn_attacks(Colour::White, sq)
                & self.pieces(Colour::Black, PieceType::Pawn));
        let queens = self.by_type(PieceType::Queen);
        pawns
            | (attacks::knight_attacks(sq) & self.by_type(PieceType::Knight))
            | (attacks::king_attacks(sq) & self.by_type(PieceType::King))
            | (attacks::rook_attacks(sq, occ) & (self.by_type(PieceType::Rook) | queens))
            | (attacks::bishop_attacks(sq, occ) & (self.by_type(PieceType::Bishop) | queens))
    }

    /// With king and rook both lifted: in `4k3/8/8/8/8/8/8/rRK5 w B` the castling rook is what
    /// shields the king from a1.
    #[must_use]
    pub fn can_castle(&self, c: Colour, s: CastleSide) -> bool {
        let i = ci(c, s);
        if !self.state().rights.has(c, s) {
            return false;
        }
        // Held rights always have their squares: the parser guarantees it.
        let (Some(kf), Some(rf)) = (
            self.layout.king_from[c.index()].get(),
            self.layout.rook_from[i].get(),
        ) else {
            return false;
        };
        if (self.occupied() & self.layout.must_be_empty[i]).any() {
            return false;
        }
        let occ = self.occupied().without(kf).without(rf);
        let them = self.by_colour(c.flip());
        for sq in self.layout.king_path[i] {
            if (self.attackers_to(sq, occ) & them).any() {
                return false;
            }
        }
        true
    }

    /// Without making the move.
    ///
    /// # Panics
    ///
    /// If `m.from_sq()` is empty.
    #[must_use]
    pub fn gives_check(&self, m: Move) -> bool {
        let us = self.stm;
        let them = us.flip();
        let ksq = self.king_square(them);
        let from = m.from_sq();
        let to = m.to_sq();
        let mut occ = self.occupied();
        let mut rq = self.pieces(us, PieceType::Rook) | self.pieces(us, PieceType::Queen);
        let mut bq = self.pieces(us, PieceType::Bishop) | self.pieces(us, PieceType::Queen);
        let mut knights = self.pieces(us, PieceType::Knight);
        let mut pawns = self.pieces(us, PieceType::Pawn);

        if m.is_castle() {
            let i = ci(us, m.castle_side());
            let (Some(kt), Some(rt)) = (self.layout.king_to[i].get(), self.layout.rook_to[i].get())
            else {
                return false;
            };
            occ = occ.without(from).without(to).with(kt).with(rt);
            rq = rq.without(to).with(rt);
        } else {
            occ = occ.without(from).with(to);
            if m.is_en_passant() {
                occ = occ.without(Square::new(to.index() as u8 ^ 8));
            }
            let mover = self.mailbox[from.index()].expect("a piece moves");
            match mover.piece_type() {
                PieceType::Pawn => {
                    pawns = pawns.without(from);
                    match m.promotion_piece().map(PromoPiece::piece_type) {
                        None => pawns = pawns.with(to),
                        Some(PieceType::Knight) => knights = knights.with(to),
                        Some(PieceType::Bishop) => bq = bq.with(to),
                        Some(PieceType::Rook) => rq = rq.with(to),
                        Some(_) => {
                            rq = rq.with(to);
                            bq = bq.with(to);
                        }
                    }
                }
                PieceType::Knight => knights = knights.without(from).with(to),
                PieceType::Bishop => bq = bq.without(from).with(to),
                PieceType::Rook => rq = rq.without(from).with(to),
                PieceType::Queen => {
                    rq = rq.without(from).with(to);
                    bq = bq.without(from).with(to);
                }
                PieceType::King => {}
            }
        }
        ((attacks::rook_attacks(ksq, occ) & rq)
            | (attacks::bishop_attacks(ksq, occ) & bq)
            | (attacks::knight_attacks(ksq) & knights)
            | (attacks::pawn_attacks(them, ksq) & pawns))
            .any()
    }

    // --- make / unmake ----------------------------------------------------

    /// `m` must come from `generate_legal`.
    ///
    /// # Panics
    ///
    /// If `from` is empty or the stack is full.
    pub fn make_move(&mut self, m: Move) -> DirtyPieces {
        let us = self.stm;
        let them = us.flip();
        let from = m.from_sq();
        let to = m.to_sq();
        let mover = self.mailbox[from.index()].expect("make_move: no piece on the from square");
        let old = *self.state();

        // The old ep key comes out before the board changes.
        let mut key = old.key;
        if let Some(ep) = old.ep.get() {
            key ^= self.ep_key(ep, us);
        }

        // From here the helpers XOR into the new slot.
        self.ply += 1;
        *self.state_mut() = StateInfo {
            key,
            pawn_key: old.pawn_key,
            rights: old.rights,
            ep: OptSquare::NONE,
            halfmove: old.halfmove.saturating_add(1),
            captured: None,
            plies_from_null: old.plies_from_null.saturating_add(1),
            checkers: Bitboard::EMPTY,
            blockers: [Bitboard::EMPTY; 2],
            pinners: [Bitboard::EMPTY; 2],
        };

        let mut dirty = DirtyPieces::EMPTY;

        if m.is_castle() {
            let i = ci(us, m.castle_side());
            let king = Piece::new(us, PieceType::King);
            let rook = Piece::new(us, PieceType::Rook);
            let (kf, rf) = (from, to);
            let kt = self.layout.king_to[i].get().expect("castling: king_to");
            let rt = self.layout.rook_to[i].get().expect("castling: rook_to");
            // Both origins cleared first: king and rook may land on each other's squares.
            self.remove_piece(king, kf);
            self.remove_piece(rook, rf);
            self.put_piece(king, kt);
            self.put_piece(rook, rt);
            if kf != kt {
                dirty.push(DirtyPiece::moved(king, kf, kt));
            }
            if rf != rt {
                dirty.push(DirtyPiece::moved(rook, rf, rt));
            }
        } else {
            if m.is_capture() {
                let (victim, victim_sq) = if m.is_en_passant() {
                    (
                        Piece::new(them, PieceType::Pawn),
                        Square::new(to.index() as u8 ^ 8),
                    )
                } else {
                    (self.mailbox[to.index()].expect("capture: no victim"), to)
                };
                self.remove_piece(victim, victim_sq);
                self.state_mut().captured = Some(victim);
                dirty.push(DirtyPiece::removed(victim, victim_sq));
            }
            if let Some(promo) = m.promotion_piece() {
                let promoted = Piece::new(us, promo.piece_type());
                self.remove_piece(mover, from);
                self.put_piece(promoted, to);
                dirty.push(DirtyPiece::removed(mover, from));
                dirty.push(DirtyPiece::added(promoted, to));
            } else {
                self.move_piece(mover, from, to);
                dirty.push(DirtyPiece::moved(mover, from, to));
            }
            if m.is_double_push() {
                let ep = Square::new(usize::midpoint(from.index(), to.index()) as u8);
                self.state_mut().ep = OptSquare::some(ep);
                let ep_key = self.ep_key(ep, them);
                self.state_mut().key ^= ep_key;
            }
        }

        if mover.piece_type() as u8 == PieceType::Pawn as u8 || m.is_capture() {
            self.state_mut().halfmove = 0;
        }

        let rights = old
            .rights
            .masked(self.layout.update_mask[from.index()] & self.layout.update_mask[to.index()]);
        if rights != old.rights {
            let st = self.state_mut();
            st.key ^= zobrist::castling(old.rights) ^ zobrist::castling(rights);
            st.rights = rights;
        }

        self.stm = them;
        self.state_mut().key ^= zobrist::side();
        if us as u8 == Colour::Black as u8 {
            self.fullmove += 1;
        }

        self.compute_check_info();
        dirty
    }

    /// # Panics
    ///
    /// In debug builds, if nothing has been made.
    pub fn unmake_move(&mut self, m: Move) {
        debug_assert!(self.ply > 0, "unmake_move with nothing made");
        let them = self.stm;
        let us = them.flip();
        let from = m.from_sq();
        let to = m.to_sq();

        if m.is_castle() {
            let i = ci(us, m.castle_side());
            let king = Piece::new(us, PieceType::King);
            let rook = Piece::new(us, PieceType::Rook);
            let kt = self.layout.king_to[i].get().expect("castling: king_to");
            let rt = self.layout.rook_to[i].get().expect("castling: rook_to");
            // Destinations first, as make clears origins first.
            self.remove_piece(king, kt);
            self.remove_piece(rook, rt);
            self.put_piece(king, from);
            self.put_piece(rook, to);
        } else {
            if let Some(promo) = m.promotion_piece() {
                self.remove_piece(Piece::new(us, promo.piece_type()), to);
                self.put_piece(Piece::new(us, PieceType::Pawn), from);
            } else {
                let mover = self.mailbox[to.index()].expect("unmake: no piece on the to square");
                self.move_piece(mover, to, from);
            }
            if let Some(victim) = self.state().captured {
                let victim_sq = if m.is_en_passant() {
                    Square::new(to.index() as u8 ^ 8)
                } else {
                    to
                };
                self.put_piece(victim, victim_sq);
            }
        }

        self.stm = us;
        if us as u8 == Colour::Black as u8 {
            self.fullmove -= 1;
        }
        self.ply -= 1;
    }

    /// A game move: the key joins the history and the result becomes the root.
    ///
    /// # Panics
    ///
    /// In debug builds, if search moves are still made.
    pub fn play(&mut self, m: Move) {
        debug_assert_eq!(self.ply, 0, "play with search moves on the stack");
        let key = self.key();
        self.make_move(m);
        // The new position's snapshot is in slot 1; it becomes the root.
        self.states[0] = self.states[self.ply as usize];
        self.ply = 0;
        self.history.push(key);
    }

    /// Named rather than `Clone`, because the copy is 16 KiB and the history.
    #[must_use]
    pub fn duplicate(&self) -> Board {
        Board {
            by_type: self.by_type,
            by_colour: self.by_colour,
            mailbox: self.mailbox,
            stm: self.stm,
            fullmove: self.fullmove,
            ply: self.ply,
            layout: self.layout,
            states: Box::new(*self.states),
            history: self.history.clone(),
        }
    }

    /// # Panics
    ///
    /// If the stack is full.
    pub fn make_null_move(&mut self) -> DirtyPieces {
        let us = self.stm;
        let old = *self.state();
        let mut key = old.key ^ zobrist::side();
        if let Some(ep) = old.ep.get() {
            key ^= self.ep_key(ep, us);
        }
        self.ply += 1;
        *self.state_mut() = StateInfo {
            key,
            pawn_key: old.pawn_key,
            rights: old.rights,
            ep: OptSquare::NONE,
            halfmove: old.halfmove.saturating_add(1),
            captured: None,
            plies_from_null: 0,
            checkers: Bitboard::EMPTY,
            blockers: [Bitboard::EMPTY; 2],
            pinners: [Bitboard::EMPTY; 2],
        };
        self.stm = us.flip();
        if us as u8 == Colour::Black as u8 {
            self.fullmove += 1;
        }
        self.compute_check_info();
        DirtyPieces::EMPTY
    }

    /// # Panics
    ///
    /// In debug builds, if nothing has been made.
    pub fn unmake_null_move(&mut self) {
        debug_assert!(self.ply > 0, "unmake_null_move with nothing made");
        self.stm = self.stm.flip();
        if self.stm as u8 == Colour::Black as u8 {
            self.fullmove -= 1;
        }
        self.ply -= 1;
    }

    // --- the mutation choke point -----------------------------------------
    // The only code that touches the piece sets, the mailbox and the running key.

    #[inline]
    fn put_piece(&mut self, p: Piece, sq: Square) {
        debug_assert!(
            self.mailbox[sq.index()].is_none(),
            "put_piece onto an occupied square"
        );
        self.by_type[p.piece_type().index()].set(sq);
        self.by_colour[p.colour().index()].set(sq);
        self.mailbox[sq.index()] = Some(p);
        let k = zobrist::piece(p, sq);
        let st = self.state_mut();
        st.key ^= k;
        if p.piece_type() as u8 == PieceType::Pawn as u8 {
            st.pawn_key ^= k;
        }
    }

    #[inline]
    fn remove_piece(&mut self, p: Piece, sq: Square) {
        debug_assert!(
            self.mailbox[sq.index()] == Some(p),
            "remove_piece of the wrong piece"
        );
        self.by_type[p.piece_type().index()].clear(sq);
        self.by_colour[p.colour().index()].clear(sq);
        self.mailbox[sq.index()] = None;
        let k = zobrist::piece(p, sq);
        let st = self.state_mut();
        st.key ^= k;
        if p.piece_type() as u8 == PieceType::Pawn as u8 {
            st.pawn_key ^= k;
        }
    }

    #[inline]
    fn move_piece(&mut self, p: Piece, from: Square, to: Square) {
        debug_assert!(
            self.mailbox[from.index()] == Some(p),
            "move_piece of the wrong piece"
        );
        debug_assert!(
            self.mailbox[to.index()].is_none(),
            "move_piece onto an occupied square"
        );
        let both = from.bb() | to.bb();
        self.by_type[p.piece_type().index()] ^= both;
        self.by_colour[p.colour().index()] ^= both;
        self.mailbox[from.index()] = None;
        self.mailbox[to.index()] = Some(p);
        let k = zobrist::piece(p, from) ^ zobrist::piece(p, to);
        let st = self.state_mut();
        st.key ^= k;
        if p.piece_type() as u8 == PieceType::Pawn as u8 {
            st.pawn_key ^= k;
        }
    }

    // --- derived state ----------------------------------------------------

    /// Zero when no pawn can take, which keeps the key a function of the position; `recompute_key`
    /// calls this too.
    #[inline]
    fn ep_key(&self, ep: Square, capturer: Colour) -> u64 {
        // A `capturer` pawn attacks `ep` from the squares an opposing pawn on `ep` would
        // attack.
        let takers =
            attacks::pawn_attacks(capturer.flip(), ep) & self.pieces(capturer, PieceType::Pawn);
        if takers.any() {
            zobrist::ep(ep.file())
        } else {
            0
        }
    }

    fn compute_check_info(&mut self) {
        let us = self.stm;
        let occ = self.occupied();
        let ksq = self.king_square(us);
        // The enemy king is not excluded: adjacent kings are unreachable, and the cross-check generator
        // decides legality the same way.
        let checkers = self.attackers_to(ksq, occ) & self.by_colour(us.flip());
        let white = self.slider_blockers(Colour::White);
        let black = self.slider_blockers(Colour::Black);
        let st = self.state_mut();
        st.checkers = checkers;
        st.blockers = [white.0, black.0];
        st.pinners = [white.1, black.1];
    }

    /// Full occupancy, so a slider behind a checking slider counts the checker as its blocker.
    fn slider_blockers(&self, c: Colour) -> (Bitboard, Bitboard) {
        let ksq = self.king_square(c);
        let them = c.flip();
        let occ = self.occupied();
        let queens = self.pieces(them, PieceType::Queen);
        let snipers = (attacks::rook_attacks(ksq, Bitboard::EMPTY)
            & (self.pieces(them, PieceType::Rook) | queens))
            | (attacks::bishop_attacks(ksq, Bitboard::EMPTY)
                & (self.pieces(them, PieceType::Bishop) | queens));
        let mut blockers = Bitboard::EMPTY;
        let mut pinners = Bitboard::EMPTY;
        for sniper in snipers {
            let between = attacks::between(ksq, sniper) & occ;
            if between.any() && !between.more_than_one() {
                blockers |= between;
                pinners.set(sniper);
            }
        }
        (blockers, pinners)
    }
}

/// Shredder FEN, the unambiguous spelling.
impl fmt::Debug for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Board({})", self.to_fen(crate::fen::FenStyle::Shredder))
    }
}
