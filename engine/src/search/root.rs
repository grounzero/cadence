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
            let mut partial = (root_moves[0], -INFINITE, -INFINITE);
            for _ in 0..wanted {
                let (best, score, floor) =
                    self.aspirate(board, (&legal, &root_moves), depth, out, previous);
                if self.run.aborted {
                    partial = (best, score, floor);
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
                        let (best, score, _) = partial;
                        self.run.best = best;
                        self.run.score = if score == -INFINITE { ABORTED } else { score };
                        self.run.pv.push(best);
                    }
                } else if self.keep_unfinished(partial) {
                    self.report_unfinished(board, out);
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
            // A clocked ponder times its iterations too, so a hit decides on the whole record.
            if self.clock.budget.or(self.clock.budget_on_hit).is_some() {
                let elapsed = self.elapsed_ms();
                self.run.iterations.push(elapsed);
            }
            // Before the budget is read, so the first clocked decision uses the hit's budget.
            self.absorb_ponder_hit();
            // The soft budget, scaled by how long the move has stood, says this move has had its
            // share; the prediction says the next iteration would not finish.
            if let Some(b) = self.clock.budget
                && !time::starts_another(
                    &self.limits,
                    b,
                    &self.run.iterations,
                    self.stable_iterations(),
                    self.clock.hit_at,
                )
            {
                break;
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

    /// True where the unfinished iteration's move replaced the completed one's, which it does only
    /// where its score beat its window's floor and no ponderhit refused the iteration. At or under
    /// the floor it may be a fail low's bound, and a refused iteration is one a fresh search would
    /// not have run.
    fn keep_unfinished(&mut self, partial: (Move, Score, Score)) -> bool {
        self.run.unfinished = Some(partial);
        let (best, score, floor) = partial;
        if self.multipv > 1 || score <= floor || self.run.refused_at_hit {
            return false;
        }
        self.run.best = best;
        self.run.score = score;
        self.run.pv.clear();
        self.run.pv.extend_from_slice(self.table.line(0));
        true
    }

    /// Only where the root reports one line: at `MultiPV` above one the later lines are bounded
    /// above by the first, so a window centred on the previous best would fail low on them. The
    /// third value is the floor of the window the returned move was searched in.
    fn aspirate(
        &mut self,
        board: &mut Position,
        root: (&MoveList, &[Move]),
        depth: u32,
        out: &mut dyn Write,
        previous: Option<Score>,
    ) -> (Move, Score, Score) {
        let centre = previous
            .filter(|&p| self.multipv == 1 && depth >= ASPIRATION_DEPTH && !score::is_decisive(p));
        let Some(centre) = centre else {
            let (best, score) = self.search_root(board, root, depth, out, -INFINITE, INFINITE);
            return (best, score, -INFINITE);
        };
        let mut delta = ASPIRATION_DELTA;
        let mut alpha = centre.saturating_sub(delta).max(-INFINITE);
        let mut beta = centre.saturating_add(delta).min(INFINITE);
        loop {
            let (best, score) = self.search_root(board, root, depth, out, alpha, beta);
            if self.run.aborted {
                return (best, score, alpha);
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
                return (best, score, alpha);
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use cadence_core::position::Board;
    use cadence_core::{Move, START_FEN, generate_legal, parse_uci};

    use crate::search::Search;
    use crate::time::Budget;
    use crate::tt::Table;

    /// The completed iteration's move and the unfinished one's.
    fn two_moves() -> (Move, Move) {
        let board = Board::from_fen(START_FEN).expect("the start position");
        let legal = generate_legal(&board);
        let mv = |uci| parse_uci(&legal, uci).expect("a legal move");
        (mv("e2e4"), mv("d2d4"))
    }

    /// Three iterations completed on `completed`, ending 100, 200 and 400 ms in, and the unfinished
    /// iteration's line in the table.
    fn completed_three<'a>(
        stop: &'a AtomicBool,
        tt: &'a Table,
        (completed, unfinished): (Move, Move),
    ) -> Search<'a> {
        let mut search = Search::new(stop, tt);
        search.run.completed_depth = 3;
        search.run.best = completed;
        search.run.score = 20;
        search.run.pv.push(completed);
        search.run.roots = vec![(completed, 20); 3];
        search.run.iterations = vec![100, 200, 400];
        search.table.update(0, unfinished);
        search
    }

    /// The hit lands at once, and iterations doubling to 400 ms put the next one far past it plus
    /// the hard budget, so a fresh search would not have started it. Its move beat the floor, and
    /// is still not played.
    #[test]
    fn a_ponderhit_that_refuses_the_iteration_keeps_the_completed_move() {
        let moves = two_moves();
        let stop = AtomicBool::new(false);
        let hit = AtomicBool::new(true);
        let tt = Table::new(1).expect("a one mebibyte table");
        let mut search = completed_three(&stop, &tt, moves);
        search.set_ponder_hit(&hit);
        search.clock.pondering = true;
        search.clock.budget_on_hit = Some(Budget { soft: 10, hard: 30 });
        assert!(
            search.out_of_time(),
            "the hit refuses the iteration running"
        );
        search.keep_unfinished((moves.1, 50, 0));
        assert_eq!(search.run.best, moves.0);
        assert_eq!(search.run.pv, [moves.0]);
    }

    /// A stop and the hard deadline each play the unfinished move that beat its window's floor,
    /// with its line.
    #[test]
    fn a_stop_or_the_hard_deadline_plays_the_unfinished_move_that_beat_the_floor() {
        let moves = two_moves();
        let tt = Table::new(1).expect("a one mebibyte table");
        let stopped = AtomicBool::new(true);
        let running = AtomicBool::new(false);
        let mut by_stop = completed_three(&stopped, &tt, moves);
        let mut by_clock = completed_three(&running, &tt, moves);
        by_clock.clock.budget = Some(Budget { soft: 0, hard: 0 });
        for search in [&mut by_stop, &mut by_clock] {
            assert!(search.out_of_time(), "the limit stops the iteration");
            search.keep_unfinished((moves.1, 50, 0));
            assert_eq!(search.run.best, moves.1);
            assert_eq!(search.run.pv, [moves.1]);
        }
    }

    /// A different move at or under the floor may be a fail low's bound, so the completed move
    /// stands.
    #[test]
    fn a_stop_keeps_the_completed_move_over_a_different_one_at_or_under_the_floor() {
        let moves = two_moves();
        let tt = Table::new(1).expect("a one mebibyte table");
        let stop = AtomicBool::new(true);
        for score in [0, -30] {
            let mut search = completed_three(&stop, &tt, moves);
            assert!(search.out_of_time(), "the stop ends the iteration");
            search.keep_unfinished((moves.1, score, 0));
            assert_eq!(search.run.best, moves.0, "at {score} against a floor of 0");
            assert_eq!(search.run.pv, [moves.0]);
        }
    }
}
