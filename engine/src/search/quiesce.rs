// SPDX-License-Identifier: GPL-3.0-or-later

//! Out of check only noisy moves, in check every evasion. The table is read before the stand pat
//! and written as a bound by beta at depth 0.

use cadence_core::{MAX_PLY, Move, generate_legal, generate_noisy, has_legal_move};

use super::Search;
use crate::corrhist::CorrectionKeys;
use crate::picker;
use crate::position::Position;
use crate::score::{ABORTED, DRAW, INFINITE, Score, mated_in};
use crate::see;

impl<S: crate::search::Settings> Search<'_, S> {
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
        if board.is_repetition() || board.is_insufficient_material() {
            return DRAW;
        }
        if ply >= MAX_PLY {
            return self.static_eval(board);
        }

        // Before the stand pat, so a hit saves its evaluation too; a PV node takes no cutoff.
        let key = board.key();
        let pv = beta > alpha + 1;
        let (tt_move, cutoff, hit_pv, stored) = self.probe(board, key, 0, ply, alpha, beta);
        if let Some(score) = cutoff.filter(|_| !pv) {
            self.stats.quiescence_table_cutoffs.add_one();
            return score;
        }
        let pv = pv || hit_pv;

        let in_check = board.in_check();
        let (moves, mut best, eval) = if in_check {
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
            // The sort reads each noisy evasion's exchange once.
            let noisy = evasions.iter().filter(|m| m.is_noisy()).count();
            self.stats.evasion_exchanges.add(noisy as u64);
            picker::sort_evasions(board, &mut evasions);
            // The head is what a cutoff here is bought with.
            self.stats.evasion_lists.add_one();
            if evasions.as_slice()[0] != generated_first {
                self.stats.evasion_lists_reordered.add_one();
            }
            (evasions, -INFINITE, None)
        } else {
            if board.halfmove_clock() >= 100 {
                return DRAW;
            }
            // Corrected as the main search's evaluation is, by all three keys and the side to move.
            let correction = self
                .heuristics
                .corrhist
                .correction(&CorrectionKeys::of(board), board.side_to_move());
            self.stats.corrhist_applied.add_if(correction != 0);
            let raw = match stored {
                Some(eval) => self.table_eval(board, eval),
                None => self.static_eval(board),
            };
            let stand_pat = raw + correction;
            if stand_pat >= beta {
                // A stand-pat cut on a stalemate would score it as the evaluation.
                if !has_legal_move(board) {
                    return DRAW;
                }
                let value = if S::SOFT_STAND_PAT { stand_pat } else { beta };
                self.store_quiescence(board, key, ply, value, tt_move, beta, pv, Some(raw));
                return value;
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
            (noisy, stand_pat, Some(raw))
        };

        let mut best_move = Move::NULL;
        for m in moves.iter() {
            self.stats.quiescence_exchanges.add_if(!in_check);
            // Out of check only: in check every move answers the check.
            if !in_check && !see::see_ge(board, m, 0) {
                continue;
            }
            board.make_move(m);
            let score = -self.quiesce(board, ply + 1, -beta, -alpha);
            board.unmake_move(m);
            if self.run.aborted {
                return ABORTED;
            }
            if score > best {
                best = score;
                if score > alpha {
                    alpha = score;
                    best_move = m;
                    if alpha >= beta {
                        break;
                    }
                }
            }
        }
        // A node that raised nothing keeps the move the table already held.
        let mv = if best_move.is_null() {
            tt_move
        } else {
            best_move
        };
        self.store_quiescence(board, key, ply, best, mv, beta, pv, eval);
        best
    }
}
