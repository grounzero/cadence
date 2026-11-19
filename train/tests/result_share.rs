// SPDX-License-Identifier: GPL-3.0-or-later

//! The result share and output buckets a run trained with, recorded beside its checkpoints and
//! read back by validation and every reader of its checkpoints in place of a default.

mod common;

use cadence_train::data::{
    OUTPUT_BUCKETS_FILE, fresh_run, record_output_buckets, record_result_share, run_output_buckets,
    run_result_share,
};
use cadence_train::validate::{score_note, score_share};
use common::scratch;

/// Validation reads the share its run recorded; a run that recorded none, or a share given that
/// differs from the recorded one, is refused.
#[test]
fn validation_reads_the_result_share_its_run_recorded() {
    let run = scratch("result-share");
    let recorded = record_result_share(&run, 0.3);
    let read = run_result_share(&run, None);
    let same = run_result_share(&run, Some(0.3));
    let other = run_result_share(&run, Some(0.75));
    let empty = scratch("result-share-none");
    let none = run_result_share(&empty, None);
    let given = run_result_share(&empty, Some(0.75));
    let _ = std::fs::remove_dir_all(&run);
    let _ = std::fs::remove_dir_all(&empty);
    recorded.expect("the share is recorded");
    assert_eq!(read.map(|s| s.to_string()), Ok("0.3".to_string()));
    assert_eq!(same.map(|s| s.to_string()), Ok("0.3".to_string()));
    assert!(
        other.is_err_and(|e| e.contains("0.75") && e.contains("0.3")),
        "a share given that differs from the run's was taken"
    );
    assert!(none.is_err(), "a run that recorded no share was read");
    assert!(given.is_err(), "a given share stood in for a run's record");
}

/// A loss may be scored at a share other than the run's, as comparing nets trained at different
/// blends needs, and each line says which share it was scored at and whether that is the run's.
#[test]
fn a_loss_may_be_scored_at_another_share_and_says_which() {
    assert_eq!(
        score_share(0.75, None).map(|s| s.to_string()),
        Ok("0.75".to_string())
    );
    assert_eq!(
        score_share(0.75, Some(0.5)).map(|s| s.to_string()),
        Ok("0.5".to_string())
    );
    assert!(
        score_share(0.75, Some(1.5)).is_err(),
        "a share past 1 was taken"
    );
    assert_eq!(
        score_note(0.75, 0.75),
        "scored at result share 0.75, the run's"
    );
    assert_eq!(
        score_note(0.5, 0.75),
        "scored at result share 0.5, not the run's 0.75"
    );
}

/// A run's bucket count is recorded beside its checkpoints and read back; a run that recorded none
/// predates the record and reads as one bucket, and a record of any count the engine does not read
/// is refused.
#[test]
fn the_output_buckets_a_run_recorded_are_read_back() {
    let run = scratch("output-buckets");
    let eight = record_output_buckets(&run, 8).and_then(|()| run_output_buckets(&run));
    let one = record_output_buckets(&run, 1).and_then(|()| run_output_buckets(&run));
    let refused: Vec<Result<usize, String>> = ["3\n", "0\n", "eight\n", ""]
        .iter()
        .map(|text| {
            std::fs::write(run.join(OUTPUT_BUCKETS_FILE), text).expect("writes");
            run_output_buckets(&run)
        })
        .collect();
    let fresh = fresh_run(&run);
    let empty = scratch("output-buckets-none");
    let none = run_output_buckets(&empty);
    let _ = std::fs::remove_dir_all(&run);
    let _ = std::fs::remove_dir_all(&empty);
    assert_eq!(eight, Ok(8));
    assert_eq!(one, Ok(1));
    for read in refused {
        assert!(
            read.as_ref().is_err_and(|e| e.contains("is not 1 or 8")),
            "{read:?}"
        );
    }
    assert!(
        fresh.is_err_and(|e| e.contains(OUTPUT_BUCKETS_FILE)),
        "a directory holding a bucket record was taken for a fresh run"
    );
    assert_eq!(none, Ok(1));
}
