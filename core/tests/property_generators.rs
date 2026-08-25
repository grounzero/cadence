// SPDX-License-Identifier: GPL-3.0-or-later

//! Self-checks on the generative machinery.
//!
//! These pass today. They assert nothing about the engine: they assert that
//! the position generator the property tests rely on produces the positions it
//! claims to, which is what makes those tests' failures meaningful rather than
//! an artefact of a broken generator.
//!
//! The same argument as the corpus integrity tests, applied to generated data
//! instead of read data.

mod support;

use support::generative as generate;

/// The Scharnagl decoder against every index the corpus names (twenty arrays,
/// two indices each, including 0, 518 and 959).
///
/// This is the only external check available on the decoder. If it agrees with
/// the corpus on forty independently computed back ranks including both
/// extremes and the standard array, it is right.
#[test]
fn scharnagl_matches_the_corpus() {
    for a in support::dfrc_arrays() {
        let generated = generate::dfrc_start_fen(a.wid, a.bid);
        assert_eq!(
            generated, a.fen,
            "array {}/{}: the generator and the corpus disagree",
            a.wid, a.bid
        );
    }
}

/// 960 distinct arrays, each with the king strictly between its rooks.
///
/// The betweenness is not decoration: `castle_side` is *derived* from
/// `rook_file > king_file` rather than stored, and that derivation is only
/// sound because it holds in all 960.
#[test]
fn all_960_arrays_are_distinct_and_well_formed() {
    let fens = generate::all_960_start_fens();
    assert_eq!(fens.len(), 960);

    let unique: std::collections::BTreeSet<&String> = fens.iter().collect();
    assert_eq!(
        unique.len(),
        960,
        "the generator produced a duplicate array"
    );

    for (n, fen) in fens.iter().enumerate() {
        let back_rank = fen
            .split('/')
            .next_back()
            .and_then(|r| r.split(' ').next())
            .expect("a FEN has a first rank");
        assert_eq!(
            back_rank.len(),
            8,
            "array {n}: back rank is not eight files"
        );

        let king = back_rank
            .find('K')
            .unwrap_or_else(|| panic!("array {n}: no king"));
        let rooks: Vec<usize> = back_rank.match_indices('R').map(|(i, _)| i).collect();
        assert_eq!(rooks.len(), 2, "array {n}: not two rooks");
        assert!(
            rooks[0] < king && king < rooks[1],
            "array {n}: the king is not strictly between its rooks ({back_rank})"
        );

        for (piece, count) in [('Q', 1), ('B', 2), ('N', 2)] {
            assert_eq!(
                back_rank.matches(piece).count(),
                count,
                "array {n}: wrong number of {piece} in {back_rank}"
            );
        }
        // Bishops on opposite colours.
        let bishops: Vec<usize> = back_rank.match_indices('B').map(|(i, _)| i).collect();
        assert_ne!(
            bishops[0] % 2,
            bishops[1] % 2,
            "array {n}: bishops are on the same colour ({back_rank})"
        );
    }
}

/// The RNG is deterministic and seed-separated, so a failing property test can
/// be re-run on the seed that failed.
#[test]
fn rng_is_deterministic_and_seed_separated() {
    let a: Vec<u64> = (0..8).map(|_| generate::Rng::new(1).next_u64()).collect();
    assert!(
        a.iter().all(|x| *x == a[0]),
        "same seed, different first draw"
    );

    let mut r1 = generate::Rng::new(1);
    let mut r2 = generate::Rng::new(1);
    let mut r3 = generate::Rng::new(2);
    let s1: Vec<u64> = (0..64).map(|_| r1.next_u64()).collect();
    let s2: Vec<u64> = (0..64).map(|_| r2.next_u64()).collect();
    let s3: Vec<u64> = (0..64).map(|_| r3.next_u64()).collect();
    assert_eq!(s1, s2, "the same seed gave two different streams");
    assert_ne!(s1, s3, "two seeds gave the same stream");

    let mut r = generate::Rng::new(7);
    for n in [1usize, 2, 3, 17, 218] {
        for _ in 0..1000 {
            assert!(r.below(n) < n, "below({n}) went out of range");
        }
    }
}
