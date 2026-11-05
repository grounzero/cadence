// SPDX-License-Identifier: GPL-3.0-or-later

//! Syzygy tables, probed through `shakmaty-syzygy` on a copy of the board in that crate's types.
//! Nothing is loaded until a path is set, so a search without one is the search it always was.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cadence_core::position::Board;
use cadence_core::{Colour, Move, PieceType, generate_legal};
use shakmaty::{CastlingMode, Chess, FromSetup as _};
use shakmaty_syzygy::filesystem::{Filesystem, OsFilesystem, RandomAccessFile};
use shakmaty_syzygy::{AmbiguousWdl, Dtz, MaybeRounded, Metric, SyzygyError, Tablebase};

use crate::score::{DRAW, Score, is_mate, tb_loss_in, tb_win_in};
use crate::uci::say::say;

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
    /// The tables a probe has failed on, each named once.
    named: Mutex<BTreeSet<String>>,
}

/// The crate keeps every table it opens for as long as the set lives, so a second open of a path
/// follows a failure: refused here, so a failed table costs one system call and not one per probe.
#[derive(Default)]
struct OpenOnce {
    os: OsFilesystem,
    opened: Mutex<BTreeSet<PathBuf>>,
}

impl Filesystem for OpenOnce {
    fn regular_file_size(&self, path: &Path) -> io::Result<u64> {
        self.os.regular_file_size(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.os.read_dir(path)
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn RandomAccessFile>> {
        let first = self
            .opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf());
        if !first {
            return Err(io::Error::other("failed before, so not opened again"));
        }
        self.os.open(path)
    }
}

/// A root move's outcome for the side that plays it, under the fifty-move counter it is played at,
/// ordered as the crate orders its outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Loss,
    /// A loss the counter may save, which the table cannot settle.
    MaybeLoss,
    /// A loss the counter saves.
    BlessedLoss,
    Draw,
    /// A win the counter does not allow.
    CursedWin,
    /// A win the counter may not allow, which the table cannot settle.
    MaybeWin,
    Win,
}

impl Rank {
    /// What a root of this rank reports: the search's own mate for the side the table says wins,
    /// and otherwise the table's verdict, a draw wherever the fifty-move counter makes one.
    #[must_use]
    pub fn reported(self, searched: Score) -> Score {
        match self {
            Rank::Win if is_mate(searched) && searched > 0 => searched,
            Rank::Win => tb_win_in(1),
            Rank::Loss if is_mate(searched) && searched < 0 => searched,
            Rank::Loss => tb_loss_in(1),
            // The counter decides these and the table cannot say how, so the search speaks.
            Rank::MaybeWin | Rank::MaybeLoss => searched,
            Rank::CursedWin | Rank::Draw | Rank::BlessedLoss => DRAW,
        }
    }

    /// The rank of a move into a position whose side to move is `found` plies from zeroing, the
    /// move leaving the counter at `counter`.
    #[must_use]
    pub fn from_distance(found: MaybeRounded<Dtz>, counter: u32) -> Rank {
        let (Dtz(plies), rounded) = match found {
            MaybeRounded::Rounded(d) => (d, true),
            MaybeRounded::Precise(d) => (d, false),
        };
        let to_move_wins = plies > 0;
        let total = plies.unsigned_abs().saturating_add(counter);
        // A rounded distance may be a ply longer than it reads, so one landing on the hundredth
        // half-move is either outcome; the crate says none lands there after zeroing, and this does
        // not rely on it.
        Rank::own(match total {
            _ if plies == 0 => AmbiguousWdl::Draw,
            100 if rounded && to_move_wins => AmbiguousWdl::MaybeWin,
            100 if rounded => AmbiguousWdl::MaybeLoss,
            t if t <= 100 && to_move_wins => AmbiguousWdl::Win,
            t if t <= 100 => AmbiguousWdl::Loss,
            _ if to_move_wins => AmbiguousWdl::CursedWin,
            _ => AmbiguousWdl::BlessedLoss,
        })
        .reversed()
    }

    /// The same from what a WDL table holds, the outcome for the side to move as though the move
    /// had zeroed the counter, which a counter run part-way leaves unsettled.
    #[must_use]
    pub fn from_outcome(wdl: shakmaty_syzygy::Wdl, counter: u32) -> Rank {
        use shakmaty_syzygy::Wdl;
        Rank::own(match wdl {
            Wdl::Win if counter >= 100 => AmbiguousWdl::CursedWin,
            Wdl::Win if counter > 0 => AmbiguousWdl::MaybeWin,
            Wdl::Loss if counter >= 100 => AmbiguousWdl::BlessedLoss,
            Wdl::Loss if counter > 0 => AmbiguousWdl::MaybeLoss,
            settled => AmbiguousWdl::from(settled),
        })
        .reversed()
    }

    /// The same outcome for the other side.
    fn reversed(self) -> Rank {
        match self {
            Rank::Loss => Rank::Win,
            Rank::MaybeLoss => Rank::MaybeWin,
            Rank::BlessedLoss => Rank::CursedWin,
            Rank::Draw => Rank::Draw,
            Rank::CursedWin => Rank::BlessedLoss,
            Rank::MaybeWin => Rank::MaybeLoss,
            Rank::Win => Rank::Loss,
        }
    }

    /// The side to move's own rank.
    fn own(outcome: AmbiguousWdl) -> Rank {
        match outcome {
            AmbiguousWdl::Loss => Rank::Loss,
            AmbiguousWdl::MaybeLoss => Rank::MaybeLoss,
            AmbiguousWdl::BlessedLoss => Rank::BlessedLoss,
            AmbiguousWdl::Draw => Rank::Draw,
            AmbiguousWdl::CursedWin => Rank::CursedWin,
            AmbiguousWdl::MaybeWin => Rank::MaybeWin,
            AmbiguousWdl::Win => Rank::Win,
        }
    }
}

/// The root moves a table ranks best.
pub struct Ranked {
    pub moves: Vec<Move>,
    pub best: Rank,
    /// What the root reports: `best`, or the root's own outcome where that is exact and the moves'
    /// ranks are not.
    pub verdict: Rank,
    /// Ranked by distance to zeroing, so the result is settled and a probe in the search would only
    /// flatten the kept moves' scores into one band.
    pub decided: bool,
}

impl Tables {
    /// Every directory in `paths`, separated as UCI separates them: `;` on Windows and `:`
    /// elsewhere. A directory or a file that cannot be read is named in the second value and the
    /// rest are kept.
    #[must_use]
    pub fn load(paths: &str, limit: usize) -> (Tables, Vec<String>) {
        let separator = if cfg!(windows) { ';' } else { ':' };
        let filesystem = Arc::new(OpenOnce::default());
        let mut tablebase = Tablebase::with_filesystem(filesystem.clone());
        let mut files = 0;
        let mut problems = Vec::new();
        for dir in paths
            .split(separator)
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            // File by file, since the crate's own scan of a directory stops at its first bad file.
            let mut entries = match filesystem.read_dir(Path::new(dir)) {
                Ok(entries) => entries,
                Err(e) => {
                    problems.push(format!("{dir}: {e}"));
                    continue;
                }
            };
            entries.sort();
            for entry in entries {
                match tablebase.add_file(&entry) {
                    Ok(()) => files += 1,
                    Err(e) if e.kind() == io::ErrorKind::InvalidInput => {}
                    Err(e) => problems.push(format!("{}: {e}", entry.display())),
                }
            }
        }
        let cardinality = limit.min(tablebase.max_pieces());
        let tables = Tables {
            tablebase,
            files,
            cardinality,
            hits: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            named: Mutex::new(BTreeSet::new()),
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
        // A root at a zeroed counter is read exactly by the WDL files, where its quiet moves are not.
        let unsettled = matches!(best, Rank::MaybeWin | Rank::MaybeLoss);
        let verdict = if unsettled && candidates.is_empty() && board.halfmove_clock() == 0 {
            self.answered(Tablebase::probe_wdl_after_zeroing, board)
                .map_or(best, |own| Rank::own(AmbiguousWdl::from(own)))
        } else {
            best
        };
        Some(Ranked {
            moves,
            best,
            verdict,
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
        let counter = u32::from(board.halfmove_clock());
        Ok(if dtz {
            Rank::from_distance(self.counted(self.tablebase.probe_dtz(&pos))?, counter)
        } else {
            Rank::from_outcome(
                self.counted(self.tablebase.probe_wdl_after_zeroing(&pos))?,
                counter,
            )
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
                self.name_once(&e);
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

    /// An `info string` the first time a probe fails on a table, from whichever thread it was,
    /// through the writer every UCI line takes.
    fn name_once(&self, e: &SyzygyError) {
        let table = match e {
            SyzygyError::MissingTable { metric, material }
            | SyzygyError::ProbeFailed {
                metric, material, ..
            } => format!("{metric} {material}"),
            other => other.to_string(),
        };
        let mut named = self.named.lock().unwrap_or_else(PoisonError::into_inner);
        if named.insert(table) {
            say(format_args!("info string Syzygy: {e}"));
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
