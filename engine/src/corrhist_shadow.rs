// SPDX-License-Identifier: GPL-3.0-or-later

//! A shadow of the correction table that learns under other lifetimes and decides nothing.
//! It is an instrument on a branch that never merges, and its counters print on `shadow`.

use std::sync::Mutex;

use cadence_core::{Colour, Move};

use crate::corrhist::{CorrectionHistory, MAX_DELTA};
use crate::score::{self, Score};
use crate::tt::Bound;

/// Raw, the cleared copy, the kept copy, the kept placebo, the cleared placebo.
const ARMS: usize = 5;
const NAMES: [&str; ARMS] = ["raw", "cleared", "kept", "placebo_kept", "placebo_cleared"];
/// Every scored node, and the nodes whose kept entry was last written by an earlier search.
const POPS: usize = 2;
const POP_NAMES: [&str; POPS] = ["all", "cross"];
const PLIES: usize = cadence_core::MAX_PLY + 1;

struct Shadow {
    cleared: CorrectionHistory,
    kept: CorrectionHistory,
    placebo_kept: CorrectionHistory,
    placebo_cleared: CorrectionHistory,
    /// The search that last wrote each kept slot, so a read can say whether it crossed a move.
    kept_written: Vec<u32>,
    search: u32,
    /// Per ply, read at the node's preamble: raw evaluation, each arm's correction, and whether
    /// the kept entry read there was last written by an earlier search.
    raw: Vec<Option<Score>>,
    corr: Vec<[Score; ARMS]>,
    cross: Vec<bool>,
    reads: u64,
    live_mismatch: u64,
    n: [u64; POPS],
    sdd: [i128; POPS],
    sse: [[i128; ARMS]; POPS],
    sdc: [[i128; ARMS]; POPS],
    scc: [[i128; ARMS]; POPS],
}

static SHADOW: Mutex<Option<Shadow>> = Mutex::new(None);

/// The placebo keys every position to one slot a side, which is the table knowing nothing.
const PLACEBO_KEY: u64 = 0;

fn kept_slot(pawn_key: u64, side: Colour) -> usize {
    side.index() * (1 << 14) + (pawn_key % (1 << 14)) as usize
}

fn with<R>(f: impl FnOnce(&mut Shadow) -> R) -> R {
    let mut guard = SHADOW
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let shadow = guard.get_or_insert_with(|| Shadow {
        cleared: CorrectionHistory::new(),
        kept: CorrectionHistory::new(),
        placebo_kept: CorrectionHistory::new(),
        placebo_cleared: CorrectionHistory::new(),
        kept_written: vec![u32::MAX; 2 << 14],
        search: 0,
        raw: vec![None; PLIES],
        corr: vec![[0; ARMS]; PLIES],
        cross: vec![false; PLIES],
        reads: 0,
        live_mismatch: 0,
        n: [0; POPS],
        sdd: [0; POPS],
        sse: [[0; ARMS]; POPS],
        sdc: [[0; ARMS]; POPS],
        scc: [[0; ARMS]; POPS],
    });
    f(shadow)
}

/// `ucinewgame`: every table starts empty, as the live one does with a new `Search`.
pub fn new_game() {
    with(|s| {
        s.kept.clear();
        s.placebo_kept.clear();
        s.kept_written.fill(u32::MAX);
    });
}

/// Every `go`: the cleared arms forget, the kept arms do not.
pub fn new_search() {
    with(|s| {
        s.cleared.clear();
        s.placebo_cleared.clear();
        s.search = s.search.wrapping_add(1);
    });
}

/// At the node's preamble, before the node folds anything in, so every reading is out of sample.
pub fn read(ply: usize, pawn_key: u64, side: Colour, raw: Option<Score>, live: Score) {
    with(|s| {
        let Some(raw) = raw else {
            s.raw[ply] = None;
            return;
        };
        let cleared = s.cleared.correction(pawn_key, side);
        s.reads += 1;
        s.live_mismatch += u64::from(cleared != live);
        s.raw[ply] = Some(raw);
        s.corr[ply] = [
            0,
            cleared,
            s.kept.correction(pawn_key, side),
            s.placebo_kept.correction(PLACEBO_KEY, side),
            s.placebo_cleared.correction(PLACEBO_KEY, side),
        ];
        let written = s.kept_written[kept_slot(pawn_key, side)];
        s.cross[ply] = written != u32::MAX && written != s.search;
    });
}

/// Scores each arm on the common population, then updates each by the live table's own rule.
#[allow(clippy::too_many_arguments)]
pub fn observe(
    ply: usize,
    pawn_key: u64,
    side: Colour,
    best: Score,
    best_move: Move,
    bound: Bound,
    depth: u32,
) {
    with(|s| {
        let Some(raw) = s.raw[ply] else {
            return;
        };
        if score::is_mate(best) || best_move == Move::NULL || best_move.is_noisy() {
            return;
        }
        let corr = s.corr[ply];
        // Scored where the bound runs the way a correction of the raw evaluation would.
        let scored = match bound {
            Bound::Exact => true,
            Bound::Lower => best > raw,
            Bound::Upper => best < raw,
        };
        if scored {
            let delta = i128::from((best - raw).clamp(-MAX_DELTA, MAX_DELTA));
            let pops: &[usize] = if s.cross[ply] { &[0, 1] } else { &[0] };
            for &p in pops {
                s.n[p] += 1;
                s.sdd[p] += delta * delta;
                for (k, &c) in corr.iter().enumerate() {
                    let c = i128::from(c);
                    let e = delta - c;
                    s.sse[p][k] += e * e;
                    s.sdc[p][k] += delta * c;
                    s.scc[p][k] += c * c;
                }
            }
        }
        // Each arm learns as the live table does: its own corrected evaluation decides.
        let arms: [(&mut CorrectionHistory, u64, Score); 4] = [
            (&mut s.cleared, pawn_key, corr[1]),
            (&mut s.kept, pawn_key, corr[2]),
            (&mut s.placebo_kept, PLACEBO_KEY, corr[3]),
            (&mut s.placebo_cleared, PLACEBO_KEY, corr[4]),
        ];
        let mut kept_wrote = false;
        for (i, (table, key, c)) in arms.into_iter().enumerate() {
            let eval = raw + c;
            let usable = match bound {
                Bound::Exact => true,
                Bound::Lower => best > eval,
                Bound::Upper => best < eval,
            };
            if usable {
                table.update(key, side, best - eval, depth);
                kept_wrote |= i == 1;
            }
        }
        if kept_wrote {
            let slot = kept_slot(pawn_key, side);
            s.kept_written[slot] = s.search;
        }
    });
}

/// One `name value` line each, so a driver playing one game per process can sum them over games.
pub fn report(mut say: impl FnMut(&str, i128)) {
    with(|s| {
        say("shadow_reads", i128::from(s.reads));
        say("shadow_live_mismatch", i128::from(s.live_mismatch));
        for (p, pop) in POP_NAMES.iter().enumerate() {
            say(&format!("shadow_{pop}_n"), i128::from(s.n[p]));
            say(&format!("shadow_{pop}_sdd"), s.sdd[p]);
            for (k, arm) in NAMES.iter().enumerate() {
                say(&format!("shadow_{pop}_{arm}_sse"), s.sse[p][k]);
                say(&format!("shadow_{pop}_{arm}_sdc"), s.sdc[p][k]);
                say(&format!("shadow_{pop}_{arm}_scc"), s.scc[p][k]);
            }
        }
    });
}
