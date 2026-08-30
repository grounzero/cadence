// SPDX-License-Identifier: GPL-3.0-or-later

//! The transposition table: what the search already knows about a position.
//!
//! A fixed-size open-addressed table of 64-byte buckets, four slots each,
//! indexed by the Zobrist key. A slot is two 64-bit words and holds one
//! result: the depth it was searched to, its score, whether that score is
//! exact or a bound, and the move that produced it.
//!
//! **Lockless integrity.** A slot stores `key ^ data` beside `data`, and a
//! read is accepted only when the two words XOR back to the key being looked
//! up. Two words cannot be read atomically as a pair, so a reader racing a
//! writer can see one word of each: the check turns that into a miss instead
//! of a plausible-looking entry belonging to another position, which the
//! search would act on. A truncated key stored directly cannot express this,
//! and is why it is not what is stored. The words are `AtomicU64` and every
//! access is `Relaxed`: the correctness argument is the XOR check and not an
//! ordering. The search is single threaded and will be for a long time; this
//! is not a scheme to retrofit under pressure.
//!
//! **Determinism.** The index is a function of the key and the bucket count;
//! the bucket scan runs in slot order and the replacement tie-break keeps
//! the first minimum; the generation is a function of how many searches have
//! run since the last [`Table::clear`], and `clear` resets it. So a cleared
//! table of a fixed size gives a node count that is a function of the code
//! alone, which is what `bench` rests on: it clears at every position seam.
//! No hash map, no float.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use cadence_core::Move;

/// The default `Hash`, in mebibytes: what the engine allocates before a
/// GUI says otherwise. Sixteen, which is what the STC preset passes.
pub const DEFAULT_HASH_MB: usize = 16;

/// The smallest `Hash` the UCI option accepts. One mebibyte is 16,384
/// buckets, which is small enough to be useless and large enough to work.
pub const MIN_HASH_MB: usize = 1;

/// The largest `Hash` the UCI option accepts. Four gibibytes is past any
/// machine in the fleet; the allocation is fallible either way.
pub const MAX_HASH_MB: usize = 4096;

/// Slots per bucket. Four sixteen-byte slots is one 64-byte cache line, so
/// a probe that scans the whole bucket costs one cache miss.
const SLOTS: usize = 4;

/// Depth, in ply, that one generation of age is worth when the least
/// valuable slot of a full bucket is chosen.
const AGE_PENALTY: i32 = 8;

/// The generation counter is six bits, so it wraps at 64.
const AGE_MASK: u8 = 0x3F;

/// What a score in a slot is: the value itself, or a bound on it.
///
/// Fail-soft alpha-beta returns three kinds of value, and a table that
/// does not distinguish them cannot be probed safely. `Exact` is a value
/// the window contained. `Lower` came from a node that failed high, so the
/// true value is at least this. `Upper` came from a node that failed low,
/// so the true value is at most this.
///
/// The discriminants are the two bits stored in a slot, and **zero is not
/// one of them**: an untouched slot decodes to no bound at all, so an
/// all-zero slot can never be read as a result (see [`Entry::decode`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Bound {
    /// The search failed high here: the true value is at least the score.
    Lower = 1,
    /// The search failed low here: the true value is at most the score.
    Upper = 2,
    /// The window contained the value: the score is the value.
    Exact = 3,
}

/// One decoded result: what the search reads off a hit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hit {
    /// The best move found at this node. The search tries it first at an
    /// interior node (`search::order_first`), whatever `depth` says: a hit
    /// too shallow to answer the question still names the move that
    /// answered it before.
    pub mv: Move,
    /// The score, relative to the node it was stored at rather than to the
    /// root: see `score::to_tt`.
    pub score: i16,
    /// The depth the score was searched to.
    pub depth: u8,
    pub bound: Bound,
}

/// The data word of a slot.
///
/// | Bits      | Field  | Notes                                     |
/// |-----------|--------|-------------------------------------------|
/// | `0..=15`  | move   | `Move::to_bits`                            |
/// | `16..=31` | score  | `i16`, relative to the node                |
/// | `32..=39` | depth  | ply; the main search never stores past 255 |
/// | `40..=41` | bound  | 0 is an untouched slot                     |
/// | `42..=47` | age    | the generation the store belonged to       |
/// | `48..=63` | unused | zero                                       |
///
/// The high sixteen bits are deliberately empty. A static evaluation
/// belongs there when something reads one; nothing does yet, and a field
/// no code reads is a field no test covers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(transparent)]
pub struct Entry(u64);

const MOVE_SHIFT: u32 = 0;
const SCORE_SHIFT: u32 = 16;
const DEPTH_SHIFT: u32 = 32;
const BOUND_SHIFT: u32 = 40;
const AGE_SHIFT: u32 = 42;

impl Entry {
    /// An untouched slot: no bound, so it decodes to nothing.
    pub const EMPTY: Entry = Entry(0);

    /// Pack one result. `age` is masked to six bits.
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

    /// The generation this was stored in, or zero for an untouched slot.
    #[must_use]
    const fn age(self) -> u8 {
        ((self.0 >> AGE_SHIFT) as u8) & AGE_MASK
    }

    /// The result, or `None` for an untouched slot.
    ///
    /// The bound field is what decides: it is two bits and zero is not a
    /// bound, so a slot that has never been written decodes to nothing
    /// whatever else its bits happen to say.
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

/// The lockless read, as a function: the two words of a slot, validated
/// against the key being looked up.
///
/// `Some` only when `word0 ^ word1` is `key` **and** the data decodes to a
/// result. A read that mixed one word of one write with one word of
/// another fails the first test; an untouched slot fails the second, and
/// also the first for every key but zero.
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

    /// Data first, then the checked key. Under `Relaxed` neither the
    /// compiler nor the hardware owes a reader that order; what the order
    /// buys is that the window in which a reader can see a new key beside
    /// old data is not widened on purpose. The check is what makes either
    /// order safe.
    #[inline]
    fn write(&self, key: u64, entry: Entry) {
        let data = entry.to_bits();
        self.data.store(data, Ordering::Relaxed);
        self.xor_key.store(key ^ data, Ordering::Relaxed);
    }
}

/// Four slots on one cache line.
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

/// The table. Allocated once, never resized: a new size is a new table.
pub struct Table {
    buckets: Box<[Bucket]>,
    generation: AtomicU8,
}

impl Table {
    /// A table of `mb` mebibytes, rounded down to whole buckets, or `None`
    /// if the allocation fails.
    ///
    /// A GUI can ask for more memory than the machine has, and an engine
    /// that aborts on it is an engine that loses the game; the caller
    /// keeps whatever table it had.
    #[must_use]
    pub fn new(mb: usize) -> Option<Table> {
        let bytes = mb.checked_mul(1 << 20)?;
        Table::with_buckets(bytes / size_of::<Bucket>())
    }

    /// A table of exactly `buckets` buckets, or `None` if the allocation
    /// fails. Zero buckets is a table that never stores and never hits,
    /// which is the search with no table at all.
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

    /// How many buckets there are.
    #[must_use]
    pub fn buckets(&self) -> usize {
        self.buckets.len()
    }

    /// How much memory the table occupies, in bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.buckets.len() * size_of::<Bucket>()
    }

    /// Empty every slot and put the generation back to zero.
    ///
    /// `bench` calls this between positions, which is what makes its node
    /// count independent of position order; `ucinewgame`
    /// calls it because the next game's tree has nothing to do with this
    /// one's.
    pub fn clear(&self) {
        for bucket in &self.buckets {
            for slot in &bucket.slots {
                slot.xor_key.store(0, Ordering::Relaxed);
                slot.data.store(0, Ordering::Relaxed);
            }
        }
        self.generation.store(0, Ordering::Relaxed);
    }

    /// Begin a search: everything stored from now on is one generation
    /// younger than what is already there.
    pub fn new_search(&self) {
        let next = self.generation.load(Ordering::Relaxed).wrapping_add(1) & AGE_MASK;
        self.generation.store(next, Ordering::Relaxed);
    }

    /// The current generation.
    #[must_use]
    pub fn generation(&self) -> u8 {
        self.generation.load(Ordering::Relaxed)
    }

    /// The result stored for `key`, if there is one.
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

    /// Store a result for `key`, choosing which slot of the bucket to take.
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
                // An untouched slot is free; take it and stop looking.
                slot.write(key, fresh);
                return;
            };
            if word0 ^ word1 == key {
                // The same position. Keep what is there when it was
                // searched deeper and the new result is only a bound: a
                // shallower bound tells the next probe less.
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

    /// The bucket `key` indexes, or `None` when the table has no buckets.
    ///
    /// The index is the high half of `key * buckets`, which spreads a
    /// 64-bit key over any bucket count rather than only over a power of
    /// two. That is what lets `Hash` mean what it says: a table asked for
    /// 48 mebibytes is 48 mebibytes, not 32.
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
