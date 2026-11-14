// SPDX-License-Identifier: GPL-3.0-or-later

//! The packed writer: chunks of games under a run seed, closed whole, paused or stopped by files in
//! its directory. A kill leaves a `.tmp` of whole records that `repair` closes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cadence_core::rng::Rng;
use cadence_dataset::chunk::GameRecord;

use super::opening::Probe;
use crate::tt::Table;

#[must_use]
pub fn chunk_seed(_run_seed: u64, _chunk_index: u32) -> u64 {
    0
}

pub fn start_kind(_rng: &mut Rng, _share: u8) -> u8 {
    0
}

#[must_use]
pub fn play(chunk_seed: u64, index: u32, share: u8, tt: &Table) -> GameRecord {
    play_with(chunk_seed, index, share, tt, &|_, _| None)
}

pub fn play_with(_: u64, index: u32, _: u8, _: &Table, _: &Probe<'_>) -> GameRecord {
    GameRecord {
        index,
        seed: 0,
        kind: 0,
        white: 0,
        black: 0,
        plies: Vec::new(),
        moves: Vec::new(),
        outcome: 0,
        ending: 0,
        refused: 0,
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub dir: PathBuf,
    pub run_seed: u64,
    pub threads: usize,
    pub chunk_games: u32,
    pub standard_share: u8,
    pub host: String,
    pub ahead: u32,
    pub poll: Duration,
    pub status_every: Duration,
}

#[derive(Clone, Debug)]
pub struct Build {
    pub commit_full: String,
    pub modified: bool,
    pub version: String,
    pub target: String,
    pub features: String,
    pub cfg_features: String,
}

#[must_use]
pub fn build_refusal(_commit_full: &str, _modified: bool) -> Option<&'static str> {
    None
}

pub type Player<'a> = dyn Fn(u64, u32, u8, &Table) -> GameRecord + Sync + 'a;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub chunks: u32,
    pub games: [u64; 2],
}

/// # Errors
///
/// Not yet.
pub fn run(_settings: &Settings, _build: &Build, _player: &Player<'_>) -> Result<Written, String> {
    Err("not built".to_string())
}

/// # Errors
///
/// Not yet.
pub fn parse_seed(_value: &str) -> Result<u64, String> {
    Err("not built".to_string())
}

#[must_use]
pub fn utc(_secs: u64) -> String {
    String::new()
}

/// # Errors
///
/// Not yet.
pub fn repair(_tmp: &Path) -> Result<String, String> {
    Err("not built".to_string())
}
