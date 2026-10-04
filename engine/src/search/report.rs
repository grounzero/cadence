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
        let reported = &self.lines[number - 1];
        let ms = self.elapsed_ms();
        let nps = self.reported_nodes() * 1000 / ms.max(1);
        let numbered = if self.multipv > 1 {
            format!(" multipv {number}")
        } else {
            String::new()
        };
        let mut line = format!(
            "info depth {} seldepth {}{numbered} score {} nodes {} nps {nps} hashfull {} time {ms} pv",
            self.completed_depth,
            self.seldepth,
            score::uci(reported.score),
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
