// SPDX-License-Identifier: GPL-3.0-or-later

use cadence_core::{MAX_PLY, Move};

/// Row `ply` holds the best line from that ply. `MAX_PLY` squared moves, 128 KiB, on the heap.
pub(super) struct PvTable {
    rows: Box<[Move]>,
    len: [usize; MAX_PLY],
}

impl PvTable {
    pub(super) fn new() -> PvTable {
        PvTable {
            rows: vec![Move::NULL; MAX_PLY * MAX_PLY].into_boxed_slice(),
            len: [0; MAX_PLY],
        }
    }

    #[inline]
    pub(super) fn clear(&mut self, ply: usize) {
        if ply < MAX_PLY {
            self.len[ply] = 0;
        }
    }

    /// `m` is the new best at `ply`: row `ply` becomes `m` followed by row `ply + 1`.
    pub(super) fn update(&mut self, ply: usize, m: Move) {
        if ply >= MAX_PLY {
            return;
        }
        let (child_len, child_start) = if ply + 1 < MAX_PLY {
            (self.len[ply + 1], (ply + 1) * MAX_PLY)
        } else {
            (0, 0)
        };
        let row = ply * MAX_PLY;
        self.rows[row] = m;
        // Rows never overlap, so the copy is between disjoint ranges.
        let n = child_len.min(MAX_PLY - 1);
        for i in 0..n {
            self.rows[row + 1 + i] = self.rows[child_start + i];
        }
        self.len[ply] = n + 1;
    }

    pub(super) fn line(&self, ply: usize) -> &[Move] {
        let row = ply * MAX_PLY;
        &self.rows[row..row + self.len[ply]]
    }
}
