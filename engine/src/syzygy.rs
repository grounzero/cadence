// SPDX-License-Identifier: GPL-3.0-or-later

//! Syzygy tables, probed through `shakmaty-syzygy` on a copy of the board in that crate's types.
//! Nothing is loaded until a path is set, so a search without one is the search it always was.

use std::sync::atomic::{AtomicU64, Ordering};

use cadence_core::position::Board;
use cadence_core::{Colour, Move, PieceType, generate_legal};
use shakmaty::{CastlingMode, Chess, FromSetup as _};
use shakmaty_syzygy::{MaybeRounded, Metric, SyzygyError, Tablebase};

/// The largest tables published, so the highest `SyzygyProbeLimit` that means anything.
pub const MAX_PROBE_LIMIT: usize = 7;

/// From the side to move's point of view, a cursed win and a blessed loss read as the draws the
/// fifty-move rule makes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wdl {
    Loss,
    Draw,
    Win,
}

/// One set, shared by every search thread a `go` starts.
pub struct Tables {
    tablebase: Tablebase<Chess>,
    files: usize,
    /// The smaller of the limit and the largest table added; zero turns probing off.
    cardinality: usize,
    hits: AtomicU64,
    failures: AtomicU64,
}

/// A root move's outcome for the side that plays it, under the fifty-move counter it is played at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Loss,
    /// A loss the counter saves.
    BlessedLoss,
    Draw,
    /// A win the counter may not allow.
    CursedWin,
    Win,
}

/// The root moves a table ranks best.
pub struct Ranked {
    pub moves: Vec<Move>,
    pub best: Rank,
    /// Ranked by distance to zeroing, so the result is settled and a probe in the search would only
    /// flatten the kept moves' scores into one band.
    pub decided: bool,
}

impl Tables {
    /// Every directory in `paths`, separated as UCI separates them: `;` on Windows and `:`
    /// elsewhere. One that cannot be read is named in the second value and the rest are kept.
    #[must_use]
    pub fn load(paths: &str, limit: usize) -> (Tables, Vec<String>) {
        let separator = if cfg!(windows) { ';' } else { ':' };
        let mut tablebase = Tablebase::new();
        let mut files = 0;
        let mut problems = Vec::new();
        for dir in paths
            .split(separator)
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            match tablebase.add_directory(dir) {
                Ok(n) => files += n,
                Err(e) => problems.push(format!("{dir}: {e}")),
            }
        }
        let cardinality = limit.min(tablebase.max_pieces());
        let tables = Tables {
            tablebase,
            files,
            cardinality,
            hits: AtomicU64::new(0),
            failures: AtomicU64::new(0),
        };
        (tables, problems)
    }

    #[must_use]
    pub fn files(&self) -> usize {
        self.files
    }

    #[must_use]
    pub fn cardinality(&self) -> usize {
        self.cardinality
    }

    /// Every probe answered since the last [`Tables::reset_hits`], by every thread.
    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// A missing table, an unreadable file or a corrupt one: each is no result and nothing else.
    #[must_use]
    pub fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    /// At each `go`, so `tbhits` counts that search's probes.
    pub fn reset_hits(&self) {
        self.hits.store(0, Ordering::Relaxed);
    }

    /// No castling right and no more pieces than the cardinality: a position a table can hold.
    #[must_use]
    pub fn covers(&self, board: &Board) -> bool {
        board.castling_rights().is_empty() && board.occupied().count() as usize <= self.cardinality
    }

    /// As though the position followed a capture or a pawn move, which the fifty-move rule makes
    /// exact only where it did.
    #[must_use]
    pub fn probe_wdl(&self, board: &Board) -> Option<Wdl> {
        let wdl = self.answered(Tablebase::probe_wdl_after_zeroing, board)?;
        Some(match wdl {
            shakmaty_syzygy::Wdl::Win => Wdl::Win,
            shakmaty_syzygy::Wdl::Loss => Wdl::Loss,
            _ => Wdl::Draw,
        })
    }

    /// Distance to zeroing in plies, negative where the side to move loses; a rounded distance may be
    /// one ply longer than it reads.
    #[must_use]
    pub fn probe_dtz(&self, board: &Board) -> Option<i32> {
        let found = self.answered(Tablebase::probe_dtz, board)?;
        Some(found.ignore_rounding().0)
    }

    /// The moves of `candidates`, or every legal move where it is empty, that a table ranks best.
    /// `None` where the root is outside the tables or a probe fails.
    #[must_use]
    pub fn rank_root(&self, board: &mut Board, candidates: &[Move]) -> Option<Ranked> {
        if !self.covers(board) {
            return None;
        }
        let moves: Vec<Move> = generate_legal(board)
            .iter()
            .filter(|m| candidates.is_empty() || candidates.contains(m))
            .collect();
        let (ranks, decided) = match self.ranks(board, &moves, true) {
            Ok(ranks) => (ranks, true),
            // Without a distance the ranking cannot settle the fifty-move rule, so the search keeps
            // probing.
            Err(Missing::Dtz) => (self.ranks(board, &moves, false).ok()?, false),
            Err(Missing::Other) => return None,
        };
        let best = ranks.iter().copied().max()?;
        let moves = moves
            .iter()
            .zip(&ranks)
            .filter(|&(_, &rank)| rank == best)
            .map(|(&m, _)| m)
            .collect();
        Some(Ranked {
            moves,
            best,
            decided,
        })
    }

    fn ranks(&self, board: &mut Board, moves: &[Move], dtz: bool) -> Result<Vec<Rank>, Missing> {
        let mut ranks = Vec::with_capacity(moves.len());
        for &m in moves {
            board.make_move(m);
            let rank = self.rank_after(board, dtz);
            board.unmake_move(m);
            ranks.push(rank?);
        }
        Ok(ranks)
    }

    /// `board` is the position after the move, so its side to move is the one that did not play it.
    fn rank_after(&self, board: &Board, dtz: bool) -> Result<Rank, Missing> {
        if generate_legal(board).is_empty() {
            return Ok(if board.in_check() {
                Rank::Win
            } else {
                Rank::Draw
            });
        }
        if board.is_repetition() {
            return Ok(Rank::Draw);
        }
        let pos = to_chess(board).ok_or(Missing::Other)?;
        if !dtz {
            let wdl = self.counted(self.tablebase.probe_wdl_after_zeroing(&pos))?;
            return Ok(match wdl {
                shakmaty_syzygy::Wdl::Loss => Rank::Win,
                shakmaty_syzygy::Wdl::BlessedLoss => Rank::CursedWin,
                shakmaty_syzygy::Wdl::Draw => Rank::Draw,
                shakmaty_syzygy::Wdl::CursedWin => Rank::BlessedLoss,
                shakmaty_syzygy::Wdl::Win => Rank::Loss,
            });
        }
        let found = self.counted(self.tablebase.probe_dtz(&pos))?;
        // A rounded distance may be one ply longer than it reads, so a win is kept only if that
        // longer one fits too.
        let (plies, rounded) = match found {
            MaybeRounded::Rounded(d) => (d.0, 1),
            MaybeRounded::Precise(d) => (d.0, 0),
        };
        let counter = i32::from(board.halfmove_clock());
        Ok(match plies {
            0 => Rank::Draw,
            p if p < 0 && counter - p + rounded <= 100 => Rank::Win,
            p if p < 0 => Rank::CursedWin,
            p if counter + p > 100 => Rank::BlessedLoss,
            _ => Rank::Loss,
        })
    }

    /// Converted, probed and counted; a position the conversion refuses is no result.
    fn answered<T>(
        &self,
        probe: impl FnOnce(&Tablebase<Chess>, &Chess) -> Result<T, SyzygyError>,
        board: &Board,
    ) -> Option<T> {
        let pos = to_chess(board)?;
        self.counted(probe(&self.tablebase, &pos)).ok()
    }

    fn counted<T>(&self, result: Result<T, SyzygyError>) -> Result<T, Missing> {
        match result {
            Ok(value) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Ok(value)
            }
            Err(e) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                Err(match e {
                    SyzygyError::MissingTable {
                        metric: Metric::Dtz,
                        ..
                    } => Missing::Dtz,
                    _ => Missing::Other,
                })
            }
        }
    }
}

/// Why a ranking stopped: a missing distance table falls back to the outcome alone.
enum Missing {
    Dtz,
    Other,
}

/// No castling rights: the caller has refused any position that keeps one. The crate validates
/// the rest, so an impossible position from a FEN is no result rather than a panic.
fn to_chess(board: &Board) -> Option<Chess> {
    let bits = |pt| shakmaty::Bitboard(board.by_type(pt).0);
    let by_role = shakmaty::ByRole {
        pawn: bits(PieceType::Pawn),
        knight: bits(PieceType::Knight),
        bishop: bits(PieceType::Bishop),
        rook: bits(PieceType::Rook),
        queen: bits(PieceType::Queen),
        king: bits(PieceType::King),
    };
    let by_color = shakmaty::ByColor {
        white: shakmaty::Bitboard(board.by_colour(Colour::White).0),
        black: shakmaty::Bitboard(board.by_colour(Colour::Black).0),
    };
    let mut setup = shakmaty::Setup::empty();
    setup.board = shakmaty::Board::try_from_bitboards(by_role, by_color).ok()?;
    setup.turn = match board.side_to_move() {
        Colour::White => shakmaty::Color::White,
        Colour::Black => shakmaty::Color::Black,
    };
    setup.ep_square = match board.ep_square() {
        Some(sq) => Some(shakmaty::Square::try_from(sq.index()).ok()?),
        None => None,
    };
    setup.halfmoves = u32::from(board.halfmove_clock());
    Chess::from_setup(setup, CastlingMode::Standard).ok()
}

#[cfg(test)]
mod tests {
    use cadence_core::FenStyle;
    use cadence_core::position::Board;
    use shakmaty::EnPassantMode;
    use shakmaty::fen::Fen;

    use super::to_chess;

    /// The move counter is left out: the conversion never carries it, since no table reads it.
    fn first_five(fen: &str) -> String {
        fen.split_whitespace().take(5).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn the_conversion_round_trips_both_colours_and_an_en_passant_square() {
        for fen in [
            "4k3/8/8/3pP3/8/8/8/4K3 w - d6 0 2",
            "4k3/8/8/8/3Pp3/8/8/4K3 b - d3 0 1",
            "8/8/8/3k4/8/8/8/K6Q b - - 37 60",
            "k7/8/1K6/8/8/8/7Q/8 w - - 0 1",
        ] {
            let board = Board::from_fen(fen).unwrap();
            let pos = to_chess(&board).unwrap_or_else(|| panic!("{fen}: refused"));
            let back = Fen::from_position(&pos, EnPassantMode::Always).to_string();
            assert_eq!(
                first_five(&back),
                first_five(&board.to_fen(FenStyle::XFen)),
                "{fen}"
            );
        }
    }

    /// The crate validates more than the FEN parser does: two knights cannot both have given the
    /// last check, and that position is no result rather than a panic.
    #[test]
    fn a_position_the_crate_refuses_is_no_result_and_no_panic() {
        let board = Board::from_fen("4k3/8/8/8/8/2n1n3/8/3K4 w - - 0 1").unwrap();
        assert!(to_chess(&board).is_none());
    }
}
