// SPDX-License-Identifier: GPL-3.0-or-later

//! Open-addressed 64-byte buckets of four slots, indexed by the Zobrist key.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use cadence_core::Move;

/// What the STC preset passes.
pub const DEFAULT_HASH_MB: usize = 16;

/// 16,384 buckets: small enough to be useless, large enough to work.
pub const MIN_HASH_MB: usize = 1;

/// Past any machine in the fleet; the allocation is fallible either way.
pub const MAX_HASH_MB: usize = 4096;

/// Four sixteen-byte slots fill one cache line, so a whole-bucket probe costs one miss.
const SLOTS: usize = 4;

/// Buckets [`Table::hashfull`] looks at. Four slots each, so the sample is the thousand a
/// permill is a count of.
const SAMPLE_BUCKETS: usize = 250;

/// Plies of depth one generation of age is worth when choosing a full bucket's victim.
const AGE_PENALTY: i32 = 8;

const AGE_MASK: u8 = 0x3F;

/// Fail-soft returns three kinds of value, and a table that does not distinguish them cannot be
/// probed safely.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Bound {
    /// At least the score.
    Lower = 1,
    /// At most the score.
    Upper = 2,
    /// The score is the value.
    Exact = 3,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hit {
    /// Tried first whatever `depth` says: a hit too shallow to answer still names the move that
    /// answered before.
    pub mv: Move,
    /// Relative to the node it was stored at, not the root.
    pub score: i16,
    pub depth: u8,
    pub bound: Bound,
}

/// The data word of a slot. The high sixteen bits are deliberately empty.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct Entry(u64);

const MOVE_SHIFT: u32 = 0;
const SCORE_SHIFT: u32 = 16;
const DEPTH_SHIFT: u32 = 32;
const BOUND_SHIFT: u32 = 40;
const AGE_SHIFT: u32 = 42;

impl Entry {
    /// No bound, so it decodes to nothing.
    pub const EMPTY: Entry = Entry(0);

    /// `age` is masked to six bits.
    #[must_use]
    pub const fn new(mv: Move, score: i16, depth: u8, bound: Bound, age: u8) -> Entry {
        Entry(
            ((mv.to_bits() as u64) << MOVE_SHIFT)
                | ((score as u16 as u64) << SCORE_SHIFT)
                | ((depth as u64) << DEPTH_SHIFT)
                | ((bound as u64) << BOUND_SHIFT)
                | (((age & AGE_MASK) as u64) << AGE_SHIFT),
        )
    }

    #[must_use]
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn from_bits(bits: u64) -> Entry {
        Entry(bits)
    }

    /// Zero for an untouched slot.
    #[must_use]
    const fn age(self) -> u8 {
        ((self.0 >> AGE_SHIFT) as u8) & AGE_MASK
    }

    /// The bound decides: zero is not a bound, so a never-written slot decodes to nothing whatever
    /// its other bits say.
    #[must_use]
    pub const fn decode(self) -> Option<Hit> {
        let bound = match (self.0 >> BOUND_SHIFT) & 0b11 {
            1 => Bound::Lower,
            2 => Bound::Upper,
            3 => Bound::Exact,
            _ => return None,
        };
        Some(Hit {
            mv: Move::from_bits((self.0 >> MOVE_SHIFT) as u16),
            score: ((self.0 >> SCORE_SHIFT) as u16).cast_signed(),
            depth: (self.0 >> DEPTH_SHIFT) as u8,
            bound,
        })
    }
}

/// The lockless read: `Some` only when `word0 ^ word1` is `key` and the data decodes.
#[must_use]
pub fn verify(word0: u64, word1: u64, key: u64) -> Option<Hit> {
    if word0 ^ word1 != key {
        return None;
    }
    Entry::from_bits(word1).decode()
}

/// One slot: `key ^ data`, then `data`.
struct Slot {
    xor_key: AtomicU64,
    data: AtomicU64,
}

impl Slot {
    fn empty() -> Slot {
        Slot {
            xor_key: AtomicU64::new(0),
            data: AtomicU64::new(0),
        }
    }

    #[inline]
    fn read(&self) -> (u64, u64) {
        (
            self.xor_key.load(Ordering::Relaxed),
            self.data.load(Ordering::Relaxed),
        )
    }

    /// Under `Relaxed` nothing owes a reader this order; it only avoids widening the window in
    /// which a new key sits beside old data.
    #[inline]
    fn write(&self, key: u64, entry: Entry) {
        let data = entry.to_bits();
        self.data.store(data, Ordering::Relaxed);
        self.xor_key.store(key ^ data, Ordering::Relaxed);
    }
}

#[repr(align(64))]
struct Bucket {
    slots: [Slot; SLOTS],
}

impl Bucket {
    fn empty() -> Bucket {
        Bucket {
            slots: std::array::from_fn(|_| Slot::empty()),
        }
    }
}

const _: () = assert!(size_of::<Slot>() == 16);
const _: () = assert!(size_of::<Bucket>() == 64);
const _: () = assert!(align_of::<Bucket>() == 64);

/// Never resized: a new size is a new table.
pub struct Table {
    buckets: Box<[Bucket]>,
    generation: AtomicU8,
}

impl Table {
    /// Rounded down to whole buckets; `None` if allocation fails, because an engine that aborts on
    /// a GUI's oversized request loses the game.
    #[must_use]
    pub fn new(mb: usize) -> Option<Table> {
        let bytes = mb.checked_mul(1 << 20)?;
        Table::with_buckets(bytes / size_of::<Bucket>())
    }

    /// Zero buckets never stores and never hits: the search with no table.
    #[must_use]
    pub fn with_buckets(buckets: usize) -> Option<Table> {
        let mut v: Vec<Bucket> = Vec::new();
        v.try_reserve_exact(buckets).ok()?;
        v.resize_with(buckets, Bucket::empty);
        Some(Table {
            buckets: v.into_boxed_slice(),
            generation: AtomicU8::new(0),
        })
    }

    #[must_use]
    pub fn buckets(&self) -> usize {
        self.buckets.len()
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.buckets.len() * size_of::<Bucket>()
    }

    /// Permill of the slots in the first `SAMPLE_BUCKETS` buckets. Counts a slot ever written, not
    /// one this search wrote, so within a game it only rises until [`Table::clear`].
    #[must_use]
    pub fn hashfull(&self) -> u32 {
        let sampled = SAMPLE_BUCKETS.min(self.buckets.len());
        if sampled == 0 {
            return 0;
        }
        let mut used = 0;
        for bucket in &self.buckets[..sampled] {
            for slot in &bucket.slots {
                // The data word alone: whether a slot was ever written needs no key.
                let (_, data) = slot.read();
                used += usize::from(Entry::from_bits(data).decode().is_some());
            }
        }
        (used * 1000 / (sampled * SLOTS)) as u32
    }

    /// `bench` calls this between positions, which makes its node count independent of position
    /// order.
    pub fn clear(&self) {
        for bucket in &self.buckets {
            for slot in &bucket.slots {
                slot.xor_key.store(0, Ordering::Relaxed);
                slot.data.store(0, Ordering::Relaxed);
            }
        }
        self.generation.store(0, Ordering::Relaxed);
    }

    /// Everything stored from now on is one generation younger.
    pub fn new_search(&self) {
        let next = self.generation.load(Ordering::Relaxed).wrapping_add(1) & AGE_MASK;
        self.generation.store(next, Ordering::Relaxed);
    }

    #[must_use]
    pub fn generation(&self) -> u8 {
        self.generation.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn probe(&self, key: u64) -> Option<Hit> {
        let bucket = self.bucket(key)?;
        for slot in &bucket.slots {
            let (word0, word1) = slot.read();
            if let Some(hit) = verify(word0, word1, key) {
                return Some(hit);
            }
        }
        None
    }

    pub fn store(&self, key: u64, mv: Move, score: i16, depth: u8, bound: Bound) {
        let Some(bucket) = self.bucket(key) else {
            return;
        };
        let generation = self.generation();
        let fresh = Entry::new(mv, score, depth, bound, generation);
        let mut victim = 0;
        let mut worst = i32::MAX;
        for (i, slot) in bucket.slots.iter().enumerate() {
            let (word0, word1) = slot.read();
            let entry = Entry::from_bits(word1);
            let Some(hit) = entry.decode() else {
                slot.write(key, fresh);
                return;
            };
            if word0 ^ word1 == key {
                // A deeper result is kept against a shallower bound, which tells the next probe
                // less.
                if depth < hit.depth && bound != Bound::Exact {
                    return;
                }
                slot.write(key, fresh);
                return;
            }
            let worth = i32::from(hit.depth)
                - AGE_PENALTY * i32::from(age_distance(generation, entry.age()));
            if worth < worst {
                worst = worth;
                victim = i;
            }
        }
        bucket.slots[victim].write(key, fresh);
    }

    /// The high half of `key * buckets`, which spreads a key over any bucket count, not only a
    /// power of two.
    #[inline]
    fn bucket(&self, key: u64) -> Option<&Bucket> {
        let index = ((u128::from(key) * self.buckets.len() as u128) >> 64) as usize;
        self.buckets.get(index)
    }
}

/// Generations between `now` and `then`, on the six-bit ring.
#[inline]
const fn age_distance(now: u8, then: u8) -> u8 {
    now.wrapping_sub(then) & AGE_MASK
}
