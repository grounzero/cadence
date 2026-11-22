// SPDX-License-Identifier: GPL-3.0-or-later

//! `verify` against a chunk the writer made: it passes whole, and each planted fault is refused by
//! the check that owns it, an edited label only by the regeneration.

#![cfg(feature = "packed-datagen")]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cadence_dataset::chunk::{Chunk, HEADER_LEN, Header, TRAILER_LEN, Trailer, chunk_name};
use cadence_dataset::crc32::crc32;
use cadence_dataset::sha256::{hex, sha256};
use cadence_engine::datagen::packed::{self, Build, Settings};
use cadence_engine::datagen::{HASH_MB, verify};
use cadence_engine::tt::Table;
use cadence_engine::version::VERSION;
use cadence_nnue::{AnyNetwork, Network, Provenance, WIDE};

const SEED: u64 = 0xc0de;

/// One field of a header changed.
type Edit = fn(&mut Header);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadence-verify-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// Three real games in one chunk, written by the writer under a clean, known commit.
fn written(dir: &Path, seed: u64) -> PathBuf {
    written_by(dir, seed, None)
}

/// As `written`, played by `net` where one is given and its header naming it.
fn written_by(dir: &Path, seed: u64, net: Option<&AnyNetwork>) -> PathBuf {
    let settings = Settings {
        dir: dir.to_path_buf(),
        run_seed: seed,
        threads: 2,
        chunk_games: 3,
        standard_share: 50,
        host: "test".to_string(),
        ahead: 1_024,
        poll: Duration::from_millis(10),
        status_every: Duration::from_secs(60),
    };
    let build = Build {
        commit_full: "0123456789abcdef0123456789abcdef01234567".to_string(),
        modified: false,
        version: VERSION.to_string(),
        target: "test-target-triple".to_string(),
        features: "+test".to_string(),
        cfg_features: "test".to_string(),
        net_sha256: net.map_or([0; 32], |_| packed::shipped_net_sha256()),
    };
    let stop = |s: u64, index: u32, share: u8, tt: &Table| {
        if index == 2 {
            std::fs::write(dir.join("STOP"), "").expect("writes");
        }
        packed::play(s, index, share, tt, net)
    };
    packed::run(&settings, &build, &stop).expect("a run");
    std::fs::remove_file(dir.join("STOP")).expect("the stop is removed");
    dir.join(chunk_name(seed, 0))
}

/// The bytes written in place of the chunk's, with a sidecar to match.
fn replace(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("writes");
    let name = path
        .file_name()
        .expect("a name")
        .to_string_lossy()
        .into_owned();
    let side = format!("{}  {name}\n", hex(&sha256(bytes)));
    std::fs::write(format!("{}.sha256", path.display()), side).expect("writes");
}

/// The header edited and its CRC in the trailer recomputed, so only the edit can be refused.
fn with_header(bytes: &[u8], edit: impl Fn(&mut Header)) -> Vec<u8> {
    let head: [u8; HEADER_LEN] = bytes[..HEADER_LEN].try_into().expect("a header");
    let mut header = Header::decode(&head).expect("decodes");
    edit(&mut header);
    let head = header.encode().expect("encodes");
    let tail: [u8; TRAILER_LEN] = bytes[bytes.len() - TRAILER_LEN..]
        .try_into()
        .expect("a trailer");
    let mut trailer = Trailer::decode(&tail).expect("decodes");
    trailer.header_crc = crc32(&head);
    let mut out = head.to_vec();
    out.extend_from_slice(&bytes[HEADER_LEN..bytes.len() - TRAILER_LEN]);
    out.extend_from_slice(&trailer.encode());
    out
}

fn check(path: &Path, replay: Option<u32>) -> Result<verify::Summary, String> {
    verify::chunk(path, replay, &Table::new(HASH_MB).expect("a table"), None)
}

#[test]
fn a_written_chunk_verifies_and_regenerates_byte_for_byte() {
    let dir = scratch("whole");
    let path = written(&dir, SEED);
    let plain = check(&path, None);
    let replayed = check(&path, Some(1));
    let _ = std::fs::remove_dir_all(&dir);
    let plain = plain.expect("verifies");
    assert_eq!((plain.games, plain.replayed), (3, 0));
    assert_eq!(plain.kinds[0] + plain.kinds[1], 3);
    assert_eq!(replayed.expect("regenerates").replayed, 3);
}

/// A run playing the build's net names it in the header and regenerates byte for byte with it, and
/// its games are not the handcrafted evaluation's.
#[test]
fn a_chunk_played_by_the_shipped_net_names_it_and_regenerates_with_it() {
    let net = packed::shipped_net()
        .expect("the shipped net loads")
        .expect("the build ships a net");
    let dir = scratch("net");
    let path = written_by(&dir, SEED, Some(&net));
    let bytes = std::fs::read(&path).expect("the chunk");
    let read = Chunk::read(&bytes).expect("reads");
    let tt = Table::new(HASH_MB).expect("a table");
    let replayed = verify::chunk(&path, Some(1), &tt, Some(&net));
    let unloaded = verify::chunk(&path, Some(1), &tt, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(read.header.net_sha256, packed::shipped_net_sha256());
    assert_eq!(replayed.expect("regenerates with the net").replayed, 3);
    assert!(
        unloaded
            .expect_err("regenerated with no net")
            .contains("none was loaded")
    );
    let first = &read.records[0];
    let seed = packed::chunk_seed(SEED, 0);
    let handcrafted = packed::play(seed, first.index, read.header.standard_share, &tt, None);
    assert_ne!(
        handcrafted.encode().expect("encodes"),
        first.encode().expect("encodes"),
        "the net's game is the handcrafted evaluation's"
    );
}

/// A 512-wide self-play net passes the loader runs use and regenerates its run byte for byte, so a
/// build that ships one still generates.
#[test]
fn a_chunk_played_by_a_wide_net_regenerates_with_it() {
    let bytes = Network::<WIDE>::generate(13, Provenance::SelfPlay).to_bytes();
    let net = AnyNetwork::for_datagen(&bytes).expect("a wide self-play net may generate data");
    assert!(matches!(net, AnyNetwork::Wide(_)));
    let dir = scratch("wide");
    let path = written_by(&dir, SEED, Some(&net));
    let tt = Table::new(HASH_MB).expect("a table");
    let replayed = verify::chunk(&path, Some(1), &tt, Some(&net));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(replayed.expect("regenerates with the wide net").replayed, 3);
}

#[test]
fn a_missing_or_wrong_sidecar_is_refused() {
    let dir = scratch("sidecar");
    let path = written(&dir, SEED);
    let side = PathBuf::from(format!("{}.sha256", path.display()));
    let text = std::fs::read_to_string(&side).expect("a sidecar");
    std::fs::write(
        &side,
        text.replacen(&text[..1], if &text[..1] == "0" { "1" } else { "0" }, 1),
    )
    .expect("writes");
    let wrong = check(&path, None);
    std::fs::remove_file(&side).expect("removes");
    let missing = check(&path, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        wrong
            .expect_err("a wrong sidecar passed")
            .contains("its sidecar says")
    );
    assert!(
        missing
            .expect_err("no sidecar passed")
            .contains(".sha256 is missing")
    );
}

/// Each setting edited in turn, the header's CRC recomputed, is refused naming the field; edited
/// without the CRC recomputed, it is refused for the CRC.
#[test]
fn each_header_field_is_held_to_this_build() {
    let dir = scratch("header");
    let path = written(&dir, SEED);
    let whole = std::fs::read(&path).expect("the chunk");
    let edits: [(&str, Edit); 9] = [
        ("nodes", |h| h.nodes = 4_000),
        ("node cap", |h| h.node_cap = 40_000),
        ("screen nodes", |h| h.screen_nodes = 4_000),
        ("hash", |h| h.hash_mb = 8),
        ("random plies", |h| h.random_plies = 7),
        ("window", |h| h.window = 150),
        ("network", |h| h.net_sha256 = [1; 32]),
        ("no commit", |h| h.commit = [0; 20]),
        ("modified tree", |h| h.modified = 1),
    ];
    let mut refused = Vec::new();
    for (field, edit) in edits {
        replace(&path, &with_header(&whole, edit));
        refused.push((field, check(&path, None)));
    }
    let mut flipped = whole.clone();
    flipped[156] ^= 1;
    replace(&path, &flipped);
    let crc = check(&path, None);
    let _ = std::fs::remove_dir_all(&dir);
    for (field, result) in refused {
        let error = result.expect_err(field);
        assert!(error.contains(field), "{field}: {error}");
    }
    assert!(crc.expect_err("a flipped header passed").contains("CRC"));
}

/// A game copied from another run's chunk is found by its seed.
#[test]
fn a_game_whose_seed_is_not_its_chunks_is_refused() {
    let (dir, other) = (scratch("seed"), scratch("seed-other"));
    let path = written(&dir, SEED);
    let foreign =
        Chunk::read(&std::fs::read(written(&other, SEED + 1)).expect("reads")).expect("reads");
    let mut chunk = Chunk::read(&std::fs::read(&path).expect("reads")).expect("reads");
    chunk.records[1] = foreign.records[1].clone();
    replace(
        &path,
        &Chunk::bytes(&chunk.header, &chunk.records).expect("a chunk"),
    );
    let result = check(&path, None);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&other);
    let error = result.expect_err("a foreign game passed");
    assert!(
        error.contains("game 1: seed") && error.contains("where its chunk and index give"),
        "{error}"
    );
}

/// A label edited with every CRC recomputed is legal and seeded right, so only the regeneration
/// finds it.
#[test]
fn an_edited_label_is_found_only_by_the_regeneration() {
    let dir = scratch("label");
    let path = written(&dir, SEED);
    let mut chunk = Chunk::read(&std::fs::read(&path).expect("reads")).expect("reads");
    let score = &mut chunk.records[1].moves[0].1;
    *score = i16::from(*score == 0);
    replace(
        &path,
        &Chunk::bytes(&chunk.header, &chunk.records).expect("a chunk"),
    );
    let plain = check(&path, None);
    let replayed = check(&path, Some(1));
    let _ = std::fs::remove_dir_all(&dir);
    plain.expect("the edit is invisible without the regeneration");
    let error = replayed.expect_err("the regeneration missed it");
    assert!(
        error.contains("game 1: does not regenerate as written"),
        "{error}"
    );
}

#[test]
fn an_illegal_move_or_a_false_ending_is_refused_by_the_replay() {
    let dir = scratch("replay");
    let path = written(&dir, SEED);
    let whole = Chunk::read(&std::fs::read(&path).expect("reads")).expect("reads");
    let mut illegal = whole.clone();
    illegal.records[0].moves[0].0 = 0;
    replace(
        &path,
        &Chunk::bytes(&illegal.header, &illegal.records).expect("a chunk"),
    );
    let moved = check(&path, None);
    let mut false_end = whole.clone();
    false_end.records[0].ending = (false_end.records[0].ending + 1) % 5;
    replace(
        &path,
        &Chunk::bytes(&false_end.header, &false_end.records).expect("a chunk"),
    );
    let ended = check(&path, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        moved
            .expect_err("an illegal move passed")
            .contains("is not legal")
    );
    assert!(
        ended
            .expect_err("a false ending passed")
            .contains("and the record says")
    );
}

fn verify_cli(args: &[&str]) -> (bool, String) {
    let out = Command::new(support::binary())
        .args(["datagen", "verify"])
        .args(args)
        .output()
        .expect("runs");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// The manifest lists exactly the directory's chunks, and is written only when every chunk in it
/// passed and no `.tmp` waits beside them.
#[test]
fn the_manifest_is_written_last_and_only_whole() {
    let dir = scratch("manifest");
    let path = written(&dir, SEED);
    let d = dir.to_str().expect("a path");
    let (ok, stdout) = verify_cli(&[d, "--manifest"]);
    let manifest = std::fs::read_to_string(dir.join("MANIFEST.tsv"));
    std::fs::remove_file(dir.join("MANIFEST.tsv")).expect("removes");
    std::fs::write(dir.join("x.cdg.tmp"), "").expect("writes");
    let (beside_tmp, _) = verify_cli(&[d, "--manifest"]);
    let tmp_left = dir.join("MANIFEST.tsv").exists();
    std::fs::remove_file(dir.join("x.cdg.tmp")).expect("removes");
    let mut bytes = std::fs::read(&path).expect("reads");
    bytes[HEADER_LEN + 4] ^= 1;
    std::fs::write(&path, &bytes).expect("writes");
    let (failed, _) = verify_cli(&[d, "--manifest"]);
    let failed_left = dir.join("MANIFEST.tsv").exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{stdout}");
    assert_eq!(
        stdout.contains("  not examined: each published name's durability"),
        cfg!(not(unix)),
        "{stdout}"
    );
    assert!(
        stdout.contains("  not examined: the regeneration of every game"),
        "{stdout}"
    );
    let manifest = manifest.expect("a manifest");
    let rows: Vec<&str> = manifest
        .lines()
        .skip_while(|l| !l.starts_with("chunk\t"))
        .skip(1)
        .map(|l| l.split('\t').next().unwrap_or(""))
        .collect();
    assert_eq!(rows, [chunk_name(SEED, 0)]);
    assert!(
        manifest.contains("commit\t0123456789abcdef0123456789abcdef01234567"),
        "{manifest}"
    );
    assert!(
        !beside_tmp && !tmp_left,
        "a manifest was written beside a .tmp"
    );
    assert!(
        !failed && !failed_left,
        "a manifest was written over a failing chunk"
    );
}

/// Chunks of one commit and settings built for another target, with other target features or under
/// another configuration are not one dataset, so the manifest refuses the mix.
#[test]
fn the_manifest_refuses_chunks_of_different_builds() {
    let dir = scratch("builds");
    let path = written(&dir, SEED);
    let first = check(&path, None);
    let _ = std::fs::remove_dir_all(&dir);
    let first = first.expect("verifies");
    let edits: [(&str, Edit); 3] = [
        ("target", |h| h.target = "other".to_string()),
        ("features", |h| h.features = "+other".to_string()),
        ("configuration", |h| h.cfg_sha256 = [1; 32]),
    ];
    for (field, edit) in edits {
        let mut other = first.clone();
        other.name = "other".to_string();
        edit(&mut other.header);
        let mixed = verify::manifest(&[first.clone(), other]);
        assert!(
            mixed.is_err_and(|e| e.contains("different builds")),
            "chunks of another {field} were listed together"
        );
    }
    let mut same = first.clone();
    same.name = "same".to_string();
    let one_build = verify::manifest(&[first, same]);
    one_build.expect("one build's chunks are listed");
}

/// The manifest names the run's chunk size, which numbers each game in its run, and refuses chunks
/// planned at another size.
#[test]
fn the_manifest_names_the_chunk_size_and_refuses_a_mix() {
    let dir = scratch("chunk-games");
    let path = written(&dir, SEED);
    let first = check(&path, None);
    let _ = std::fs::remove_dir_all(&dir);
    let first = first.expect("verifies");
    let listed = verify::manifest(std::slice::from_ref(&first));
    let listed = listed.expect("one chunk is listed");
    assert!(listed.contains("\nchunk-games\t3\n"), "{listed}");
    let mut other = first.clone();
    other.name = "other".to_string();
    other.header.games_planned = 4;
    let mixed = verify::manifest(&[first, other]);
    assert!(
        mixed.is_err_and(|e| e.contains("different builds")),
        "chunks planned at two sizes were listed together"
    );
}
