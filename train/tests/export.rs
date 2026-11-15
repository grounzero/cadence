// SPDX-License-Identifier: GPL-3.0-or-later

//! A checkpoint laid out as the trainer saves it, exported and loaded by the engine: the weights
//! arrive unchanged, and each fault the export refuses is named.

use cadence_dataset::sha256::sha256;
use cadence_nnue::net::{HEADER_LEN, PAYLOAD_LEN};
use cadence_nnue::{LoadError, Network, Provenance};
use cadence_train::export::{CONVERTED, ExportError, PADDING, export};

const TRAINER: &str = "0123456789abcdef0123456789abcdef01234567";

/// The weights of a seeded net in the trainer's order, zero-padded as it pads a saved file.
fn checkpoint() -> Vec<u8> {
    let mut bytes = Network::seeded(3, Provenance::SelfPlay).to_bytes()[HEADER_LEN..].to_vec();
    bytes.resize(PAYLOAD_LEN.next_multiple_of(PADDING), 0);
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
