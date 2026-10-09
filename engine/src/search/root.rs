// SPDX-License-Identifier: GPL-3.0-or-later

//! Nothing elsewhere in the search calls back into this file.

use std::io::Write;

use cadence_core::{Move, MoveList, generate_legal};

use super::depth::extension;
use super::{MAX_DEPTH, Search};
use crate::level;
use crate::position::Position;
use crate::score::{self, DRAW, INFINITE, Score, mated_in};
use crate::time;

/// Centipawns either side of the last completed iteration's score; doubled on each failure.
const ASPIRATION_DELTA: Score = 24;

/// Below it the iterations are too cheap and their scores too unsettled for a window to pay.
const ASPIRATION_DEPTH: u32 = 4;

impl Search<'_> {
    /// `Move::NULL` when there is none. Under `infinite` or an unhit ponder it returns only on
    /// `stop`.
    pub fn run(&mut self, board: &mut Position, out: &mut dyn Write) -> Move {
        // Once per group, which is why not in `begin`.
        self.tt.new_search();
        self.run_in_current_generation(board, out)
    }

    /// For a worker whose caller has advanced the generation for the group.
    pub(crate) fn run_in_current_generation(
        &mut self,
        board: &mut Position,
        out: &mut dyn Write,
    ) -> Move {
        self.begin(board);

        let legal = generate_legal(board);
        if legal.is_empty() {
            self.score = if board.in_check() { mated_in(0) } else { DRAW };
            self.wait_if_open_ended();
            self.publish_nodes();
            return Move::NULL;
        }
        let mut root_moves: Vec<Move> = legal.iter().collect();
        // The list is not empty, so the remainder is safe.
        let root_count = root_moves.len();
        root_moves.rotate_left(self.worker_index % root_count);

        let max_depth = self.limits.depth.unwrap_or(u32::MAX).clamp(1, MAX_DEPTH);
        let mut previous: Option<Score> = None;
        for depth in 1..=max_depth {
            self.root_depth = depth;
            // Per iteration, so it belongs to the depth printed beside it.
            self.seldepth = 0;
            // Each line skips the moves the lines before it took.
            let wanted = self.multipv.min(root_moves.len());
            self.lines.clear();
            let mut partial = (root_moves[0], -INFINITE);
            for _ in 0..wanted {
                let (best, score) = self.aspirate(board, &legal, &root_moves, depth, out, previous);
                if self.aborted {
                    partial = (best, score);
                    break;
                }
                self.keep_line(best, score);
            }
            if self.aborted {
                // With no completed iteration, the best fully searched root move stands, or the
                // first, so there is always a move.
                if self.completed_depth == 0 {
                    // A finished line outranks the one the abort cut short.
                    let (best, score) = match self.lines.first() {
                        Some(line) => (line.mv, line.score),
                        None => partial,
                    };
                    self.best = best;
                    self.score = if score == -INFINITE { DRAW } else { score };
                    self.pv.clear();
                    self.pv.push(best);
                }
                break;
            }
            self.completed_depth = depth;
            // Stable, so equal lines keep the root's order.
            self.lines.sort_by_key(|line| std::cmp::Reverse(line.score));
            let (best, score) = (self.lines[0].mv, self.lines[0].score);
            previous = Some(score);
            self.best = best;
            self.score = score;
            // Only accepted iterations reach here, so a run of equal moves is a run of completed
            // ones.
            self.roots.push((best, score));
            self.pv.clear();
            self.pv.extend_from_slice(&self.lines[0].pv);
            for number in 1..=self.lines.len() {
                self.report(board, number, out);
            }
            // Best move first next time, which is what makes an aborted iteration's fallback good.
            if let Some(i) = root_moves.iter().position(|&m| m == best) {
                root_moves[..=i].rotate_right(1);
            }
            // A swap, not a copy: `lines` is cleared at the next iteration's head.
            std::mem::swap(&mut self.lines, &mut self.accepted);
            // Before the budget is read, so the first clocked decision uses the new clock.
            self.absorb_ponder_hit();
            if let Some(b) = self.budget {
                let elapsed = self.elapsed_ms();
                self.iterations.push(elapsed);
                // The soft budget says this move has had its share; the prediction says the next
                // iteration would not finish.
                if elapsed >= b.soft || !time::another_iteration_fits(&self.iterations, b) {
                    break;
                }
            }
        }
        self.wait_if_open_ended();
        self.publish_nodes();
        self.best
    }

    /// Called after `run`, so a level costs the search nothing.
    pub fn sample(&mut self, policy: level::Policy, board: &Position) -> Move {
        if self.accepted.len() < 2 {
            return self.best;
        }
        let best = self.accepted[0].score;
        // Sorted, so the first line outside the margin ends the set.
        let admitted = self
            .accepted
            .iter()
            .take_while(|line| best - line.score <= policy.margin)
            .count()
            .max(1);
        let deficits: Vec<i32> = self.accepted[..admitted]
            .iter()
            .map(|line| best - line.score)
            .collect();
        // Never the process or the depth reached, which would make the reply depend on machine
        // speed; the move number makes a repetition a fresh draw.
        let seed = board.board().key() ^ u64::from(board.board().fullmove_number()).rotate_left(32);
        let chosen = level::choose(&deficits, policy.halving, seed);
        let line = &self.accepted[chosen];
        self.best = line.mv;
        self.score = line.score;
        self.pv.clear();
        self.pv.extend_from_slice(&line.pv);
        self.best
    }

    /// Only where the root reports one line: at `MultiPV` above one the later lines are bounded
    /// above by the first, so a window centred on the previous best would fail low on them.
    fn aspirate(
        &mut self,
        board: &mut Position,
        legal: &MoveList,
        moves: &[Move],
        depth: u32,
        out: &mut dyn Write,
        previous: Option<Score>,
    ) -> (Move, Score) {
        let centre = previous
            .filter(|&p| self.multipv == 1 && depth >= ASPIRATION_DEPTH && !score::is_mate(p));
        let Some(centre) = centre else {
            return self.search_root(board, legal, moves, depth, out, -INFINITE, INFINITE);
        };
        let mut delta = ASPIRATION_DELTA;
        let mut alpha = centre.saturating_sub(delta).max(-INFINITE);
        let mut beta = centre.saturating_add(delta).min(INFINITE);
        loop {
            let (best, score) = self.search_root(board, legal, moves, depth, out, alpha, beta);
            if self.aborted {
                return (best, score);
            }
            // A mate score or one past the evaluation's range opens that side fully.
            let wider = delta.saturating_mul(2);
            if score <= alpha && alpha > -INFINITE {
                // The far bound moves halfway toward the failed one, which narrows the re-search
                // without assuming the fail low will hold.
                beta = alpha + (beta - alpha) / 2;
                let far = score.saturating_sub(wider);
                alpha = if score::is_mate(score) || far <= -score::MAX_EVAL {
                    -INFINITE
                } else {
                    far
                };
            } else if score >= beta && beta < INFINITE {
                let far = score.saturating_add(wider);
                beta = if score::is_mate(score) || far >= score::MAX_EVAL {
                    INFINITE
                } else {
                    far
                };
            } else {
                return (best, score);
            }
            delta = wider;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn search_root(
        &mut self,
        board: &mut Position,
        legal: &MoveList,
        moves: &[Move],
        depth: u32,
        out: &mut dyn Write,
        mut alpha: Score,
        beta: Score,
    ) -> (Move, Score) {
        self.visit(0);
        self.table.clear(0);
        let mut best = Move::NULL;
        let mut best_score = -INFINITE;
        // Not the list index once an earlier line has taken a move.
        let mut searched = 0;
        for (i, &m) in moves.iter().enumerate() {
            if self.lines.iter().any(|line| line.mv == m) {
                continue;
            }
            if searched == 0 {
                best = m;
            }
            self.name_current(m, i + 1, legal, out);
            board.make_move(m);
            // The root extends a checking move like any node.
            let ext = extension(board.in_check(), 1, depth);
            let child = depth - 1 + ext;
            let mut score = if searched == 0 {
                -self.negamax(board, child, 1, -beta, -alpha)
            } else {
                -self.negamax(board, child, 1, -alpha - 1, -alpha)
            };
            if searched > 0 && !self.aborted && score > alpha && score < beta {
                score = -self.negamax(board, child, 1, -beta, -alpha);
            }
            board.unmake_move(m);
            searched += 1;
            if self.aborted {
                break;
            }
            if score > best_score {
                best_score = score;
                best = m;
                if score > alpha {
                    alpha = score;
                    self.table.update(0, m);
                    if alpha >= beta {
                        break;
                    }
                }
            }
        }
        (best, best_score)
    }
}
