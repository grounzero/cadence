// SPDX-License-Identifier: GPL-3.0-or-later

//! A correction table that learns and decides nothing.
//!
//! It records the running difference between a node's static evaluation and
//! the score the search returned for it, keyed by the pawn structure and the
//! side to move. Nothing here is read by any rule that chooses a move, orders
//! one, prunes one or ends a search, so a node count stays a function of the
//! rest of the code.
//!
//! **Two tables over one stream of updates, because they answer different
//! questions.** The keyed arm stores the full pawn key beside a running mean
//! and so measures whether the residual error is persistent under that key at
//! all; the direct arm is the lossy fixed-size table an implementation would
//! ship, and measures what one would actually get.
//!
//! **Every reading is out of sample.** A node's correction is read before its
//! own observation is folded in, so no entry is ever scored against the
//! measurement that made it.
//!
//! **Atomics carry no synchronisation here and are not for parallelism.** The
//! search is single-threaded and the table outlives one `go`, so the whole of
//! what `Relaxed` buys is shared ownership across the search thread boundary.

use std::sync::atomic::{AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed};

use cadence_core::Colour;

use crate::score::Score;

/// Slots in the keyed arm, which stores its key and can tell a true repeat
/// from an index collision. A million of them holds every distinct pawn
/// structure a bounded sample reaches with collisions rare enough to count.
const KEYED_SLOTS: usize = 1 << 20;

/// Slots per side in the direct arm, which is the size the first shippable
/// variant would use. It stores no key, so a collision here is silent by
/// construction and that is the point of measuring beside the keyed arm.
const TABLE_SLOTS: usize = 1 << 14;

/// The fixed-point scale an entry in the direct arm is held at. It buys
/// resolution below a centipawn so that the weighted mean below does not
/// round a small persistent correction away.
const GRAIN: i32 = 256;

/// The denominator of the direct arm's weighted mean, and the largest weight
/// a single observation may carry. A new observation moves an entry by at
/// most `MAX_WEIGHT / WEIGHT_UNIT` of the distance to itself.
const WEIGHT_UNIT: i32 = 256;
const MAX_WEIGHT: i32 = 16;

/// The largest correction either arm will offer, in centipawns. It bounds
/// what the flip counters can be reporting and keeps a correction
/// incommensurable with the mate scale.
const MAX_CORRECTION: Score = 256;

/// The largest delta folded into either arm, in centipawns. A node whose
/// search disagreed with the evaluation by more than this is a tactic rather
/// than an evaluation error, and it would otherwise dominate the mean.
const MAX_DELTA: Score = 1024;

/// The four readings every error bucket holds: the raw evaluation, the keyed
/// arm's correction, the direct arm's, and the placebo's.
///
/// **The placebo is the control the rest of this is worthless without.** It
/// runs the direct arm's update over a single slot, so it corrects for
/// whatever the evaluation is drifting by and knows nothing about the pawn
/// structure; any gain the keyed arms do not have over it is not this
/// technique's to claim.
pub const ARMS: usize = 4;
pub const RAW: usize = 0;
pub const KEYED: usize = 1;
pub const DIRECT: usize = 2;
pub const GLOBAL: usize = 3;

/// One population's error, summed rather than averaged so that the arms stay
/// integer and the division happens once, outside.
#[derive(Default)]
struct Bucket {
    n: AtomicU64,
    abs: [AtomicU64; ARMS],
    sq: [AtomicU64; ARMS],
    /// The signed sum of the observations, which is the bias a single
    /// constant would remove and this technique is not needed for.
    delta: AtomicI64,
    /// The two sums that give the best scale any linear use of a correction
    /// could have had. Their ratio is that scale and the reading is
    /// therefore free of every constant chosen above.
    dot: [AtomicI64; ARMS],
    square: [AtomicI64; ARMS],
}

impl Bucket {
    /// Fold one node's observation and the three errors against it in.
    fn add(&self, delta: i64, corrections: [i64; ARMS]) {
        self.n.fetch_add(1, Relaxed);
        self.delta.fetch_add(delta, Relaxed);
        for (((c, abs), sq), (dot, square)) in corrections
            .iter()
            .zip(self.abs.iter())
            .zip(self.sq.iter())
            .zip(self.dot.iter().zip(self.square.iter()))
        {
            let e = (delta - c).unsigned_abs();
            abs.fetch_add(e, Relaxed);
            sq.fetch_add(e * e, Relaxed);
            dot.fetch_add(delta * c, Relaxed);
            square.fetch_add(c * c, Relaxed);
        }
    }
}

/// One slot of the keyed arm: the key it belongs to, the running sum and
/// count that are its mean, and the generation that last wrote it.
#[derive(Default)]
struct Keyed {
    key: AtomicU64,
    sum: AtomicI64,
    count: AtomicU32,
    generation: AtomicU32,
}

/// What a probe of the keyed arm found, which is the part the error buckets
/// are selected on.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Seen {
    /// The slot was empty.
    Fresh,
    /// The slot held a different key, so its value is another structure's.
    Collision,
    /// The slot held this key, written during the search in progress.
    Repeat,
    /// The slot held this key, written before the search in progress began.
    CrossMove,
}

/// What one node's probe hands back to the caller, to be given to
/// [`Shadow::update`] unchanged when the node returns a score.
#[derive(Clone, Copy)]
pub struct Probe {
    key: u64,
    index: usize,
    direct: usize,
    /// The correction each arm offers, in centipawns.
    pub corrections: [Score; ARMS],
    seen: Seen,
}

/// The tables, their counters, and nothing that decides anything.
pub struct Shadow {
    keyed: Box<[Keyed]>,
    direct: Box<[AtomicI32]>,
    /// The placebo's one slot, updated by the same rule as the direct arm.
    global: AtomicI32,
    generation: AtomicU32,
    /// Nodes that offered a usable observation, which is the denominator of
    /// everything below.
    eligible: AtomicU64,
    /// How the keyed probe resolved, in [`Seen`]'s order.
    resolution: [AtomicU64; 4],
    /// Eligible nodes whose keyed slot already held this key that many
    /// observations or more.
    depth_of_evidence: [AtomicU64; 4],
    /// Error over every eligible node, over those the keyed arm had seen
    /// before, and over those it had seen in an earlier search.
    all: Bucket,
    seen: Bucket,
    cross: Bucket,
    /// Nodes at which each margin rule was asked with an evaluation in hand,
    /// indexed by rule.
    tests: [AtomicU64; 2],
    /// How often each rule fired on the raw evaluation, and how often it
    /// would have fired on each corrected one.
    fires: [[AtomicU64; ARMS]; 2],
    /// How often a correction turned a rule on that the raw evaluation left
    /// off, and off that it left on.
    flip_on: [[AtomicU64; ARMS]; 2],
    flip_off: [[AtomicU64; ARMS]; 2],
}

/// The margin rules the flip counters cover.
pub const FUTILITY: usize = 0;
pub const REVERSE: usize = 1;

impl Shadow {
    #[must_use]
    pub fn new() -> Shadow {
        Shadow {
            keyed: (0..KEYED_SLOTS).map(|_| Keyed::default()).collect(),
            direct: (0..2 * TABLE_SLOTS).map(|_| AtomicI32::new(0)).collect(),
            global: AtomicI32::new(0),
            generation: AtomicU32::new(1),
            eligible: AtomicU64::new(0),
            resolution: Default::default(),
            depth_of_evidence: Default::default(),
            all: Bucket::default(),
            seen: Bucket::default(),
            cross: Bucket::default(),
            tests: Default::default(),
            fires: Default::default(),
            flip_on: Default::default(),
            flip_off: Default::default(),
        }
    }

    /// Begin a new search, so that a later probe can tell an entry written
    /// by this search from one that survived an earlier move.
    pub fn advance(&self) {
        self.generation.fetch_add(1, Relaxed);
    }

    /// The correction both arms offer for `key` and `side`, read before this
    /// node has contributed anything of its own.
    #[must_use]
    pub fn probe(&self, key: u64, side: Colour) -> Probe {
        // The side to move is mixed in rather than indexed on, because the
        // evaluation this corrects is already relative to it.
        let keyed_key = if side == Colour::White { key } else { !key };
        let index = (keyed_key % KEYED_SLOTS as u64) as usize;
        let slot = &self.keyed[index];
        let held = slot.key.load(Relaxed);
        let count = slot.count.load(Relaxed);
        let seen = if count == 0 {
            Seen::Fresh
        } else if held != keyed_key {
            Seen::Collision
        } else if slot.generation.load(Relaxed) == self.generation.load(Relaxed) {
            Seen::Repeat
        } else {
            Seen::CrossMove
        };
        let keyed = match seen {
            Seen::Repeat | Seen::CrossMove => {
                clamp_correction((slot.sum.load(Relaxed) / i64::from(count)) as Score)
            }
            Seen::Fresh | Seen::Collision => 0,
        };
        let direct = side.index() * TABLE_SLOTS + (key % TABLE_SLOTS as u64) as usize;
        let stored = self.direct[direct].load(Relaxed);
        let global = self.global.load(Relaxed);
        Probe {
            key: keyed_key,
            index,
            direct,
            corrections: [
                0,
                keyed,
                clamp_correction(stored / GRAIN),
                clamp_correction(global / GRAIN),
            ],
            seen,
        }
    }

    /// Record what one margin rule would have decided on each corrected
    /// evaluation beside what it did decide on the raw one.
    pub fn rule(&self, rule: usize, raw: bool, corrected: [bool; ARMS]) {
        self.tests[rule].fetch_add(1, Relaxed);
        for ((now, fires), (on, off)) in corrected
            .iter()
            .zip(self.fires[rule].iter())
            .zip(self.flip_on[rule].iter().zip(self.flip_off[rule].iter()))
        {
            fires.fetch_add(u64::from(*now), Relaxed);
            if *now && !raw {
                on.fetch_add(1, Relaxed);
            } else if !*now && raw {
                off.fetch_add(1, Relaxed);
            }
        }
    }

    /// Fold one node's observation in, and score the corrections that were
    /// read before it against it.
    ///
    /// `delta` is the search's score less the static evaluation, in
    /// centipawns. `depth` weights the direct arm, on the same ground a
    /// history bonus is weighted by it: a deeper node's disagreement stood up
    /// to more search.
    pub fn update(&self, probe: &Probe, delta: Score, depth: u32) {
        let delta = delta.clamp(-MAX_DELTA, MAX_DELTA);
        self.eligible.fetch_add(1, Relaxed);
        self.resolution[probe.seen as usize].fetch_add(1, Relaxed);
        let d = i64::from(delta);
        let cs = probe.corrections.map(i64::from);
        self.all.add(d, cs);
        if matches!(probe.seen, Seen::Repeat | Seen::CrossMove) {
            self.seen.add(d, cs);
            let count = self.keyed[probe.index].count.load(Relaxed);
            for (i, floor) in [1u32, 2, 4, 8].iter().enumerate() {
                self.depth_of_evidence[i].fetch_add(u64::from(count >= *floor), Relaxed);
            }
        }
        if probe.seen == Seen::CrossMove {
            self.cross.add(d, cs);
        }
        self.fold_keyed(probe, delta);
        Shadow::fold_direct(&self.direct[probe.direct], delta, depth);
        Shadow::fold_direct(&self.global, delta, depth);
    }

    /// The keyed arm's update: a running mean, restarted where the slot
    /// belonged to another key.
    fn fold_keyed(&self, probe: &Probe, delta: Score) {
        let slot = &self.keyed[probe.index];
        if probe.seen == Seen::Collision || probe.seen == Seen::Fresh {
            slot.key.store(probe.key, Relaxed);
            slot.sum.store(0, Relaxed);
            slot.count.store(0, Relaxed);
        }
        slot.sum.fetch_add(i64::from(delta), Relaxed);
        slot.count.fetch_add(1, Relaxed);
        slot.generation
            .store(self.generation.load(Relaxed), Relaxed);
    }

    /// The direct arm's update: a weighted mean toward the observation,
    /// integer throughout and bounded by construction.
    ///
    /// Its equilibrium under a constant delta is that delta, which is what a
    /// correction has to preserve and what a gravity update of the kind a
    /// history table uses does not.
    fn fold_direct(slot: &AtomicI32, delta: Score, depth: u32) {
        let entry = slot.load(Relaxed);
        let weight = (i32::try_from(depth).unwrap_or(MAX_WEIGHT) + 1).min(MAX_WEIGHT);
        let target = delta * GRAIN;
        let next = (entry * (WEIGHT_UNIT - weight) + target * weight) / WEIGHT_UNIT;
        slot.store(
            next.clamp(-MAX_CORRECTION * GRAIN, MAX_CORRECTION * GRAIN),
            Relaxed,
        );
    }

    /// Every counter, one `name value` pair per line, for a reader outside
    /// the process. Sums are reported unaveraged so that no division happens
    /// where the tree forbids a float.
    ///
    /// # Errors
    ///
    /// Whatever `out` returns. Nothing here inspects it, because a caller
    /// writing to a closed pipe has already lost the reading.
    pub fn report(&self, out: &mut dyn std::io::Write) -> std::io::Result<()> {
        writeln!(out, "eligible {}", self.eligible.load(Relaxed))?;
        for (name, slot) in ["fresh", "collision", "repeat", "crossmove"]
            .iter()
            .zip(self.resolution.iter())
        {
            writeln!(out, "seen_{name} {}", slot.load(Relaxed))?;
        }
        for (floor, slot) in [1, 2, 4, 8].iter().zip(self.depth_of_evidence.iter()) {
            writeln!(out, "evidence_ge{floor} {}", slot.load(Relaxed))?;
        }
        for (name, bucket) in [
            ("all", &self.all),
            ("seen", &self.seen),
            ("cross", &self.cross),
        ] {
            writeln!(out, "{name}_n {}", bucket.n.load(Relaxed))?;
            writeln!(out, "{name}_delta {}", bucket.delta.load(Relaxed))?;
            for (arm, label) in ["raw", "keyed", "direct", "global"].iter().enumerate() {
                writeln!(out, "{name}_abs_{label} {}", bucket.abs[arm].load(Relaxed))?;
                writeln!(out, "{name}_sq_{label} {}", bucket.sq[arm].load(Relaxed))?;
                writeln!(out, "{name}_dot_{label} {}", bucket.dot[arm].load(Relaxed))?;
                writeln!(
                    out,
                    "{name}_square_{label} {}",
                    bucket.square[arm].load(Relaxed)
                )?;
            }
        }
        for (rule, name) in ["futility", "reverse"].iter().enumerate() {
            writeln!(out, "{name}_tests {}", self.tests[rule].load(Relaxed))?;
            for (arm, label) in ["raw", "keyed", "direct", "global"].iter().enumerate() {
                writeln!(
                    out,
                    "{name}_fires_{label} {}",
                    self.fires[rule][arm].load(Relaxed)
                )?;
                writeln!(
                    out,
                    "{name}_on_{label} {}",
                    self.flip_on[rule][arm].load(Relaxed)
                )?;
                writeln!(
                    out,
                    "{name}_off_{label} {}",
                    self.flip_off[rule][arm].load(Relaxed)
                )?;
            }
        }
        Ok(())
    }
}

impl Default for Shadow {
    fn default() -> Shadow {
        Shadow::new()
    }
}

/// A correction, held inside the band the flip counters are meaningful in.
fn clamp_correction(v: Score) -> Score {
    v.clamp(-MAX_CORRECTION, MAX_CORRECTION)
}
