// SPDX-License-Identifier: GPL-3.0-or-later

//! The recipe that trains the engine's network, and the export of one of its checkpoints to the
//! file the engine loads.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bullet_lib::game::inputs::Chess768;
use bullet_lib::nn::optimiser::AdamW;
use bullet_lib::trainer::save::SavedFormat;
use bullet_lib::trainer::schedule::{TrainingSchedule, TrainingSteps, lr, wdl};
use bullet_lib::trainer::settings::LocalSettings;
use bullet_lib::value::ValueTrainerBuilder;
use bullet_lib::value::loader::DirectSequentialDataLoader;
use cadence_core::features::NUM_INPUTS;
use cadence_dataset::sha256::{hex, sha256};
use cadence_nnue::net::{QA, QB, SCALE};
use cadence_nnue::{HIDDEN, Provenance};
use cadence_train::data::BOOK_HEADER;

const SUPERBATCHES: usize = 40;
const BATCH_SIZE: usize = 16_384;
const BATCHES_PER_SUPERBATCH: usize = 6_104;
/// The share of the target the game's result supplies, the search's score supplying the rest.
const RESULT_SHARE: f32 = 0.75;
const INITIAL_LR: f32 = 0.001;
/// Five steps of 0.3 below the first rate by the last superbatch, decayed along a cosine.
const FINAL_LR: f32 = INITIAL_LR * 0.3 * 0.3 * 0.3 * 0.3 * 0.3;
const SAVE_RATE: usize = 10;
const THREADS: usize = 2;
const BATCH_QUEUE: usize = 32;

const COMMIT: &str = env!("CADENCE_TRAIN_COMMIT");
const DIRTY: &str = env!("CADENCE_TRAIN_DIRTY");
/// Written beside the checkpoints, so an export names the commit that trained them, not its own.
const COMMIT_FILE: &str = "trainer-commit";

const USAGE: &str = "usage: cadence-train run --data FILE... --manifest FILE... --out DIR --id NAME
           [--superbatches N] [--batches N] [--result-share F]
       cadence-train export --checkpoint DIR --manifest FILE --provenance NAME --out FILE
       cadence-train book --in FILE --out FILE --seed HEX";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.split_first() {
        Some((command, rest)) if command == "run" => flags(rest).and_then(|f| run(&f)),
        Some((command, rest)) if command == "export" => flags(rest).and_then(|f| export(&f)),
        Some((command, rest)) if command == "book" => flags(rest).and_then(|f| book(&f)),
        _ => Err(USAGE.to_string()),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cadence-train: {e}");
            ExitCode::FAILURE
        }
    }
}

type Flags = BTreeMap<String, Vec<String>>;

/// `--name value` pairs; a name given twice keeps both values, in order.
fn flags(args: &[String]) -> Result<Flags, String> {
    let mut out = Flags::new();
    let mut words = args.iter();
    while let Some(name) = words.next() {
        let value = words
            .next()
            .filter(|_| name.starts_with("--"))
            .ok_or_else(|| format!("`{name}` is not a flag with a value\n{USAGE}"))?;
        out.entry(name.clone()).or_default().push(value.clone());
    }
    Ok(out)
}

fn one<'a>(flags: &'a Flags, name: &str) -> Result<&'a str, String> {
    match flags.get(name).map(Vec::as_slice) {
        Some([value]) => Ok(value),
        _ => Err(format!("exactly one {name} is needed\n{USAGE}")),
    }
}

/// A flag's value parsed, or `default` when the flag is absent.
fn parsed<T: std::str::FromStr>(flags: &Flags, name: &str, default: T) -> Result<T, String> {
    match flags.get(name).map(Vec::as_slice) {
        None => Ok(default),
        Some([value]) => value
            .parse()
            .map_err(|_| format!("{name} {value} does not parse")),
        Some(_) => Err(format!("{name} is given more than once")),
    }
}

fn run(flags: &Flags) -> Result<(), String> {
    // Before anything is built: a run trains only on the files its manifests name.
    let manifests: Vec<&Path> = flags
        .get("--manifest")
        .map(|m| m.iter().map(Path::new).collect())
        .ok_or_else(|| {
            format!("run needs --manifest, the manifest of the data it trains on\n{USAGE}")
        })?;
    let data_paths: Vec<&Path> = flags
        .get("--data")
        .map(|d| d.iter().map(Path::new).collect())
        .ok_or_else(|| format!("run needs --data\n{USAGE}"))?;
    cadence_train::data::verify(&data_paths, &manifests)?;
    let superbatches = parsed(flags, "--superbatches", SUPERBATCHES)?;
    let batches = parsed(flags, "--batches", BATCHES_PER_SUPERBATCH)?;
    let result_share = parsed(flags, "--result-share", RESULT_SHARE)?;
    if superbatches == 0 || batches == 0 || !(0.0..=1.0).contains(&result_share) {
        return Err(format!(
            "--superbatches and --batches must be positive and --result-share in [0, 1]\n{USAGE}"
        ));
    }
    let data: Vec<&str> = flags
        .get("--data")
        .map(|d| d.iter().map(String::as_str).collect())
        .ok_or_else(|| format!("run needs --data\n{USAGE}"))?;
    let out = one(flags, "--out")?;
    let id = one(flags, "--id")?;
    if COMMIT.len() != 40 || DIRTY != "0" {
        return Err(format!(
            "a net names the commit that trained it, and this build's is {COMMIT} (modified {DIRTY})"
        ));
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{out}: {e}"))?;
    let commit_file = Path::new(out).join(COMMIT_FILE);
    std::fs::write(&commit_file, format!("{COMMIT}\n"))
        .map_err(|e| format!("{}: {e}", commit_file.display()))?;

    let mut trainer = ValueTrainerBuilder::default()
        .dual_perspective()
        .optimiser(AdamW)
        .inputs(Chess768)
        .save_format(&[
            SavedFormat::id("l0w").round().quantise::<i16>(QA as i16),
            SavedFormat::id("l0b").round().quantise::<i16>(QA as i16),
            SavedFormat::id("l1w").round().quantise::<i16>(QB as i16),
            SavedFormat::id("l1b")
                .round()
                .quantise::<i16>((QA * QB) as i16),
        ])
        .loss_fn(|output, target| output.sigmoid().squared_error(target))
        .build(|builder, stm_inputs, ntm_inputs| {
            let l0 = builder.new_affine("l0", NUM_INPUTS, HIDDEN);
            let l1 = builder.new_affine("l1", 2 * HIDDEN, 1);
            let stm_hidden = l0.forward(stm_inputs).screlu();
            let ntm_hidden = l0.forward(ntm_inputs).screlu();
            l1.forward(stm_hidden.concat(ntm_hidden))
        });
    let schedule = TrainingSchedule {
        net_id: id.to_string(),
        eval_scale: f32::from(i16::try_from(SCALE).unwrap_or(i16::MAX)),
        steps: TrainingSteps {
            batch_size: BATCH_SIZE,
            batches_per_superbatch: batches,
            start_superbatch: 1,
            end_superbatch: superbatches,
        },
        wdl_scheduler: wdl::ConstantWDL {
            value: result_share,
        },
        lr_scheduler: lr::CosineDecayLR {
            initial_lr: INITIAL_LR,
            final_lr: FINAL_LR,
            final_superbatch: superbatches,
        },
        save_rate: SAVE_RATE.min(superbatches),
    };
    let settings = LocalSettings {
        threads: THREADS,
        test_set: None,
        output_directory: out,
        batch_queue_size: BATCH_QUEUE,
    };
    trainer.run(
        &schedule,
        &settings,
        &DirectSequentialDataLoader::new(&data),
    );
    Ok(())
}

fn export(flags: &Flags) -> Result<(), String> {
    let checkpoint = PathBuf::from(one(flags, "--checkpoint")?);
    let manifest_path = one(flags, "--manifest")?;
    let provenance = match one(flags, "--provenance")? {
        "self-play" => Provenance::SelfPlay,
        name => Provenance::External(name.to_string()),
    };
    let out = PathBuf::from(one(flags, "--out")?);
    if out.exists() {
        return Err(format!(
            "{} exists, and an export never replaces a net",
            out.display()
        ));
    }
    let read = |path: &Path| std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
    let quantised = read(&checkpoint.join("quantised.bin"))?;
    let commit_file = checkpoint
        .parent()
        .unwrap_or(Path::new("."))
        .join(COMMIT_FILE);
    let trainer = String::from_utf8_lossy(&read(&commit_file)?)
        .trim()
        .to_string();
    let manifest = read(Path::new(manifest_path))?;
    let net = cadence_train::export::export(&quantised, provenance, &trainer, &manifest)
        .map_err(|e| e.to_string())?;
    std::fs::write(&out, net.to_bytes()).map_err(|e| format!("{}: {e}", out.display()))?;
    println!(
        "{}: provenance {}, trainer {}, manifest {}",
        out.display(),
        net.provenance(),
        net.trainer(),
        hex(&net.manifest())
    );
    Ok(())
}

/// A tuning set's text to the trainer's records, with a manifest the exported net's header hashes.
fn book(flags: &Flags) -> Result<(), String> {
    let input = one(flags, "--in")?;
    let out = PathBuf::from(one(flags, "--out")?);
    let seed_text = one(flags, "--seed")?;
    let seed =
        u64::from_str_radix(seed_text, 16).map_err(|_| format!("--seed {seed_text} is not hex"))?;
    let manifest_path = PathBuf::from(format!("{}.manifest", out.display()));
    for path in [&out, &manifest_path] {
        if path.exists() {
            return Err(format!(
                "{} exists, and a conversion never replaces one",
                path.display()
            ));
        }
    }
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let text = String::from_utf8(bytes.clone()).map_err(|e| format!("{input}: not UTF-8: {e}"))?;
    let (records, read) = cadence_train::book::convert(&text, seed)?;
    let flat: Vec<u8> = records.iter().flatten().copied().collect();
    std::fs::write(&out, &flat).map_err(|e| format!("{}: {e}", out.display()))?;
    let name = Path::new(input)
        .file_name()
        .map_or_else(|| input.into(), |n| n.to_string_lossy());
    let manifest = format!(
        "{BOOK_HEADER}\nsource\t{name}\nsource-sha256\t{}\nsource-bytes\t{}\npositions\t{}\n\
         white-to-move\t{}\nresults-loss-draw-win\t{} {} {}\nscore\tnone, written as 0\n\
         shuffle-seed\t{seed:016x}\nrecords-sha256\t{}\nconverter-commit\t{COMMIT}\n\
         converter-modified\t{DIRTY}\n",
        hex(&sha256(&bytes)),
        bytes.len(),
        read.positions,
        read.white_to_move,
        read.results[0],
        read.results[1],
        read.results[2],
        hex(&sha256(&flat)),
    );
    std::fs::write(&manifest_path, manifest)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    println!(
        "{}: {} positions, {} with White to move, mover's results {:?}; manifest {}",
        out.display(),
        read.positions,
        read.white_to_move,
        read.results,
        manifest_path.display()
    );
    Ok(())
}
