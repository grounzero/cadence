// SPDX-License-Identifier: GPL-3.0-or-later

//! The recipe that trains the engine's network, and the export of one of its checkpoints to the
//! file the engine loads.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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
use cadence_train::data::{BOOK_HEADER, COMMIT_FILE};

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
/// Holdout positions each checkpoint's validation loss is read over; the trainer scores them one
/// at a time, so this bounds what the reading costs.
const VALIDATION_POSITIONS: usize = 20_000;
/// The share of a book's lines held out, one in this many, as the converter holds out games.
const BOOK_EVERY: usize = 10;
/// How often the run's directory is read for a checkpoint saved since the last read.
const WATCH_EVERY: Duration = Duration::from_secs(5);

const COMMIT: &str = env!("CADENCE_TRAIN_COMMIT");
const DIRTY: &str = env!("CADENCE_TRAIN_DIRTY");

const USAGE: &str =
    "usage: cadence-train run --data FILE... --manifest FILE... --holdout FILE --out DIR --id NAME
           [--superbatches N] [--batches N] [--result-share F] [--validation-positions N]
       cadence-train validate --checkpoint DIR... --holdout FILE --manifest FILE...
           [--validation-positions N] [--result-share F]
       cadence-train export --checkpoint DIR --manifest FILE --provenance NAME --out FILE
       cadence-train book --in FILE --out FILE --seed HEX [--every N]
       cadence-train eval --checkpoint DIR --fens FILE";

/// The network and its saved format, built the same way wherever a trainer is needed, so a
/// checkpoint is read back by the definition that wrote it.
macro_rules! recipe {
    () => {
        ValueTrainerBuilder::default()
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
            })
    };
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.split_first() {
        Some((command, rest)) if command == "run" => flags(rest).and_then(|f| run(&f)),
        Some((command, rest)) if command == "export" => flags(rest).and_then(|f| export(&f)),
        Some((command, rest)) if command == "book" => flags(rest).and_then(|f| book(&f)),
        Some((command, rest)) if command == "eval" => flags(rest).and_then(|f| eval(&f)),
        Some((command, rest)) if command == "validate" => flags(rest).and_then(|f| validate(&f)),
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
    let verified = cadence_train::data::matched(&data_paths, &manifests)?;
    let holdout = Path::new(one(flags, "--holdout")?);
    cadence_train::data::verify_holdout(holdout, &verified)
        .map_err(|e| format!("{e}, and only a manifest naming a --data file counts"))?;
    let out = one(flags, "--out")?;
    cadence_train::data::fresh_run(Path::new(out))?;
    let superbatches = parsed(flags, "--superbatches", SUPERBATCHES)?;
    let batches = parsed(flags, "--batches", BATCHES_PER_SUPERBATCH)?;
    let result_share = parsed(flags, "--result-share", RESULT_SHARE)?;
    let positions = parsed(flags, "--validation-positions", VALIDATION_POSITIONS)?;
    if superbatches == 0 || batches == 0 || !(0.0..=1.0).contains(&result_share) {
        return Err(format!(
            "--superbatches and --batches must be positive and --result-share in [0, 1]\n{USAGE}"
        ));
    }
    let data: Vec<&str> = flags
        .get("--data")
        .map(|d| d.iter().map(String::as_str).collect())
        .ok_or_else(|| format!("run needs --data\n{USAGE}"))?;
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
    cadence_train::data::record_manifests(Path::new(out), &verified)?;

    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (stop, out, id) = (Arc::clone(&stop), PathBuf::from(out), id.to_string());
        std::thread::spawn(move || watch(&out, &id, &stop))
    };
    let mut trainer = recipe!();
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
    // Freed before the checkpoints are read back, each into a trainer of its own.
    drop(trainer);
    stop.store(true, Ordering::Relaxed);
    let mut seen = watcher
        .join()
        .map_err(|_| "the checkpoint watcher panicked".to_string())?;
    cadence_train::checkpoint::check_saved(Path::new(out), id, &mut seen)?;
    let save_rate = SAVE_RATE.min(superbatches);
    let checkpoints: Vec<PathBuf> = (1..=superbatches)
        .filter(|sb| sb % save_rate == 0 || *sb == superbatches)
        .map(|sb| Path::new(out).join(format!("{id}-{sb}")))
        .collect();
    validation(&checkpoints, holdout, positions, result_share)
}

/// Loads each checkpoint as it is saved and ends the process at the first the engine's loader
/// refuses, since every step after it trains a net that cannot ship.
fn watch(out: &Path, id: &str, stop: &AtomicBool) -> BTreeSet<PathBuf> {
    let mut seen = BTreeSet::new();
    while !stop.load(Ordering::Relaxed) {
        if let Err(e) = cadence_train::checkpoint::check_saved(out, id, &mut seen) {
            eprintln!("cadence-train: {e}");
            std::process::exit(1);
        }
        std::thread::sleep(WATCH_EVERY);
    }
    seen
}

/// Each checkpoint's validation loss over the holdout, from the trainer's own forward pass, beside
/// the training loss it logged for the same superbatch; a reading, not a gate.
fn validate(flags: &Flags) -> Result<(), String> {
    let checkpoints: Vec<PathBuf> = flags
        .get("--checkpoint")
        .map(|c| c.iter().map(PathBuf::from).collect())
        .ok_or_else(|| format!("validate needs --checkpoint\n{USAGE}"))?;
    let manifests: Vec<&Path> = flags
        .get("--manifest")
        .map(|m| m.iter().map(Path::new).collect())
        .ok_or_else(|| format!("validate needs --manifest, the holdout's manifest\n{USAGE}"))?;
    let holdout = Path::new(one(flags, "--holdout")?);
    cadence_train::data::verify_holdout(holdout, &manifests)?;
    let positions = parsed(flags, "--validation-positions", VALIDATION_POSITIONS)?;
    let result_share = parsed(flags, "--result-share", RESULT_SHARE)?;
    validation(&checkpoints, holdout, positions, result_share)
}

/// The holdout's first `positions` records, scored by each checkpoint in a trainer of its own,
/// since the trainer keeps the first weights it evaluates with.
fn validation(
    checkpoints: &[PathBuf],
    holdout: &Path,
    positions: usize,
    result_share: f32,
) -> Result<(), String> {
    let bytes = std::fs::read(holdout).map_err(|e| format!("{}: {e}", holdout.display()))?;
    let examples = bytes
        .as_chunks::<{ cadence_dataset::trainer::RECORD_LEN }>()
        .0
        .iter()
        .take(positions)
        .map(cadence_train::validate::example)
        .collect::<Result<Vec<_>, String>>()
        .map_err(|e| format!("{}: {e}", holdout.display()))?;
    if examples.is_empty() {
        return Err(format!("{} holds no record", holdout.display()));
    }
    if let Some(missing) = checkpoints
        .iter()
        .find(|c| !c.join("optimiser_state").is_dir())
    {
        return Err(format!(
            "{} is not a checkpoint the trainer saved",
            missing.display()
        ));
    }
    let scale = f32::from(i16::try_from(SCALE).unwrap_or(i16::MAX));
    let n = u32::try_from(examples.len()).unwrap_or(u32::MAX);
    for checkpoint in checkpoints {
        let mut trainer = recipe!();
        trainer.load_from_checkpoint(&checkpoint.to_string_lossy());
        let loss: f64 = examples
            .iter()
            .map(|e| cadence_train::validate::loss(trainer.eval(&e.fen), e, result_share, scale))
            .sum::<f64>()
            / f64::from(n);
        let superbatch = checkpoint
            .file_name()
            .and_then(|name| name.to_string_lossy().rsplit('-').next()?.parse().ok());
        let log = std::fs::read_to_string(checkpoint.join("log.txt")).unwrap_or_default();
        let training = superbatch
            .and_then(|sb| cadence_train::validate::training_loss(&log, sb))
            .map_or_else(|| "not logged".to_string(), |t| format!("{t:.6}"));
        println!(
            "validation: {}: training loss {training}, validation loss {loss:.6} over {n} holdout \
             positions",
            checkpoint.display()
        );
    }
    Ok(())
}

fn export(flags: &Flags) -> Result<(), String> {
    let checkpoint = PathBuf::from(one(flags, "--checkpoint")?);
    let manifest_path = one(flags, "--manifest")?;
    let provenance = match one(flags, "--provenance")? {
        "self-play" => Provenance::SelfPlay,
        "canary" => Provenance::Canary,
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
    let run = checkpoint.parent().unwrap_or(Path::new("."));
    let commit_file = run.join(COMMIT_FILE);
    let trainer = String::from_utf8_lossy(&read(&commit_file)?)
        .trim()
        .to_string();
    let manifest = read(Path::new(manifest_path))?;
    cadence_train::data::verify_export(run, &manifest)?;
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
    let every = parsed(flags, "--every", BOOK_EVERY)?;
    let manifest_path = PathBuf::from(format!("{}.manifest", out.display()));
    let holdout_path = PathBuf::from(format!("{}.holdout", out.display()));
    for path in [&out, &holdout_path, &manifest_path] {
        if path.exists() {
            return Err(format!(
                "{} exists, and a conversion never replaces one",
                path.display()
            ));
        }
    }
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let text = String::from_utf8(bytes.clone()).map_err(|e| format!("{input}: not UTF-8: {e}"))?;
    let (split, read) = cadence_train::book::convert(&text, seed, every)?;
    let flat: Vec<u8> = split.train.iter().flatten().copied().collect();
    let held: Vec<u8> = split.holdout.iter().flatten().copied().collect();
    std::fs::write(&out, &flat).map_err(|e| format!("{}: {e}", out.display()))?;
    std::fs::write(&holdout_path, &held).map_err(|e| format!("{}: {e}", holdout_path.display()))?;
    let name = Path::new(input)
        .file_name()
        .map_or_else(|| input.into(), |n| n.to_string_lossy());
    let manifest = format!(
        "{BOOK_HEADER}\nsource\t{name}\nsource-sha256\t{}\nsource-bytes\t{}\npositions\t{}\n\
         white-to-move\t{}\nresults-loss-draw-win\t{} {} {}\nscore\tnone, written as 0\n\
         shuffle-seed\t{seed:016x}\nholdout-every\t{every}\nrecords\t{}\nrecords-sha256\t{}\n\
         holdout-records\t{}\nholdout-sha256\t{}\nconverter-commit\t{COMMIT}\n\
         converter-modified\t{DIRTY}\n",
        hex(&sha256(&bytes)),
        bytes.len(),
        read.positions,
        read.white_to_move,
        read.results[0],
        read.results[1],
        read.results[2],
        split.train.len(),
        hex(&sha256(&flat)),
        split.holdout.len(),
        hex(&sha256(&held)),
    );
    std::fs::write(&manifest_path, manifest)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    println!(
        "{}: {} positions, {} held out in {}, {} with White to move, mover's results {:?}; \
         manifest {}",
        out.display(),
        read.positions,
        split.holdout.len(),
        holdout_path.display(),
        read.white_to_move,
        read.results,
        manifest_path.display()
    );
    Ok(())
}

/// The trainer's own output for each position, read back from a checkpoint the recipe saved, in
/// centipawns: the reading any other forward pass over the same weights is held to.
fn eval(flags: &Flags) -> Result<(), String> {
    let checkpoint = one(flags, "--checkpoint")?;
    let fens = one(flags, "--fens")?;
    let text = std::fs::read_to_string(fens).map_err(|e| format!("{fens}: {e}"))?;
    let state = Path::new(checkpoint).join("optimiser_state");
    if !state.is_dir() {
        return Err(format!(
            "{} is not a checkpoint the trainer saved",
            state.display()
        ));
    }
    let positions: Vec<&str> = text
        .lines()
        .map(|line| line.split('[').next().unwrap_or(line).trim())
        .filter(|fen| !fen.is_empty())
        .collect();
    let mut trainer = recipe!();
    trainer.load_from_checkpoint(checkpoint);
    let scale = f32::from(i16::try_from(SCALE).unwrap_or(i16::MAX));
    for fen in positions {
        println!("{:.4}\t{fen}", trainer.eval(fen) * scale);
    }
    Ok(())
}
