// SPDX-License-Identifier: GPL-3.0-or-later

//! `keep_line` is the one item here the search reads back.

use std::io::Write;

use cadence_core::{Move, MoveList, generate_legal, to_uci};

use super::{CURRMOVE_AFTER_MS, RootLine, Search};
use crate::position::Position;
use crate::score::{self, Score};

impl Search<'_> {
    /// Under a depth or node limit nothing is written and no clock is read.
    pub(super) fn name_current(
        &self,
        m: Move,
        number: usize,
        legal: &MoveList,
        out: &mut dyn Write,
    ) {
        if self.budget.is_none() && !self.limits.infinite {
            return;
        }
        if self.elapsed_ms() < CURRMOVE_AFTER_MS {
            return;
        }
        // No `depth`: a harness picking lines by `info depth ` would collect these.
        let _ = writeln!(
            out,
            "info currmove {} currmovenumber {number}",
            to_uci(m, legal, self.chess960)
        );
        let _ = out.flush();
    }

    /// Taken now: the next line's search of the root clears the table's row zero.
    pub(super) fn keep_line(&mut self, mv: Move, score: Score) {
        let pv = self.table.line(0).to_vec();
        self.lines.push(RootLine { mv, score, pv });
    }

    /// `multipv` is left out at one line, which is what every rating list reads.
    pub(super) fn report(&self, board: &mut Position, number: usize, out: &mut dyn Write) {
        self.write_line(board, &self.lines[number - 1], number, out);
    }

    /// After `sample`, the set it drew from as `info string` lines, and the played move's own line
    /// where it is not the first, so the last main line a GUI reads is the move played.
    pub fn report_level(&self, board: &mut Position, out: &mut dyn Write) {
        let legal = generate_legal(board);
        for (i, line) in self.accepted.iter().enumerate() {
            let _ = writeln!(
                out,
                "info string candidate {} {} score {}",
                i + 1,
                to_uci(line.mv, &legal, self.chess960),
                score::uci(self.shown_score(line.score))
            );
        }
        let first = self.accepted.first().map(|line| line.mv);
        if first != Some(self.best)
            && let Some(played) = self.accepted.iter().find(|line| line.mv == self.best)
        {
            self.write_line(board, played, 1, out);
        }
        let _ = out.flush();
    }

    /// What the tables make of a line's score where they ranked the root.
    fn shown_score(&self, score: Score) -> Score {
        self.root_rank.map_or(score, |rank| rank.reported(score))
    }

    fn write_line(
        &self,
        board: &mut Position,
        reported: &RootLine,
        number: usize,
        out: &mut dyn Write,
    ) {
        let ms = self.elapsed_ms();
        let nps = self.reported_nodes() * 1000 / ms.max(1);
        let numbered = if self.shown > 1 {
            format!(" multipv {number}")
        } else {
            String::new()
        };
        // Only with tables loaded, so every line a run without them prints is the line it was.
        let tbhits = self
            .tables
            .map_or(String::new(), |tables| format!(" tbhits {}", tables.hits()));
        let mut line = format!(
            "info depth {} seldepth {}{numbered} score {} nodes {} nps {nps} hashfull {}{tbhits} time {ms} pv",
            self.completed_depth,
            self.seldepth,
            score::uci(self.shown_score(reported.score)),
            self.reported_nodes(),
            self.tt.hashfull()
        );
        let mut made = 0;
        for &m in &reported.pv {
            let legal = generate_legal(board);
            if !legal.contains(m) {
                break;
            }
            line.push(' ');
            line.push_str(&to_uci(m, &legal, self.chess960));
            board.make_move(m);
            made += 1;
        }
        for &m in reported.pv[..made].iter().rev() {
            board.unmake_move(m);
        }
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}
