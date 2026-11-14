// SPDX-License-Identifier: GPL-3.0-or-later

//! Datasets of packed chunks to the trainer's position records: each chunk held to its manifest
//! first, then filtered, split by game and shuffled from a recorded seed. The outputs are published
//! as one unit, a directory whose manifest is written last.

#![expect(
    dead_code,
    reason = "nothing reads the request until the converter is built"
)]

use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const BUCKETS: usize = 42;

/// What the command line asks.
#[derive(Clone, Debug)]
pub struct Request {
    pub manifests: Vec<PathBuf>,
    pub out: PathBuf,
    pub every: u32,
    pub shuffle_seed: u64,
    pub standard_share: Option<u8>,
    pub score_cap: Option<i16>,
    pub swing_cap: Option<i16>,
    pub dedup: bool,
}

/// # Errors
///
/// Not yet.
pub fn run(_args: &[String]) -> Result<ExitCode, String> {
    Err("not built".to_string())
}

/// # Errors
///
/// Not yet.
pub fn convert(
    _request: &Request,
    _rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, String> {
    Err("not built".to_string())
}

fn bucket(_score: i16) -> usize {
    0
}

fn result_for_mover(_outcome: u8, _white_to_move: bool) -> u8 {
    0
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;

    use super::{BUCKETS, Request, bucket, convert, result_for_mover};
    use cadence_dataset::chunk::{Chunk, Header, chunk_name};
    use cadence_dataset::sha256::{hex, sha256};
    use cadence_dataset::trainer::{DRAW, LOSS, WIN};

    /// The unit is renamed whole into place, so a rename that fails leaves neither the unit nor its
    /// staging behind.
    #[test]
    fn a_failed_rename_publishes_nothing() {
        let dir = std::env::temp_dir().join(format!("cadence-convert-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let header = Header {
            commit: [1; 20],
            version: "0.0.0-test".to_string(),
            target: "test".to_string(),
            features: String::new(),
            nodes: 5_000,
            screen_nodes: 5_000,
            hash_mb: 16,
            random_plies: 8,
            window: 200,
            standard_share: 50,
            run_seed: 1,
            chunk_index: 0,
            games_planned: 1,
            host: "test".to_string(),
            start_time: 0,
            net_sha256: [0; 32],
            modified: 0,
            cfg_sha256: [0; 32],
        };
        let bytes = Chunk::bytes(&header, &[]).expect("an empty chunk");
        let (name, digest) = (chunk_name(1, 0), hex(&sha256(&bytes)));
        std::fs::write(dir.join(&name), &bytes).expect("writes");
        std::fs::write(
            dir.join(format!("{name}.sha256")),
            format!("{digest}  {name}\n"),
        )
        .expect("writes");
        let manifest = dir.join("MANIFEST.tsv");
        std::fs::write(&manifest, format!("chunk\tsha256\n{name}\t{digest}\n")).expect("writes");
        let out = dir.join("out");
        let request = Request {
            manifests: vec![manifest],
            out: out.clone(),
            every: 10,
            shuffle_seed: 1,
            standard_share: None,
            score_cap: None,
            swing_cap: None,
            dedup: false,
        };
        let failed = convert(&request, &mut |_, _| Err(io::Error::other("injected")));
        let left: Vec<PathBuf> = std::fs::read_dir(&out)
            .expect("lists")
            .map(|e| e.expect("an entry").path())
            .collect();
        let published = convert(&request, &mut |from, to| std::fs::rename(from, to));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(failed.is_err(), "a failed rename was taken as published");
        assert_eq!(left, [] as [PathBuf; 0]);
        published.expect("the same request publishes once the rename works");
    }

    #[test]
    fn a_score_lands_in_its_hundred_and_the_tails_hold_the_rest() {
        assert_eq!(bucket(-2_001), 0);
        assert_eq!(bucket(-2_000), 1);
        assert_eq!(bucket(-1), 20);
        assert_eq!(bucket(0), 21);
        assert_eq!(bucket(1_999), BUCKETS - 2);
        assert_eq!(bucket(2_000), BUCKETS - 1);
    }

    /// White's result, turned to the side to move's.
    #[test]
    fn the_result_is_the_movers() {
        assert_eq!(result_for_mover(0, true), WIN);
        assert_eq!(result_for_mover(0, false), LOSS);
        assert_eq!(result_for_mover(1, false), WIN);
        assert_eq!(result_for_mover(1, true), LOSS);
        assert_eq!(result_for_mover(2, true), DRAW);
    }
}
