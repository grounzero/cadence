// SPDX-License-Identifier: GPL-3.0-or-later

//! Each saved checkpoint is loaded as an export would load it: one the engine's loader refuses
//! stops the run, and one still being written waits for the next scan.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cadence_core::features::NUM_INPUTS;
use cadence_nnue::net::{HEADER_LEN, payload_len};
use cadence_nnue::{HIDDEN, Network, OUTPUT_BUCKETS, Provenance, WIDE};
use cadence_train::checkpoint::{OUTPUT_SUMS, check_all, check_saved, output_sum};
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
    for i in 0..payload.len().next_multiple_of(PADDING) - payload.len() {
        bytes.push(FILL[i % FILL.len()]);
    }
    bytes
}

/// Every output weight `w` and every other weight and bias zero.
fn outputs_at(w: i16) -> Vec<u8> {
    outputs_of_width(HIDDEN, w)
}

/// `outputs_at` for a net of `hidden` units a perspective.
fn outputs_of_width(hidden: usize, w: i16) -> Vec<u8> {
    let words: Vec<i16> = std::iter::repeat_n(0, NUM_INPUTS * hidden + hidden)
        .chain(std::iter::repeat_n(w, 2 * hidden))
        .chain([0])
        .collect();
    assert_eq!(words.len() * 2, payload_len(hidden, 1));
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

/// An output weight of 128 is outside the loader's range for one, so the scan refuses that
/// checkpoint by name after recording the one before it.
#[test]
fn a_checkpoint_the_loader_refuses_stops_the_run() {
    let out = scratch("refused");
    let good = padded(&Network::seeded(3, Provenance::SelfPlay).to_bytes()[HEADER_LEN..]);
    save(&out, "net-10", &good);
    save(&out, "net-20", &outputs_at(128));
    let mut seen = BTreeSet::new();
    let checked = check_saved(&out, "net", HIDDEN, 1, &mut seen);
    let sums = std::fs::read_to_string(out.join(OUTPUT_SUMS));
    let _ = std::fs::remove_dir_all(&out);
    assert_eq!(output_sum(&outputs_at(128), 1), 128 * 2 * HIDDEN as u64);
    let error = checked.expect_err("a checkpoint the loader refuses passed");
    assert!(error.contains("net-20"), "{error}");
    let want = format!("net-10\t{}\n", output_sum(&good, 1));
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
    let first = check_saved(&out, "net", HIDDEN, 1, &mut seen);
    let early = out.join(OUTPUT_SUMS).exists();
    save(&out, "net-10", &whole);
    let second = check_saved(&out, "net", HIDDEN, 1, &mut seen);
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

/// After training, a checkpoint the run was to save that no scan could load, here one byte short,
/// is refused by name, and once it is whole the count is of every checkpoint expected.
#[test]
fn an_expected_checkpoint_never_loaded_is_refused_after_training() {
    let out = scratch("expected");
    let whole = outputs_at(1);
    save(&out, "net-10", &whole);
    save(&out, "net-20", &whole[..whole.len() - 1]);
    let expected = [out.join("net-10"), out.join("net-20")];
    let refused = check_all(&out, "net", HIDDEN, 1, &expected, &mut BTreeSet::new());
    save(&out, "net-20", &whole);
    let checked = check_all(&out, "net", HIDDEN, 1, &expected, &mut BTreeSet::new());
    let _ = std::fs::remove_dir_all(&out);
    let error = refused.expect_err("a checkpoint never loaded passed");
    assert!(error.contains("net-20"), "{error}");
    assert_eq!(checked.expect("both load"), 2);
}

/// A wide run's checkpoints are judged at its width: a whole wide file loads and its output sum is
/// read over its 1,024 output weights, and one whose sum could leave `i32` stops the run by name.
#[test]
fn a_wide_runs_checkpoints_are_judged_at_its_width() {
    let out = scratch("wide");
    let good = padded(&Network::<WIDE>::generate(3, Provenance::SelfPlay).to_bytes()[HEADER_LEN..]);
    save(&out, "net-10", &good);
    save(&out, "net-20", &outputs_of_width(WIDE, 65));
    let mut seen = BTreeSet::new();
    let checked = check_saved(&out, "net", WIDE, 1, &mut seen);
    let sums = std::fs::read_to_string(out.join(OUTPUT_SUMS));
    let _ = std::fs::remove_dir_all(&out);
    assert_eq!(
        output_sum(&outputs_of_width(WIDE, 65), 1),
        65 * 2 * WIDE as u64
    );
    let error = checked.expect_err("a wide checkpoint whose sum could overflow passed");
    assert!(error.contains("net-20"), "{error}");
    let want = format!("net-10\t{}\n", output_sum(&good, 1));
    assert_eq!(sums.expect("the sums file"), want);
}

/// A bucketed run's checkpoints are judged at its bucket count: a whole bucketed file loads and its
/// output sum is its largest bucket's, one with a weight of 128 in its last bucket stops the run by
/// name, and a one-bucket file is never whole at this count, so it is refused after training.
#[test]
fn a_bucketed_runs_checkpoints_are_judged_at_its_bucket_count() {
    let out = scratch("bucketed");
    let net = Network::<HIDDEN>::generate_bucketed(3, Provenance::SelfPlay);
    let payload = &net.to_bytes()[HEADER_LEN..];
    let good = padded(payload);
    let mut words: Vec<i16> = payload
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&w| i16::from_le_bytes(w))
        .collect();
    let outputs = NUM_INPUTS * HIDDEN + HIDDEN;
    let row = 2 * HIDDEN;
    let largest = (0..OUTPUT_BUCKETS)
        .map(|b| {
            let bucket = &words[outputs + b * row..outputs + (b + 1) * row];
            bucket
                .iter()
                .map(|w| u64::from(w.unsigned_abs()))
                .sum::<u64>()
        })
        .max();
    words[outputs + (OUTPUT_BUCKETS - 1) * row] = 128;
    let refused: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    save(&out, "net-10", &good);
    save(&out, "net-20", &padded(&refused));
    let mut seen = BTreeSet::new();
    let checked = check_saved(&out, "net", HIDDEN, OUTPUT_BUCKETS, &mut seen);
    let sums = std::fs::read_to_string(out.join(OUTPUT_SUMS));
    std::fs::remove_dir_all(out.join("net-20")).expect("removes");
    save(&out, "net-20", &outputs_at(1));
    let expected = [out.join("net-10"), out.join("net-20")];
    let unread = check_all(
        &out,
        "net",
        HIDDEN,
        OUTPUT_BUCKETS,
        &expected,
        &mut BTreeSet::new(),
    );
    let _ = std::fs::remove_dir_all(&out);
    assert_eq!(Some(output_sum(&good, OUTPUT_BUCKETS)), largest);
    let error = checked.expect_err("a bucketed checkpoint with a weight of 128 passed");
    assert!(error.contains("net-20"), "{error}");
    let want = format!("net-10\t{}\n", output_sum(&good, OUTPUT_BUCKETS));
    assert_eq!(sums.expect("the sums file"), want);
    let error = unread.expect_err("a one-bucket file in a bucketed run passed");
    assert!(error.contains("net-20"), "{error}");
}
