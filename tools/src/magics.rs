// SPDX-License-Identifier: GPL-3.0-or-later

//! `cadence-tools magics [seed]`: search for the magic numbers `core` bakes
//! into its sliding-attack tables, and print them as Rust source.
//!
//! Self-contained on purpose. This tool depends on nothing in `core`'s magic
//! module, so it can regenerate the numbers that module is built from without
//! that module compiling, and so that the generator and the tables share no
//! code that could be wrong in the same way. The ray-walk here is the third
//! independent copy in the repository: `core` has one for const-evaluating
//! the tables, the gate has one as its oracle, and this one produces the
//! constants both of those are checked against.
//!
//! The method is the standard trial search: sparse random 64-bit candidates,
//! each verified against every occupancy subset of the square's relevant mask
//! for index collisions that map two different attack sets to one slot.
//! Plain magics with `shift = 64 - popcount(mask)`, no overlapping, no fancy
//! shifts: the tables are 102,400 + 5,248 entries and live in `.rodata`.
//!
//! Deterministic: the seed is printed in the output header, so the same seed
//! reproduces the same numbers.

use std::fmt::Write as _;

const ROOK_DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
const BISHOP_DIRS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];

const DEFAULT_SEED: u64 = 0x00CA_DE7C_E5EE_D001;

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A sparse candidate: the AND of three draws leaves roughly one bit in eight
    /// set, which is the density that tends to hash well.
    fn sparse(&mut self) -> u64 {
        self.next() & self.next() & self.next()
    }
}

fn bit(f: i32, r: i32) -> Option<u64> {
    ((0..8).contains(&f) && (0..8).contains(&r))
        .then(|| 1u64 << (u8::try_from(r * 8 + f).expect("in range")))
}

/// Attacks from `sq` under `occ`: walk each direction to the edge, including
/// the first occupied square.
fn attacks(sq: u32, occ: u64, dirs: &[(i32, i32)]) -> u64 {
    let (f0, r0) = (
        i32::try_from(sq % 8).expect("fits"),
        i32::try_from(sq / 8).expect("fits"),
    );
    let mut out = 0;
    for &(df, dr) in dirs {
        let (mut f, mut r) = (f0 + df, r0 + dr);
        while let Some(b) = bit(f, r) {
            out |= b;
            if occ & b != 0 {
                break;
            }
            f += df;
            r += dr;
        }
    }
    out
}

/// The relevant occupancy: the empty-board rays minus their last square in
/// each direction, since a blocker on the edge changes nothing.
fn mask(sq: u32, dirs: &[(i32, i32)]) -> u64 {
    let (f0, r0) = (
        i32::try_from(sq % 8).expect("fits"),
        i32::try_from(sq / 8).expect("fits"),
    );
    let mut out = 0;
    for &(df, dr) in dirs {
        let (mut f, mut r) = (f0 + df, r0 + dr);
        while bit(f + df, r + dr).is_some() {
            out |= bit(f, r).expect("on board");
            f += df;
            r += dr;
        }
    }
    out
}

/// One square's search result.
struct Found {
    magic: u64,
    tries: u64,
}

/// Search for a magic for `sq`.
fn search(sq: u32, dirs: &[(i32, i32)], rng: &mut Rng) -> Found {
    let mask = mask(sq, dirs);
    let bits = mask.count_ones();
    let shift = 64 - bits;
    let size = 1usize << bits;

    // Every subset of the mask and its attack set, once.
    let mut occs = Vec::with_capacity(size);
    let mut refs = Vec::with_capacity(size);
    let mut occ = 0u64;
    loop {
        occs.push(occ);
        refs.push(attacks(sq, occ, dirs));
        occ = occ.wrapping_sub(mask) & mask;
        if occ == 0 {
            break;
        }
    }

    // `epoch` marks which slots the current candidate has written, so the
    // table is not cleared between candidates.
    let mut table = vec![0u64; size];
    let mut epoch = vec![0u64; size];
    let mut tries = 0u64;
    loop {
        tries += 1;
        let magic = rng.sparse();
        // Cheap reject: a candidate whose product with the mask has too few
        // high bits cannot spread the index well.
        if (mask.wrapping_mul(magic) >> 56).count_ones() < 6 {
            continue;
        }
        let ok = occs.iter().zip(&refs).all(|(&occ, &want)| {
            let idx = ((occ & mask).wrapping_mul(magic) >> shift) as usize;
            if epoch[idx] == tries {
                table[idx] == want
            } else {
                epoch[idx] = tries;
                table[idx] = want;
                true
            }
        });
        if ok {
            return Found { magic, tries };
        }
    }
}

fn table_size(dirs: &[(i32, i32)]) -> usize {
    (0..64)
        .map(|sq| 1usize << mask(sq, dirs).count_ones())
        .sum()
}

fn emit(out: &mut String, name: &str, found: &[Found]) {
    let _ = writeln!(out, "#[rustfmt::skip]\nconst {name}: [u64; 64] = [");
    for row in found.chunks(4) {
        let cells: Vec<String> = row.iter().map(|f| format!("0x{:016X}", f.magic)).collect();
        let _ = writeln!(out, "    {},", cells.join(", "));
    }
    let _ = writeln!(out, "];");
}

pub fn run(args: &[String]) -> Result<(), String> {
    let seed = match args.first() {
        None => DEFAULT_SEED,
        Some(s) => parse_seed(s)?,
    };
    let mut rng = Rng(seed);

    let rooks: Vec<Found> = (0..64).map(|sq| search(sq, &ROOK_DIRS, &mut rng)).collect();
    let bishops: Vec<Found> = (0..64)
        .map(|sq| search(sq, &BISHOP_DIRS, &mut rng))
        .collect();

    let mut out = String::new();
    let _ = writeln!(out, "// Generated by `cadence-tools magics 0x{seed:016X}`.");
    let _ = writeln!(
        out,
        "// Plain magics, shift = 64 - popcount(mask); rook table {} entries, bishop table {}.",
        table_size(&ROOK_DIRS),
        table_size(&BISHOP_DIRS)
    );
    let _ = writeln!(
        out,
        "// Candidates tried: {} rook, {} bishop.",
        rooks.iter().map(|f| f.tries).sum::<u64>(),
        bishops.iter().map(|f| f.tries).sum::<u64>()
    );
    emit(&mut out, "ROOK_MAGIC_NUMBERS", &rooks);
    emit(&mut out, "BISHOP_MAGIC_NUMBERS", &bishops);
    print!("{out}");
    Ok(())
}

fn parse_seed(s: &str) -> Result<u64, String> {
    let (digits, radix) = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (s, 10),
    };
    u64::from_str_radix(&digits.replace('_', ""), radix)
        .map_err(|e| format!("`{s}` is not a seed: {e}"))
}
