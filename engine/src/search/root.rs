// SPDX-License-Identifier: GPL-3.0-or-later

//! The root of a search: the iteration loop, the root moves' own searches, and the move it
//! returns or samples. Nothing elsewhere in the search calls back into this file.

use std::io::Write;

use cadence_core::{Move, MoveList, generate_legal};

use super::depth::extension;
use super::{MAX_DEPTH, Search};
use crate::level;
use crate::position::Position;
use crate::score::{DRAW, INFINITE, Score, mated_in};
use crate::time;

impl Search<'_> {
    /// The best move in `board`, or `Move::NULL` when there is none. Returns when the limits
    /// are met or `stop` is raised; under `infinite` and under a ponder nobody has hit, only
    /// when `stop` is raised.
    pub fn run(&mut self, board: &mut Position, out: &mut dyn Write) -> Move {
        // Here and not in `begin`, which is per worker rather than per search. A group of
        // workers advances the generation once between them, so the bump belongs to whoever
        // starts the group.
        self.tt.new_search();
        self.run_in_current_generation(board, out)
    }

    /// The same run for a worker whose caller has already advanced the generation once for the
    /// whole group. Everything else that separates a worker from a lone search arrived through
    /// `set_parallel`.
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
        // The list is never empty here, so the remainder is safe; a helper starting on a
        // different root move is the whole of the group's explicit divergence.
        let root_count = root_moves.len();
        root_moves.rotate_left(self.worker_index % root_count);

        let max_depth = self.limits.depth.unwrap_or(u32::MAX).clamp(1, MAX_DEPTH);
        for depth in 1..=max_depth {
            self.root_depth = depth;
            // Per iteration, so that the figure printed beside a depth belongs to it rather
            // than to the deepest line of any iteration before it.
            self.seldepth = 0;
            // One search of the root per line asked for, each skipping the moves the lines
            // before it took. A root with fewer moves than this reports the moves it has.
            let wanted = self.multipv.min(root_moves.len());
            self.lines.clear();
            let mut partial = (root_moves[0], -INFINITE);
            for _ in 0..wanted {
                let (best, score) = self.search_root(board, &legal, &root_moves, depth, out);
                if self.aborted {
                    partial = (best, score);
                    break;
                }
                self.keep_line(best, score);
            }
            if self.aborted {
                // The last completed iteration stands. If there is none, the best root move
                // fully searched so far does -- the first root move, if not even one was -- so
                // that there is always a move.
                if self.completed_depth == 0 {
                    // A line that finished outranks the one the abort cut short, and at
                    // `MultiPV` 1 there is never one to prefer.
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
            // Descending, and the sort is stable, so lines that scored
            // equally stay in the order the root found them.
            self.lines.sort_by_key(|line| std::cmp::Reverse(line.score));
            let (best, score) = (self.lines[0].mv, self.lines[0].score);
            self.best = best;
            self.score = score;
            // The iteration is accepted, so the pair it ended on joins the ones before it. An
            // aborted iteration breaks out above and leaves nothing, which is what makes a run
            // of equal moves here a run of completed ones.
            self.roots.push((best, score));
            self.pv.clear();
            self.pv.extend_from_slice(&self.lines[0].pv);
            for number in 1..=self.lines.len() {
                self.report(board, number, out);
            }
            // The best move first next time: what makes an aborted iteration's fallback -- the
            // previous iteration -- a good one, and the only ordering there is.
            if let Some(i) = root_moves.iter().position(|&m| m == best) {
                root_moves[..=i].rotate_right(1);
            }
            // The iteration stood, so its lines become the set a level samples
            // among. A swap rather than a copy: the vector left in `lines` is
            // cleared at the head of the next iteration.
            std::mem::swap(&mut self.lines, &mut self.accepted);
            // A hit may have arrived while this iteration ran, and the ladder below is read
            // against the origin it moves. Absorbed before the budget is consulted, so the
            // first clocked decision of the search is made on the new clock.
            self.absorb_ponder_hit();
            if let Some(b) = self.budget {
                let elapsed = self.elapsed_ms();
                self.iterations.push(elapsed);
                // Two reasons not to start another, and they are different reasons: the soft
                // budget says this move has had its share, and the prediction says the next
                // iteration would be abandoned unfinished at the hard budget and buy nothing.
                if elapsed >= b.soft || !time::another_iteration_fits(&self.iterations, b) {
                    break;
                }
            }
        }
        self.wait_if_open_ended();
        self.publish_nodes();
        self.best
    }

    /// Replace the best move with one sampled from the lines within `policy`'s
    /// margin, and return it. **Called by the caller after `run` has returned**,
    /// so the level is on no path inside the search and costs the default tree
    /// nothing, not even a field on this struct.
    pub fn sample(&mut self, policy: level::Policy, board: &Position) -> Move {
        if self.accepted.len() < 2 {
            return self.best;
        }
        let best = self.accepted[0].score;
        // The lines are sorted descending, so the first one outside the margin
        // ends the candidate set rather than being skipped over.
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
        // The seed is the position and the move number, never the process and
        // never the search's own depth, which would make the reply depend on how
        // fast the machine was. The move number is what makes a repetition of one
        // position a fresh draw rather than the same move twice.
        let seed = board.board().key() ^ u64::from(board.board().fullmove_number()).rotate_left(32);
        let chosen = level::choose(&deficits, policy.halving, seed);
        let line = &self.accepted[chosen];
        self.best = line.mv;
        self.score = line.score;
        self.pv.clear();
        self.pv.extend_from_slice(&line.pv);
        self.best
    }

    /// The root: every move a line before this one has not taken, the first in the full window
    /// and the rest in a null one, returning the best of them and its score. `MultiPV` 1 leaves
    /// nothing to skip, so the loop runs the whole list as it always has.
    fn search_root(
        &mut self,
        board: &mut Position,
        legal: &MoveList,
        moves: &[Move],
        depth: u32,
        out: &mut dyn Write,
    ) -> (Move, Score) {
        self.visit(0);
        self.table.clear(0);
        let mut alpha = -INFINITE;
        let beta = INFINITE;
        let mut best = Move::NULL;
        let mut best_score = -INFINITE;
        // The moves this root has searched, which is what the null window is conditioned on.
        // It is the index in the list only when no earlier line took anything.
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
            // A root move that gives check is extended like any other: the rule is about the
            // move, and the root has no claim on being the exception.
            let ext = extension(board.in_check(), 1, depth);
            let child = depth - 1 + ext;
            // The same rule as the interior nodes, and the root is where it is most nearly
            // free: the move in hand is the one the last iteration chose, and a root move that
            // beats it is what an iteration is looking for rather than what it expects. `beta`
            // here is `INFINITE`, so the second condition on the re-search is true whenever the
            // first is; it is written out because the rule is one rule.
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
                }
            }
        }
        (best, best_score)
    }
}
