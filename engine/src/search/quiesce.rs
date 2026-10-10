// SPDX-License-Identifier: GPL-3.0-or-later

//! Out of check only noisy moves, in check every evasion.

use cadence_core::{MAX_PLY, Move, generate_legal, generate_noisy, has_legal_move};

use super::Search;
use crate::eval;
use crate::picker;
use crate::position::Position;
use crate::score::{ABORTED, DRAW, INFINITE, Score, mated_in};
use crate::see;

impl Search<'_> {
    /// Stands pat only out of check: a side in check must answer it.
    pub(super) fn quiesce(
        &mut self,
        board: &mut Position,
        ply: usize,
        mut alpha: Score,
        beta: Score,
    ) -> Score {
        self.visit(ply);
        self.table.clear(ply);
        if self.out_of_time() {
            return ABORTED;
        }
        if board.is_repetition() {
            return DRAW;
        }
        if ply >= MAX_PLY {
            return eval::evaluate(board);
        }

        let in_check = board.in_check();
        let (moves, mut best) = if in_check {
            let mut evasions = generate_legal(board);
            if evasions.is_empty() {
                return mated_in(ply);
            }
            // After the mate check: a mate on the hundredth half-move is a mate.
            if board.halfmove_clock() >= 100 {
                return DRAW;
            }
            // No killers and no history for the quiet evasions: whether they help is unmeasured.
            let generated_first = evasions.as_slice()[0];
            picker::sort_from(board, &mut evasions, 0, [Move::NULL; 2], &[]);
            // The head is what a cutoff here is bought with.
            self.evasion_lists += 1;
            if evasions.as_slice()[0] != generated_first {
                self.evasion_lists_reordered += 1;
            }
            (evasions, -INFINITE)
        } else {
            if board.halfmove_clock() >= 100 {
                return DRAW;
            }
            let stand_pat = eval::evaluate(board);
            if stand_pat >= beta {
                // A stand-pat cut on a stalemate would score it as the evaluation.
                return if has_legal_move(board) { beta } else { DRAW };
            }
            if stand_pat > alpha {
                alpha = stand_pat;
            }
            let mut noisy = generate_noisy(board);
            // A position with no legal move has no noisy one, so only an empty list is asked.
            if noisy.is_empty() && !has_legal_move(board) {
                return DRAW;
            }
            picker::sort_noisy(board, &mut noisy);
            (noisy, stand_pat)
        };

        for m in moves.iter() {
            // Out of check only: in check every move answers the check.
            if !in_check && see::see(board, m) < 0 {
                continue;
            }
            board.make_move(m);
            let score = -self.quiesce(board, ply + 1, -beta, -alpha);
            board.unmake_move(m);
            if self.aborted {
                return ABORTED;
            }
            if score > best {
                best = score;
                if score > alpha {
                    alpha = score;
                    if alpha >= beta {
                        break;
                    }
                }
            }
        }
        best
    }
}
