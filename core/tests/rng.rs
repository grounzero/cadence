// SPDX-License-Identifier: GPL-3.0-or-later

//! The gate for `rng`. The known answers are splitmix64's published reference outputs for seed
//! zero, so the stream is pinned to the algorithm and not only to itself.

use cadence_core::rng::{Rng, splitmix64};

#[test]
fn seed_zero_gives_the_reference_outputs() {
    let mut rng = Rng::new(0);
    let got: Vec<u64> = (0..5).map(|_| rng.next_u64()).collect();
    assert_eq!(
        got,
        [
            0xE220_A839_7B1D_CDAF,
            0x6E78_9E6A_A1B9_65F4,
            0x06C4_5D18_8009_454F,
            0xF88B_B8A8_724C_81EC,
            0x1B39_896A_51A8_749B,
        ]
    );
}

#[test]
fn the_stream_is_the_step_applied_to_its_own_state() {
    let mut rng = Rng::new(0x00CA_DE7C_E5EE_D002);
    let mut state = 0x00CA_DE7C_E5EE_D002;
    for _ in 0..1000 {
        let (out, next) = splitmix64(state);
        state = next;
        assert_eq!(rng.next_u64(), out);
    }
}

#[test]
fn below_stays_in_range_and_reaches_every_value() {
    for n in [1usize, 2, 3, 7, 960] {
        let mut rng = Rng::new(n as u64);
        let mut seen = vec![0u32; n];
        for _ in 0..200 * n {
            let d = rng.below(n);
            assert!(d < n, "below({n}) gave {d}");
            seen[d] += 1;
        }
        // Two hundred expected per value, so under fifty is a skew and not chance.
        assert!(seen.iter().all(|&c| c > 50), "below({n}): {seen:?}");
    }
}

#[test]
#[should_panic(expected = "below(0)")]
fn below_zero_panics() {
    Rng::new(1).below(0);
}

#[test]
fn a_clone_continues_the_same_stream() {
    let mut a = Rng::new(42);
    a.next_u64();
    let mut b = a.clone();
    for _ in 0..100 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
}
