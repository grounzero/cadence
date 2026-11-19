// SPDX-License-Identifier: GPL-3.0-or-later

//! A checkpoint laid out as the trainer saves it, exported and loaded by the engine: the weights
//! arrive unchanged, and each fault the export refuses is named.

use std::path::Path;
use std::process::Command;

use cadence_dataset::sha256::sha256;
use cadence_nnue::net::{HEADER_LEN, PAYLOAD_LEN};
use cadence_nnue::{LoadError, Network, Provenance};
use cadence_train::data::{COMMIT_FILE, record_manifests};
use cadence_train::export::{CONVERTED, ExportError, PADDING, export};

const TRAINER: &str = "0123456789abcdef0123456789abcdef01234567";

/// The weights of a seeded net in the trainer's order, padded as the pinned trainer pads a saved
/// file: its name repeated from the first byte past the weights.
fn checkpoint() -> Vec<u8> {
    let mut bytes = Network::seeded(3, Provenance::SelfPlay).to_bytes()[HEADER_LEN..].to_vec();
    let fill = b"bullet";
    for i in 0..PAYLOAD_LEN.next_multiple_of(PADDING) - PAYLOAD_LEN {
        bytes.push(fill[i % fill.len()]);
    }
    bytes
}

fn manifest() -> Vec<u8> {
    format!("{CONVERTED}converter-commit\t{TRAINER}\n").into_bytes()
}

#[test]
fn a_checkpoint_exports_to_a_file_the_engine_loads_unchanged() {
    let saved = checkpoint();
    let net = export(&saved, Provenance::SelfPlay, TRAINER, &manifest()).expect("exports");
    let file = net.to_bytes();
    let back = Network::for_datagen(&file).expect("the engine loads it, for datagen too");
    assert_eq!(&file[HEADER_LEN..], &saved[..PAYLOAD_LEN]);
    assert_eq!(back.trainer(), TRAINER);
    assert_eq!(back.manifest(), sha256(&manifest()));
    assert_eq!(*back.provenance(), Provenance::SelfPlay);
}

#[test]
fn each_fault_is_refused_by_name() {
    let saved = checkpoint();
    let external = || Provenance::External("lichess-big3-resolved".to_string());
    let attempt = |bytes: &[u8], provenance, trainer, manifest: &[u8]| {
        export(bytes, provenance, trainer, manifest).err()
    };
    assert!(matches!(
        attempt(
            &saved[..PAYLOAD_LEN],
            Provenance::SelfPlay,
            TRAINER,
            &manifest()
        ),
        Some(ExportError::Length { .. })
    ));
    let mut padded = saved.clone();
    *padded.last_mut().expect("a byte") = 1;
    assert_eq!(
        attempt(&padded, Provenance::SelfPlay, TRAINER, &manifest()),
        Some(ExportError::Padding)
    );
    let mut zeroed = saved.clone();
    zeroed[PAYLOAD_LEN..].fill(0);
    assert_eq!(
        attempt(&zeroed, Provenance::SelfPlay, TRAINER, &manifest()),
        Some(ExportError::Padding),
        "the pinned trainer never pads with zeros"
    );
    assert_eq!(
        attempt(&saved, Provenance::SelfPlay, TRAINER, b"a list of files\n"),
        Some(ExportError::NotConverted)
    );
    assert!(attempt(&saved, external(), TRAINER, b"a list of files\n").is_none());
    assert_eq!(
        attempt(&saved, Provenance::SelfPlay, "unknown", &manifest()),
        Some(ExportError::Load(LoadError::Trainer))
    );
    let mut wide = saved.clone();
    wide[PAYLOAD_LEN - 2..PAYLOAD_LEN].copy_from_slice(&i16::MAX.to_le_bytes());
    let mut output = wide.clone();
    output[PAYLOAD_LEN - 4..PAYLOAD_LEN - 2].copy_from_slice(&200_i16.to_le_bytes());
    assert!(matches!(
        attempt(&output, Provenance::SelfPlay, TRAINER, &manifest()),
        Some(ExportError::Load(LoadError::OutputWeight { .. }))
    ));
}

/// The command reads the record its run wrote beside the checkpoints: trained against manifest A,
/// a checkpoint exports with A and is refused with B, and nothing is written.
#[test]
fn an_export_refuses_a_manifest_its_run_did_not_verify() {
    let dir = std::env::temp_dir().join(format!("cadence-export-verified-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let run = dir.join("checkpoints");
    let saved = run.join("net-10");
    std::fs::create_dir_all(&saved).expect("a checkpoint directory");
    std::fs::write(saved.join("quantised.bin"), checkpoint()).expect("writes");
    std::fs::write(run.join(COMMIT_FILE), format!("{TRAINER}\n")).expect("writes");
    let (a, b) = (dir.join("a.tsv"), dir.join("b.tsv"));
    std::fs::write(&a, manifest()).expect("writes");
    std::fs::write(
        &b,
        format!("{CONVERTED}converter-commit\t{}\n", "f".repeat(40)),
    )
    .expect("writes");
    record_manifests(&run, &[&a]).expect("records");
    let export = |manifest: &Path, out: &Path, provenance: &str| {
        let done = Command::new(env!("CARGO_BIN_EXE_cadence-train"))
            .arg("export")
            .arg("--checkpoint")
            .arg(&saved)
            .arg("--manifest")
            .arg(manifest)
            .args(["--provenance", provenance, "--out"])
            .arg(out)
            .output()
            .expect("cadence-train runs");
        (
            done.status.success(),
            String::from_utf8_lossy(&done.stderr).into_owned(),
        )
    };
    let (ok, text) = export(&b, &dir.join("b.cdnn"), "self-play");
    assert!(
        !ok && text.contains("is not one the run verified"),
        "{text}"
    );
    assert!(!dir.join("b.cdnn").exists());
    let (ok, text) = export(&a, &dir.join("a.cdnn"), "self-play");
    assert!(ok, "{text}");
    let (ok, text) = export(&a, &dir.join("c.cdnn"), "canary");
    let canary = std::fs::read(dir.join("c.cdnn"));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "{text}");
    let canary = canary.expect("written");
    let canary = Network::from_bytes(&canary).expect("loads");
    assert_eq!(*canary.provenance(), Provenance::Canary);
}

/// A canary net is exported only from data the converter wrote, as a self-play net is.
#[test]
fn a_canary_exports_only_from_converted_data() {
    let saved = checkpoint();
    let refused = export(&saved, Provenance::Canary, TRAINER, b"not converted\n");
    assert_eq!(refused.err(), Some(ExportError::NotConverted));
    let net = export(&saved, Provenance::Canary, TRAINER, &manifest()).expect("exports");
    let back = Network::from_bytes(&net.to_bytes()).expect("loads");
    assert_eq!(*back.provenance(), Provenance::Canary);
}
