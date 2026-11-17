// SPDX-License-Identifier: GPL-3.0-or-later

//! Each saved checkpoint is loaded as an export would load it: one the engine's loader refuses
//! stops the run, and one still being written waits for the next scan.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cadence_core::features::NUM_INPUTS;
use cadence_nnue::net::{HEADER_LEN, PAYLOAD_LEN};
use cadence_nnue::{HIDDEN, Network, Provenance};
use cadence_train::checkpoint::{OUTPUT_SUMS, check_saved, output_sum};
use cadence_train::export::{FILL, PADDING};

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("cadence-checkpoint-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A payload padded as the pinned trainer pads a saved file.
fn padded(payload: &[u8]) -> Vec<u8> {
    let mut bytes = payload.to_vec();
    for i in 0..PAYLOAD_LEN.next_multiple_of(PADDING) - PAYLOAD_LEN {
        bytes.push(FILL[i % FILL.len()]);
    }
    bytes
}

/// Every output weight `w` and every other weight and bias zero.
fn outputs_at(w: i16) -> Vec<u8> {
    let words: Vec<i16> = std::iter::repeat_n(0, NUM_INPUTS * HIDDEN + HIDDEN)
        .chain(std::iter::repeat_n(w, 2 * HIDDEN))
        .chain([0])
        .collect();
    padded(
        &words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<u8>>(),
    )
}

fn save(out: &Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(out.join(name)).expect("a checkpoint directory");
    std::fs::write(out.join(name).join("quantised.bin"), bytes).expect("writes");
}

/// Every output weight at 70 is inside the trainer's clip and the loader's range for one weight,
/// and sums past the loader's bound, so the scan refuses that checkpoint by name.
#[test]
fn a_checkpoint_the_loader_refuses_stops_the_run() {
    let out = scratch("refused");
    let good = padded(&Network::seeded(3, Provenance::SelfPlay).to_bytes()[HEADER_LEN..]);
    save(&out, "net-10", &good);
    save(&out, "net-20", &outputs_at(70));
    let mut seen = BTreeSet::new();
    let checked = check_saved(&out, "net", &mut seen);
    let sums = std::fs::read_to_string(out.join(OUTPUT_SUMS));
    let _ = std::fs::remove_dir_all(&out);
    assert_eq!(output_sum(&outputs_at(70)), 70 * 2 * HIDDEN as u64);
    let error = checked.expect_err("a checkpoint the loader refuses passed");
    assert!(error.contains("net-20"), "{error}");
    let want = format!("net-10\t{}\n", output_sum(&good));
    assert_eq!(sums.expect("the sums file"), want);
}

/// A quantised file shorter than a whole one is still being written, so one scan leaves it and
/// the next judges it.
#[test]
fn a_checkpoint_still_being_written_is_left_for_the_next_scan() {
    let out = scratch("partial");
    let whole = outputs_at(1);
    save(&out, "net-10", &whole[..whole.len() / 2]);
    let mut seen = BTreeSet::new();
    let first = check_saved(&out, "net", &mut seen);
    let early = out.join(OUTPUT_SUMS).exists();
    save(&out, "net-10", &whole);
    let second = check_saved(&out, "net", &mut seen);
    let sums = std::fs::read_to_string(out.join(OUTPUT_SUMS));
    let _ = std::fs::remove_dir_all(&out);
    first.expect("a partial file is not judged");
    second.expect("the whole file loads");
    assert!(!early, "a partial file was recorded");
    assert_eq!(
        sums.expect("the sums file"),
        format!("net-10\t{}\n", 2 * HIDDEN)
    );
}
