// SPDX-License-Identifier: GPL-3.0-or-later

//! Nothing elsewhere in the search calls back into this file.

use std::io::Write;

use cadence_core::{Move, MoveList, generate_legal};

use super::depth::extension;
use super::{Expected, MAX_DEPTH, Search, Via};
use crate::level;
use crate::position::Position;
use crate::score::{self, ABORTED, DRAW, INFINITE, Score, mated_in};
use crate::time;

/// Centipawns either side of the last completed iteration's score; doubled on each failure.
const ASPIRATION_DELTA: Score = 24;

/// Below it the iterations are too cheap and their scores too unsettled for a window to pay.
const ASPIRATION_DEPTH: u32 = 4;

/// The plies a mate in `n` moves takes, held to the deepest the search can score so no count
/// overflows.
fn mate_plies(n: u32) -> Score {
    let plies = n.saturating_mul(2).saturating_sub(1).min(MAX_DEPTH);
    Score::try_from(plies).unwrap_or(Score::MAX)
}

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
            self.run.score = if board.in_check() { mated_in(0) } else { DRAW };
            // Said here, as `mate 0` where the formatter would write `mate -0`, so a GUI that reads
            // the score from the last `info` line has one.
            let said = if board.in_check() { "mate 0" } else { "cp 0" };
            let _ = writeln!(out, "info depth 0 score {said}");
            let _ = out.flush();
            self.wait_if_open_ended();
            self.publish_nodes();
            return Move::NULL;
        }
        let mut root_moves: Vec<Move> = legal.iter().collect();
        if root_moves.iter().any(|m| self.root_filter.contains(m)) {
            root_moves.retain(|m| self.root_filter.contains(m));
        }
        // The list is not empty, so the remainder is safe.
        let root_count = root_moves.len();
        root_moves.rotate_left(self.worker_index % root_count);

        let max_depth = self.limits.depth.unwrap_or(u32::MAX).clamp(1, MAX_DEPTH);
        let mut previous: Option<Score> = None;
        for depth in 1..=max_depth {
            self.root_depth = depth;
            // Per iteration, so it belongs to the depth printed beside it.
            self.run.seldepth = 0;
            // Each line skips the moves the lines before it took.
            let wanted = self.multipv.min(root_moves.len());
            self.run.lines.clear();
            let mut partial = (root_moves[0], -INFINITE);
            for _ in 0..wanted {
                let (best, score) =
                    self.aspirate(board, (&legal, &root_moves), depth, out, previous);
                if self.run.aborted {
                    partial = (best, score);
                    break;
                }
                self.keep_line(best, score);
            }
            if self.run.aborted {
                // With no completed iteration, the best fully searched root move stands, or the
                // first, so there is always a move.
                if self.run.completed_depth == 0 {
                    self.run.pv.clear();
                    // A finished line outranks the one the abort cut short, and keeps its reply.
                    if let Some(line) = self.run.lines.first() {
                        self.run.best = line.mv;
                        self.run.score = line.score;
                        self.run.pv.extend_from_slice(&line.pv);
                    } else {
                        let (best, score) = partial;
                        self.run.best = best;
                        self.run.score = if score == -INFINITE { ABORTED } else { score };
                        self.run.pv.push(best);
                    }
                }
                break;
            }
            self.run.completed_depth = depth;
            // Stable, so equal lines keep the root's order.
            self.run
                .lines
                .sort_by_key(|line| std::cmp::Reverse(line.score));
            let (best, score) = (self.run.lines[0].mv, self.run.lines[0].score);
            previous = Some(score);
            self.run.best = best;
            self.run.score = score;
            // Only accepted iterations reach here, so a run of equal moves is a run of completed
            // ones.
            self.run.roots.push((best, score));
            self.run.pv.clear();
            self.run.pv.extend_from_slice(&self.run.lines[0].pv);
            for number in 1..=self.run.lines.len().min(self.shown) {
                self.report(board, number, out);
            }
            // Best move first next time, which is what makes an aborted iteration's fallback good.
            if let Some(i) = root_moves.iter().position(|&m| m == best) {
                root_moves[..=i].rotate_right(1);
            }
            // A swap, not a copy: `lines` is cleared at the next iteration's head.
            std::mem::swap(&mut self.run.lines, &mut self.run.accepted);
            // `go mate`: a proved mate in that many moves or fewer answers the question asked.
            if let Some(n) = self.limits.mate
                && score >= score::MATE - mate_plies(n)
            {
                break;
            }
            // Before the budget is read, so the first clocked decision uses the new clock.
            self.absorb_ponder_hit();
            // An iteration has completed, so none is left running unbudgeted from a hit.
            self.clock.unbudgeted_iteration = false;
            if let Some(b) = self.clock.budget {
                let elapsed = self.elapsed_ms();
                self.run.iterations.push(elapsed);
                // The soft budget, scaled by how long the move has stood, says this move has had
                // its share; the prediction says the next iteration would not finish.
                let soft = time::soft_for_run(&self.limits, b, self.stable_iterations());
                if elapsed >= soft || !time::another_iteration_fits(&self.run.iterations, b) {
                    break;
                }
            }
        }
        self.wait_if_open_ended();
        self.publish_nodes();
        self.run.best
    }

    /// Called after `run`, so a level costs the search nothing. The set it drew from is reported
    /// with the move it plays.
    pub fn sample(
        &mut self,
        policy: level::Policy,
        board: &mut Position,
        out: &mut dyn Write,
    ) -> Move {
        if self.run.accepted.len() >= 2 {
            self.choose(policy, board);
        }
        self.report_level(board, out);
        self.run.best
    }

    fn choose(&mut self, policy: level::Policy, board: &Position) {
        let best = self.run.accepted[0].score;
        // Sorted, so the first line outside the margin ends the set.
        let admitted = self
            .run
            .accepted
            .iter()
            .take_while(|line| best - line.score <= policy.margin)
            .count()
            .max(1);
        let deficits: Vec<i32> = self.run.accepted[..admitted]
            .iter()
            .map(|line| best - line.score)
            .collect();
        // Never the process or the depth reached, which would make the reply depend on machine
        // speed; the move number makes a repetition a fresh draw.
        let seed = board.board().key() ^ u64::from(board.board().fullmove_number()).rotate_left(32);
        let chosen = level::choose(&deficits, policy.halving, seed);
        let line = &self.run.accepted[chosen];
        self.run.best = line.mv;
        self.run.score = line.score;
        self.run.pv.clear();
        self.run.pv.extend_from_slice(&line.pv);
    }

    /// Only where the root reports one line: at `MultiPV` above one the later lines are bounded
    /// above by the first, so a window centred on the previous best would fail low on them.
    fn aspirate(
        &mut self,
        board: &mut Position,
        root: (&MoveList, &[Move]),
        depth: u32,
        out: &mut dyn Write,
        previous: Option<Score>,
    ) -> (Move, Score) {
        let centre = previous
            .filter(|&p| self.multipv == 1 && depth >= ASPIRATION_DEPTH && !score::is_decisive(p));
        let Some(centre) = centre else {
            return self.search_root(board, root, depth, out, -INFINITE, INFINITE);
        };
        let mut delta = ASPIRATION_DELTA;
        let mut alpha = centre.saturating_sub(delta).max(-INFINITE);
        let mut beta = centre.saturating_add(delta).min(INFINITE);
        loop {
            let (best, score) = self.search_root(board, root, depth, out, alpha, beta);
            if self.run.aborted {
                return (best, score);
            }
            // A decided score or one past the evaluation's range opens that side fully.
            let wider = delta.saturating_mul(2);
            if score <= alpha && alpha > -INFINITE {
                // The far bound moves halfway toward the failed one, which narrows the re-search
                // without assuming the fail low will hold.
                beta = alpha + (beta - alpha) / 2;
                let far = score.saturating_sub(wider);
                alpha = if score::is_decisive(score) || far <= -score::MAX_EVAL {
                    -INFINITE
                } else {
                    far
                };
            } else if score >= beta && beta < INFINITE {
                let far = score.saturating_add(wider);
                beta = if score::is_decisive(score) || far >= score::MAX_EVAL {
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

    /// `root` is the root's legal list, which names a move for `currmove`, and the moves this line
    /// searches, in order.
    fn search_root(
        &mut self,
        board: &mut Position,
        root: (&MoveList, &[Move]),
        depth: u32,
        out: &mut dyn Write,
        mut alpha: Score,
        beta: Score,
    ) -> (Move, Score) {
        let (legal, moves) = root;
        // The root is checked and recorded as its children are.
        self.expect(0, depth, Expected::Pv, alpha, beta);
        self.visit(0);
        self.table.clear(0);
        // The root's window is always open, and its children read the flag one ply up.
        self.tt_pv[0] = true;
        let mut best = Move::NULL;
        let mut best_score = -INFINITE;
        // Not the list index once an earlier line has taken a move.
        let mut searched = 0;
        for (i, &m) in moves.iter().enumerate() {
            if self.run.lines.iter().any(|line| line.mv == m) {
                continue;
            }
            if searched == 0 {
                best = m;
            }
            self.name_current(m, i + 1, legal, out);
            self.make(board, 0, m);
            // The root extends a checking move like any node.
            let ext = extension(board.in_check(), 1, depth);
            let child = depth - 1 + ext;
            let mut score = if searched == 0 {
                let first = self.derive(Expected::Pv, Via::First);
                -self.negamax(board, child, 1, -beta, -alpha, first)
            } else {
                let later = self.derive(Expected::Pv, Via::Null);
                -self.negamax(board, child, 1, -alpha - 1, -alpha, later)
            };
            if searched > 0 && !self.run.aborted && score > alpha && score < beta {
                let open = self.derive(Expected::Pv, Via::Open);
                score = -self.negamax(board, child, 1, -beta, -alpha, open);
            }
            board.unmake_move(m);
            searched += 1;
            if self.run.aborted {
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
