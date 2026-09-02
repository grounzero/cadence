// SPDX-License-Identifier: GPL-3.0-or-later

//! Position state: `Board`, `StateInfo`, make/unmake. The mutation choke point.

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

/// Copy-make: the irreversible part of a position, snapshotted per ply. Everything else is
/// derived from the `Move` on the way back out, so `unmake_move` decrements a cursor and does
/// no hashing.
#[derive(Clone, Copy)]
pub struct StateInfo {
    /// The Zobrist key of the position. `zobrist` states what is mixed in and when.
    pub key: u64,
    /// The pawn-structure key: the piece-square keys of the pawns only. Maintained now, read by
    /// nothing until an evaluation exists; kept because it is what puts this struct on exactly
    /// one cache line.
    pub pawn_key: u64,
    pub rights: CastlingRights,
    /// Set after every double pawn push, whether or not a capture is possible. The Zobrist ep
    /// key is mixed in only when one is.
    pub ep: OptSquare,
    pub halfmove: u8,
    /// The piece the move into this state captured; `None` for a non-capture, and the *pawn*
    /// for en passant.
    pub captured: Option<Piece>,
    /// Plies since the last null move, or since the position was set up. Bounds the repetition
    /// scan: a null move flips the side to move without a real move, so a position on the far
    /// side of one is not a repetition of a position on the near side even when the keys agree.
    pub plies_from_null: u16,
    /// Pieces of the side **not** to move giving check to the side to move.
    pub checkers: Bitboard,
    /// `blockers[c]`: pieces of **either** colour that stand alone between an enemy slider and
    /// `c`'s king. Those of colour `c` are `c`'s pinned pieces; those of the other colour are
    /// its discovered-check candidates.
    pub blockers: [Bitboard; 2],
    /// `pinners[c]`: the sliders of the other colour that have exactly one piece between them
    /// and `c`'s king (the sliders behind `blockers[c]`).
    pub pinners: [Bitboard; 2],
}

impl StateInfo {
    /// The state of no position. `OptSquare` has no `Default` and gets none (a default "absent"
    /// square is exactly the kind of value that ends up standing in for a real one), so the
    /// stack is built from this constant.
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
// Exactly one 64-byte cache line, and that is the rule being applied, not "minimise bytes".
// `state()` is on the hottest path in the engine, so the two open questions about this struct
// both resolve against byte count: is 56 bytes, which does not fit a smaller line: it
// straddles, and roughly every entry of the stack after the first spans two lines. line.
const _: () = assert!(size_of::<StateInfo>() == 64);
const _: () = assert!(align_of::<StateInfo>() == 8);

// The per-ply stack is `[StateInfo; MAX_PLY + 1]`: 16 KiB, comfortably L2-resident.
const _: () = assert!(size_of::<StateInfo>() * (crate::MAX_PLY + 1) == 16_448);

/// What the FEN parser hands over: the placement and the irreversible state, already validated.
/// `Board::from_setup` does the rest.
pub(crate) struct Setup {
    pub mailbox: [Option<Piece>; 64],
    pub stm: Colour,
    pub rights: CastlingRights,
    pub ep: OptSquare,
    pub halfmove: u8,
    pub fullmove: u16,
    pub layout: CastlingLayout,
}

/// A chess position, with the search state stack and the game key history. **No `Clone`.** The
/// boxed state stack makes a copy quietly expensive (fine at position setup, catastrophic in a
/// loop), so there is no derive to reach for by accident.
pub struct Board {
    by_type: [Bitboard; 6],
    by_colour: [Bitboard; 2],
    /// `Option<Piece>` niche-packs, so this is 64 bytes.
    mailbox: [Option<Piece>; 64],
    stm: Colour,
    fullmove: u16,
    /// Index into `states`: the current search ply.
    ply: u16,
    /// Immutable for the life of the position; never in the undo record.
    layout: CastlingLayout,
    /// The search stack, indexed by ply. Bounded by `MAX_PLY`.
    states: Box<[StateInfo; crate::MAX_PLY + 1]>,
    /// Zobrist keys of the game so far, growing only on real game moves. Separate from `states`
    /// because a game outlives the search stack: `MAX_PLY` bounds search depth, not game
    /// length; a game routinely outgrows it.
    history: Vec<u64>,
}

impl Board {
    // --- construction -----------------------------------------------------

    /// Build a position from validated parts. The key, the pawn key, the checkers and the pin
    /// sets are computed here; the parser only places.
    pub(crate) fn from_setup(setup: &Setup) -> Board {
        // Built on the heap directly rather than materialised as a 16 KiB stack array and
        // moved. Once per position, at setup.
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

    /// The current per-ply snapshot.
    #[inline]
    #[must_use]
    pub fn state(&self) -> &StateInfo {
        &self.states[self.ply as usize]
    }

    #[inline]
    fn state_mut(&mut self) -> &mut StateInfo {
        &mut self.states[self.ply as usize]
    }

    /// The Zobrist key, maintained incrementally.
    #[inline]
    #[must_use]
    pub fn key(&self) -> u64 {
        self.state().key
    }

    /// The pawn-structure key, maintained incrementally.
    #[inline]
    #[must_use]
    pub fn pawn_key(&self) -> u64 {
        self.state().pawn_key
    }

    /// The Zobrist key recomputed from the board, ignoring the incremental one entirely,
    /// including the rule that the en-passant key is mixed in only when a capture is available.
    /// Exists for the fuzz test and for `debug_assert`s.
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

    /// The pawn key recomputed from the board.
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

    /// Every occupied square.
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

    /// Both colours of one piece type.
    #[inline]
    #[must_use]
    pub fn by_type(&self, pt: PieceType) -> Bitboard {
        self.by_type[pt.index()]
    }

    /// The squares held by one piece type of one colour.
    #[inline]
    #[must_use]
    pub fn pieces(&self, c: Colour, pt: PieceType) -> Bitboard {
        self.by_colour[c.index()] & self.by_type[pt.index()]
    }

    /// The piece on `sq`, if any.
    #[inline]
    #[must_use]
    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        self.mailbox[sq.index()]
    }

    /// # Panics
    ///
    /// If `c` has no king. `from_fen` guarantees exactly one, and no move
    /// can remove it: generation never offers a king as a target, in a
    /// position reachable by legal play or otherwise (`movegen`). The pair
    /// is the whole invariant, and it is why this returns a `Square` rather
    /// than an `OptSquare` that every caller in the hot path would have to
    /// unwrap.
    #[inline]
    #[must_use]
    pub fn king_square(&self, c: Colour) -> Square {
        self.pieces(c, PieceType::King)
            .lsb()
            .expect("a position always holds one king of each colour")
    }

    /// The pieces giving check to the side to move. The count is what move generation branches
    /// on: at two or more, no capture and no interposition can resolve both, and generation
    /// must restrict to king moves.
    #[inline]
    #[must_use]
    pub fn checkers(&self) -> Bitboard {
        self.state().checkers
    }

    /// Whether the side to move is in check.
    #[inline]
    #[must_use]
    pub fn in_check(&self) -> bool {
        self.state().checkers.any()
    }

    /// Whether the side **not** to move is in check. No position reachable by legal play is
    /// like this: it says the side to move could take a king.
    #[must_use]
    pub fn opponent_in_check(&self) -> bool {
        let them = self.stm.flip();
        (self.attackers_to(self.king_square(them), self.occupied()) & self.by_colour(self.stm))
            .any()
    }

    /// Pieces of either colour standing alone between an enemy slider and `c`'s king.
    /// `blockers(c) & by_colour(c)` are `c`'s pinned pieces.
    #[inline]
    #[must_use]
    pub fn blockers(&self, c: Colour) -> Bitboard {
        self.state().blockers[c.index()]
    }

    /// The enemy sliders with exactly one piece between them and `c`'s king.
    #[inline]
    #[must_use]
    pub fn pinners(&self, c: Colour) -> Bitboard {
        self.state().pinners[c.index()]
    }

    /// The en-passant square, if the last move was a double pawn push.
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

    /// The castling geometry, fixed at position setup.
    #[inline]
    #[must_use]
    pub fn layout(&self) -> &CastlingLayout {
        &self.layout
    }

    /// Plies since the last capture or pawn move.
    #[inline]
    #[must_use]
    pub fn halfmove_clock(&self) -> u8 {
        self.state().halfmove
    }

    /// The move number, incremented after each Black move.
    #[inline]
    #[must_use]
    pub fn fullmove_number(&self) -> u16 {
        self.fullmove
    }

    /// The current search ply: how many moves have been made without being unmade since setup.
    #[inline]
    #[must_use]
    pub fn ply(&self) -> usize {
        self.ply as usize
    }

    /// Zobrist keys of the positions the game passed through before this one, oldest first.
    /// Empty for a position set up from a FEN.
    #[inline]
    #[must_use]
    pub fn game_history(&self) -> &[u64] {
        &self.history
    }

    /// Plies since the last null move, or since setup if there has been none. See
    /// [`StateInfo::plies_from_null`].
    #[inline]
    #[must_use]
    pub fn plies_from_null(&self) -> usize {
        self.state().plies_from_null as usize
    }

    /// Whether the current position is a repetition: **twofold within the search tree,
    /// threefold against the game history**. One backward scan in two-ply steps over the
    /// logical key sequence `history ++ states[..=ply]`, bounded by the halfmove clock and by
    /// `plies_from_null`.
    #[must_use]
    pub fn is_repetition(&self) -> bool {
        let cur = self.key();
        let root = self.history.len();
        let current = root + self.ply as usize;

        // Nothing before the last irreversible move can recur, and nothing across a null move
        // counts: a null move flips the side to move without a real move, so a line through one
        // can land on an earlier position's key without the game having repeated anything.
        let bound = core::cmp::min(self.state().halfmove as usize, self.plies_from_null());

        let mut before_root = 0u32;
        // Step 2: the side to move is in the key, so positions an odd number of plies apart
        // never compare equal.
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

    /// The key at logical index `i` of the one sequence `history ++ states[..=ply]`. The only
    /// code that knows there are two containers.
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

    /// Attackers of BOTH colours to `sq` under a CALLER-SUPPLIED occupancy. The occupancy
    /// parameter is not a convenience: it is what makes castling legality (king and rook
    /// lifted), king-evasion legality (king lifted) and SEE correct.
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

    /// The castling-legality predicate: the right exists, `must_be_empty` is empty, and no
    /// square of the king's path is attacked with **both the king and the castling rook
    /// lifted** from the occupancy. Lifting the rook is necessary, not tidy: in
    /// `4k3/8/8/8/8/8/8/rRK5 w B` White is not in check because its own b1 rook blocks the a1
    /// rook, and castling would move that rook to d1 and leave the king on c1 exposed along the
    /// rank.
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

    /// Whether `m` gives check to the opponent. Computed as "is their king attacked by our
    /// pieces after the move", on updated piece sets and occupancy, without touching the board:
    /// two slider lookups plus the leapers.
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

    /// Play `m`, returning the accumulator delta. Infallible: the caller guarantees `m` came
    /// from `generate_legal`.
    ///
    /// # Panics
    ///
    /// If `from` is empty, or if the search stack is full (`MAX_PLY`).
    pub fn make_move(&mut self, m: Move) -> DirtyPieces {
        let us = self.stm;
        let them = us.flip();
        let from = m.from_sq();
        let to = m.to_sq();
        let mover = self.mailbox[from.index()].expect("make_move: no piece on the from square");
        let old = *self.state();

        // The ep key was mixed in for the old ep square iff we could take; undo that before the
        // board changes.
        let mut key = old.key;
        if let Some(ep) = old.ep.get() {
            key ^= self.ep_key(ep, us);
        }

        // Push the snapshot. From here the helpers XOR into the new slot.
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
            // Both origins cleared before either destination is set: the king may land on the
            // rook's origin, the rook on the king's, or the two may swap.
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

    /// Undo the last move. Decrements the state cursor and reverses the placement `m` already
    /// encodes.
    ///
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
            // Both destinations cleared before either origin is set.
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

    /// Play `m` as a **game** move rather than a search move. The current key is pushed onto
    /// the game history and the position after `m` becomes the root of the search stack:
    /// `ply()` is zero again and `game_history()` is one longer.
    ///
    /// # Panics
    ///
    /// In debug builds, if called with search moves still on the stack: a game move is a root
    /// operation.
    pub fn play(&mut self, m: Move) {
        debug_assert_eq!(self.ply, 0, "play with search moves on the stack");
        let key = self.key();
        self.make_move(m);
        // The new position's snapshot is in slot 1; it becomes the root.
        self.states[0] = self.states[self.ply as usize];
        self.ply = 0;
        self.history.push(key);
    }

    /// A copy of this position: placement, state stack at its current ply, and game history.
    /// Named rather than `Clone` because the boxed state stack makes the copy quietly expensive
    /// -- 16 KiB plus the history.
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

    /// Pass the move: flip the side to move, clear the en-passant square. The delta is always
    /// empty.
    ///
    /// # Panics
    ///
    /// If the search stack is full.
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
    // These three are the only code in the crate that touches `by_type`, `by_colour`,
    // `mailbox`, or the running key. They keep the four in step.

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

    /// The Zobrist ep key for `ep`, or zero when no pawn of `capturer` can take on it. That
    /// zero is what keeps the key a function of the position rather than of the move that
    /// reached it, and this is the one function both the incremental update and `recompute_key`
    /// call.
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

    /// Checkers for the side to move, and blockers/pinners for both kings.
    fn compute_check_info(&mut self) {
        let us = self.stm;
        let occ = self.occupied();
        let ksq = self.king_square(us);
        // The enemy king is *not* excluded, though it can only be in this set when the two
        // kings are adjacent, which no legal play reaches. "Attacked by the enemy king" is what
        // king safety means one line down in `movegen`, and the independent generator the
        // cross-check is run against decides legality the same way, by playing the move and
        // asking whether anything of theirs attacks our king.
        let checkers = self.attackers_to(ksq, occ) & self.by_colour(us.flip());
        let white = self.slider_blockers(Colour::White);
        let black = self.slider_blockers(Colour::Black);
        let st = self.state_mut();
        st.checkers = checkers;
        st.blockers = [white.0, black.0];
        st.pinners = [white.1, black.1];
    }

    /// `(blockers, pinners)` for `c`'s king: for each enemy slider aligned with the king on an
    /// empty board, the pieces between them under the real occupancy; exactly one piece there
    /// makes it a blocker and the slider a pinner. Full occupancy, so a slider standing behind
    /// a checking slider counts the checker as its blocker, which is what the remove-and-retest
    /// definition says.
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

/// The Shredder FEN, which is the whole position and unambiguous.
impl fmt::Debug for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Board({})", self.to_fen(crate::fen::FenStyle::Shredder))
    }
}
