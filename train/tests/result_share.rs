// SPDX-License-Identifier: GPL-3.0-or-later

//! The result share a run trained with, recorded beside its checkpoints and read back by
//! validation in place of a default.

mod common;

use cadence_train::data::{record_result_share, run_result_share};
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
