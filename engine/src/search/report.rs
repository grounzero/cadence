// SPDX-License-Identifier: GPL-3.0-or-later

//! What a search reports as it runs, and the root lines it keeps for the report. The root
//! driver chooses its move from those kept lines, so `keep_line` is the one item here the search
//! reads back.

use std::io::Write;

use cadence_core::{Move, MoveList, generate_legal, to_uci};

use super::{CURRMOVE_AFTER_MS, RootLine, Search};
use crate::position::Position;
use crate::score::{self, Score};

impl Search<'_> {
    /// Name the root move about to be searched, and its place in the root list, once the search
    /// has been running for [`CURRMOVE_AFTER_MS`]. Nothing is written and no clock is read under
    /// a depth or node limit, which is the shape `bench` runs in.
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
        // No `depth` on this line, deliberately. A harness that picks iteration lines out of the
        // stream by their `info depth ` prefix would otherwise collect one of these per root
        // move.
        let _ = writeln!(
            out,
            "info currmove {} currmovenumber {number}",
            to_uci(m, legal, self.chess960)
        );
        let _ = out.flush();
    }

    /// Keep the line the root just returned, with the pv it ended on. Taken off
    /// [`PvTable`](super::pv::PvTable) here because the next line's search of the root clears
    /// row zero and writes its own.
    pub(super) fn keep_line(&mut self, mv: Move, score: Score) {
        let pv = self.table.line(0).to_vec();
        self.lines.push(RootLine { mv, score, pv });
    }

    /// One `info` line for line `number` of the iteration just completed, its pv spelled by
    /// walking it on the board so castling reads per the option. `multipv` is absent where only
    /// one line was asked for, which is the line every rating list and every test reads.
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
