// SPDX-License-Identifier: GPL-3.0-or-later

//! An instrument that decides nothing: what a reduced-depth capture probe would have concluded
//! at each node that could run one, beside what the node's own search then returned. Process-wide
//! and single-thread only, read back through the `shadow` UCI command.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::tt::Table;

/// One scratch table per probe arm and one for the null move's re-search, so no arm's stores
/// warm another's subtree. A quiet search stores here and reads here before the real table.
pub const NULL_ARM: usize = REDUCTIONS.len() * MARGINS.len();
static SCRATCH: OnceLock<Vec<Table>> = OnceLock::new();

/// The scratch table for `arm`, allocated at the engine's default size on first use.
///
/// # Panics
///
/// If the tables cannot be allocated.
pub fn scratch(arm: usize) -> &'static Table {
    &SCRATCH.get_or_init(|| {
        (0..=NULL_ARM)
            .map(|_| Table::new(crate::tt::DEFAULT_HASH_MB).expect("a scratch table"))
            .collect()
    })[arm]
}

/// The reductions the probe is taken at: the child is searched at the node's depth less this.
pub const REDUCTIONS: [u32; 2] = [3, 4];

/// The margins above beta the probe is asked to beat, in centipawns.
pub const MARGINS: [i32; 5] = [100, 150, 200, 250, 300];

/// The deepest node an arm runs at. Above it the probe searches most of a move's whole tree,
/// ten times over, on nodes the shadow's own counters do not charge, and two games of the first
/// full run spent two hours each there. **Every candidate the selection rule considers has its
/// minimum depth at or below ten**, so what this gives up is the deepest end of each pool.
pub const ARM_DEPTH_CAP: u32 = 16;

/// What one arm may spend before the node is dropped from every arm, in quiet nodes. The
/// backstop for a node inside the depth cap that is expensive anyway.
pub const ARM_NODE_CAP: u64 = 500_000;

/// Nodes dropped by either bound, per depth, so the exclusion is counted rather than silent.
static DROPPED: [AtomicU64; DEPTHS] = [const { AtomicU64::new(0) }; DEPTHS];

/// Depths are bucketed up to this one, deeper nodes landing in the last bucket.
const DEPTHS: usize = 32;

/// What is kept per reduction, margin and depth.
const STATS: usize = 5;
const ADMITTED: usize = 0;
const CUT: usize = 1;
const FALSE_CUT: usize = 2;
const SAVED: usize = 3;
const PROBE_NODES: usize = 4;
const STAT_NAMES: [&str; STATS] = ["admitted", "cut", "false", "saved", "probe"];

const PC_LEN: usize = REDUCTIONS.len() * MARGINS.len() * DEPTHS * STATS;
static PC: [AtomicU64; PC_LEN] = [const { AtomicU64::new(0) }; PC_LEN];

/// The null move's reference arm: sampled cuts, and how many the full search contradicts.
static NULL_SAMPLED: [AtomicU64; DEPTHS] = [const { AtomicU64::new(0) }; DEPTHS];
static NULL_FALSE: [AtomicU64; DEPTHS] = [const { AtomicU64::new(0) }; DEPTHS];
static NULL_SEEN: AtomicU64 = AtomicU64::new(0);

/// One null-move cut in this many is re-searched in full.
pub const NULL_STRIDE: u64 = 16;

fn bucket(depth: u32) -> usize {
    (depth as usize).min(DEPTHS - 1)
}

fn index(r: usize, m: usize, depth: u32, stat: usize) -> usize {
    ((r * MARGINS.len() + m) * DEPTHS + bucket(depth)) * STATS + stat
}

fn add(i: usize, n: u64) {
    PC[i].fetch_add(n, Ordering::Relaxed);
}

/// The probe's verdict for one reduction and margin at one node, and what it cost.
#[derive(Clone, Copy, Default)]
pub struct Arm {
    pub admitted: bool,
    pub cut: bool,
    pub nodes: u64,
}

/// Fold one node's arms in, against whether the node's own search failed high (`held`) and the
/// nodes that search spent below the probe.
pub fn record(depth: u32, arms: &[[Arm; MARGINS.len()]; REDUCTIONS.len()], held: bool, spent: u64) {
    for (r, row) in arms.iter().enumerate() {
        for (m, arm) in row.iter().enumerate() {
            if !arm.admitted {
                continue;
            }
            add(index(r, m, depth, ADMITTED), 1);
            add(index(r, m, depth, PROBE_NODES), arm.nodes);
            if arm.cut {
                add(index(r, m, depth, CUT), 1);
                add(index(r, m, depth, SAVED), spent);
                if !held {
                    add(index(r, m, depth, FALSE_CUT), 1);
                }
            }
        }
    }
}

/// One node dropped for cost, at `depth`.
pub fn record_dropped(depth: u32) {
    DROPPED[bucket(depth)].fetch_add(1, Ordering::Relaxed);
}

/// Whether this null-move cut is one of the sampled ones.
pub fn sample_null() -> bool {
    NULL_SEEN.fetch_add(1, Ordering::Relaxed) % NULL_STRIDE == 0
}

/// Fold one sampled null-move cut in, against whether the full search held beta.
pub fn record_null(depth: u32, held: bool) {
    NULL_SAMPLED[bucket(depth)].fetch_add(1, Ordering::Relaxed);
    if !held {
        NULL_FALSE[bucket(depth)].fetch_add(1, Ordering::Relaxed);
    }
}

/// Every non-zero counter as `name value`, one per line.
pub fn print() {
    for (r, reduction) in REDUCTIONS.iter().enumerate() {
        for (m, margin) in MARGINS.iter().enumerate() {
            for d in 0..DEPTHS {
                for (s, name) in STAT_NAMES.iter().enumerate() {
                    let v = PC[((r * MARGINS.len() + m) * DEPTHS + d) * STATS + s]
                        .load(Ordering::Relaxed);
                    if v != 0 {
                        println!("pc_r{reduction}_m{margin}_d{d}_{name} {v}");
                    }
                }
            }
        }
    }
    for d in 0..DEPTHS {
        let dropped = DROPPED[d].load(Ordering::Relaxed);
        if dropped != 0 {
            println!("dropped_d{d} {dropped}");
        }
    }
    for d in 0..DEPTHS {
        let sampled = NULL_SAMPLED[d].load(Ordering::Relaxed);
        if sampled != 0 {
            println!("null_d{d}_sampled {sampled}");
            println!("null_d{d}_false {}", NULL_FALSE[d].load(Ordering::Relaxed));
        }
    }
}
