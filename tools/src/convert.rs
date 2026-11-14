// SPDX-License-Identifier: GPL-3.0-or-later

//! Datasets of packed chunks to the trainer's position records: each chunk held to its manifest
//! first, then filtered, split by game and shuffled from a recorded seed. The outputs are published
//! as one unit, a directory whose manifest is written last.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use cadence_core::chess960::dfrc_fen;
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::Rng;
use cadence_dataset::chunk::{Chunk, GameRecord, NO_SCORE, decode_move, game_key};
use cadence_dataset::constants::{FIRST_PLY, LAST_PLY, MATE_BOUND};
use cadence_dataset::sha256::{hex, sha256};
use cadence_dataset::trainer::{self, DRAW, LOSS, RECORD_LEN, WIN};

const USAGE: &str = "usage: cadence-tools convert --manifest FILE [--manifest FILE]... --out DIR \
     --shuffle-seed HEX|fresh [--every 10] [--standard-share P] [--score-cap N] [--swing-cap N] \
     [--dedup]";

/// The last band holds every ply past it, as `datagen extract`'s does.
const PLY_BAND: usize = 10;
const PLY_BANDS: usize = 30;

/// 100 centipawns a bucket over +-2,000, and a tail at each end.
const BUCKET: i32 = 100;
const SPAN: i32 = 2_000;
const BUCKETS: usize = (2 * SPAN / BUCKET) as usize + 2;

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
/// A command line that does not parse, or a conversion that fails; nothing is published then.
pub fn run(args: &[String]) -> Result<ExitCode, String> {
    let request = request_from(args).map_err(|e| format!("{e}\n{USAGE}"))?;
    let converted = convert(&request, &mut |from, to| fs::rename(from, to));
    let mut stdout = io::stdout().lock();
    let unexamined = [
        "the trainer's own reading of the records (the train crate's round trip)",
        "each game's ending and its regeneration (datagen verify's)",
    ];
    let unit = match converted {
        Ok(unit) => unit,
        Err(e) => {
            for spot in unexamined {
                let _ = writeln!(stdout, "  not examined: {spot}");
            }
            return Err(e);
        }
    };
    let report = fs::read_to_string(unit.join("report.txt")).unwrap_or_default();
    let _ = write!(stdout, "{report}");
    for spot in unexamined {
        let _ = writeln!(stdout, "  not examined: {spot}");
    }
    let _ = writeln!(stdout, "published {}", unit.display());
    Ok(ExitCode::SUCCESS)
}

fn request_from(args: &[String]) -> Result<Request, String> {
    let mut request = Request {
        manifests: Vec::new(),
        out: PathBuf::new(),
        every: 10,
        shuffle_seed: 0,
        standard_share: None,
        score_cap: None,
        swing_cap: None,
        dedup: false,
    };
    let (mut out, mut seed) = (None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--dedup" {
            request.dedup = true;
            continue;
        }
        let value = it.next().ok_or_else(|| format!("{arg} takes a value"))?;
        let bad = || format!("bad {arg} `{value}`");
        match arg.as_str() {
            "--manifest" => request.manifests.push(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            "--every" => request.every = value.parse().map_err(|_| bad())?,
            "--shuffle-seed" => seed = Some(parse_seed(value).ok_or_else(bad)?),
            "--standard-share" => {
                let share: u8 = value.parse().map_err(|_| bad())?;
                if share > 100 {
                    return Err(bad());
                }
                request.standard_share = Some(share);
            }
            "--score-cap" => request.score_cap = Some(value.parse().map_err(|_| bad())?),
            "--swing-cap" => request.swing_cap = Some(value.parse().map_err(|_| bad())?),
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if request.manifests.is_empty() {
        return Err("--manifest is required".to_string());
    }
    request.out = out.ok_or("--out is required")?;
    request.shuffle_seed =
        seed.ok_or("--shuffle-seed is required: a hex seed, or fresh to draw one")?;
    Ok(request)
}

/// Hex, or `fresh` for one drawn from the per-process hasher keys, the time and the process.
fn parse_seed(value: &str) -> Option<u64> {
    if value == "fresh" {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let keys = std::hash::RandomState::new();
        return Some(std::hash::BuildHasher::hash_one(
            &keys,
            (nanos, std::process::id()),
        ));
    }
    u64::from_str_radix(value, 16).ok()
}

/// One dataset's manifest: its settings, and each chunk with its SHA-256.
struct Dataset {
    manifest: PathBuf,
    dir: PathBuf,
    sha256: String,
    chunks: Vec<(String, String)>,
}

fn read_manifest(path: &Path) -> Result<Dataset, String> {
    let text = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let digest = hex(&sha256(&text));
    let text = String::from_utf8(text).map_err(|_| format!("{}: not text", path.display()))?;
    let mut chunks = Vec::new();
    let mut rows = false;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        if line.starts_with("chunk\t") {
            rows = true;
        } else if rows {
            let mut fields = line.split('\t');
            let (Some(name), Some(sha)) = (fields.next(), fields.next()) else {
                return Err(format!(
                    "{}: a chunk row `{line}` does not parse",
                    path.display()
                ));
            };
            chunks.push((name.to_string(), sha.to_string()));
        }
    }
    if chunks.is_empty() {
        return Err(format!("{}: lists no chunk", path.display()));
    }
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    };
    Ok(Dataset {
        manifest: path.to_path_buf(),
        dir,
        sha256: digest,
        chunks,
    })
}

/// Before any record is read: every chunk in the directory listed, every listed chunk present, and
/// each one's SHA-256 the manifest's and its sidecar's.
fn check_listing(set: &Dataset) -> Result<(), String> {
    let listed: BTreeMap<&str, &str> = set
        .chunks
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    for entry in fs::read_dir(&set.dir).map_err(|e| format!("{}: {e}", set.dir.display()))? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        let name = name.to_string_lossy();
        if Path::new(&*name).extension().is_some_and(|e| e == "cdg") && !listed.contains_key(&*name)
        {
            return Err(format!(
                "{name} is in {} and not in its manifest",
                set.dir.display()
            ));
        }
    }
    for (name, sha) in &set.chunks {
        let path = set.dir.join(name);
        let bytes = fs::read(&path).map_err(|_| format!("{name} is listed and missing"))?;
        let digest = hex(&sha256(&bytes));
        if &digest != sha {
            return Err(format!(
                "{name}: SHA-256 {digest}, and the manifest says {sha}"
            ));
        }
        let side = set.dir.join(format!("{name}.sha256"));
        let text = fs::read_to_string(&side).map_err(|_| format!("{name}.sha256 is missing"))?;
        let said = text.split_whitespace().next().unwrap_or("");
        if said != digest {
            return Err(format!(
                "{name}: SHA-256 {digest}, and its sidecar says {said}"
            ));
        }
    }
    Ok(())
}

/// A game and where it came from.
struct Game {
    dataset: usize,
    chunk: String,
    record: GameRecord,
}

/// Counts for one dataset and one start kind.
#[derive(Clone, Debug, Default)]
struct Counts {
    games: u64,
    visited: u64,
    outside: u64,
    in_check: u64,
    noisy: u64,
    no_score: u64,
    mate: u64,
    capped: u64,
    swung: u64,
    repeated_dropped: u64,
    train: u64,
    holdout: u64,
    within_game: u64,
    across_games: u64,
    plies: Vec<u64>,
    zeros: Vec<u64>,
    unscored: Vec<u64>,
    scores: Vec<u64>,
}

impl Counts {
    fn new() -> Counts {
        Counts {
            plies: vec![0; PLY_BANDS],
            zeros: vec![0; PLY_BANDS],
            unscored: vec![0; PLY_BANDS],
            scores: vec![0; BUCKETS],
            ..Counts::default()
        }
    }
}

fn bucket(score: i16) -> usize {
    let s = i32::from(score);
    if s < -SPAN {
        0
    } else if s >= SPAN {
        BUCKETS - 1
    } else {
        usize::try_from((s + SPAN) / BUCKET).unwrap_or(0) + 1
    }
}

/// The positions kept, each with its record and its key, split by game.
#[derive(Default)]
struct Kept {
    train: Vec<[u8; RECORD_LEN]>,
    holdout: Vec<[u8; RECORD_LEN]>,
    train_keys: BTreeSet<u64>,
    holdout_keys: Vec<u64>,
}

/// The whole conversion up to its published unit's path; `rename` is the unit's last step, so a
/// test can make it fail.
///
/// # Errors
///
/// Any refusal before a record is read, a duplicate game or chunk, a record that does not replay,
/// or a file that cannot be written; nothing is published then.
pub fn convert(
    request: &Request,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, String> {
    let sets = request
        .manifests
        .iter()
        .map(|m| read_manifest(m))
        .collect::<Result<Vec<_>, _>>()?;
    for set in &sets {
        check_listing(set)?;
    }
    let games = read_games(&sets)?;
    let chosen = subsample(&games, request.standard_share);
    let mut counts = vec![[Counts::new(), Counts::new()]; sets.len()];
    let mut kept = Kept::default();
    let mut seen = BTreeSet::new();
    for game in chosen {
        positions(
            request,
            game,
            &mut counts[game.dataset][usize::from(game.record.kind)],
            &mut kept,
            &mut seen,
        )
        .map_err(|e| format!("{} game {}: {e}", game.chunk, game.record.index))?;
    }
    let overlap = kept
        .holdout_keys
        .iter()
        .filter(|k| kept.train_keys.contains(k))
        .count();
    let mut rng = Rng::new(request.shuffle_seed);
    shuffle(&mut kept.train, &mut rng);
    shuffle(&mut kept.holdout, &mut rng);
    let report = report(&sets, &counts, overlap);
    publish(request, &sets, &counts, &kept, &report, rename)
}

/// Every chunk read and held together: no run seed and chunk index twice, and no game twice.
fn read_games(sets: &[Dataset]) -> Result<Vec<Game>, String> {
    let mut chunks: BTreeMap<(u64, u32), String> = BTreeMap::new();
    let mut keys: BTreeMap<[u8; 16], (String, u32)> = BTreeMap::new();
    let mut games = Vec::new();
    for (dataset, set) in sets.iter().enumerate() {
        for (name, _) in &set.chunks {
            let bytes = fs::read(set.dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
            let chunk = Chunk::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
            let at = (chunk.header.run_seed, chunk.header.chunk_index);
            if let Some(first) = chunks.insert(at, name.clone()) {
                return Err(format!(
                    "{first} and {name} carry run seed {:016x} and chunk index {}",
                    at.0, at.1
                ));
            }
            for record in chunk.records {
                let key = game_key(&record);
                if let Some((first, index)) = keys.get(&key) {
                    return Err(format!(
                        "{name} game {} is the same game as {first} game {index}",
                        record.index
                    ));
                }
                keys.insert(key, (name.clone(), record.index));
                games.push(Game {
                    dataset,
                    chunk: name.clone(),
                    record,
                });
            }
        }
    }
    Ok(games)
}

/// Games of the more common kind dropped from the end until the standard share is the one asked,
/// or every game kept when none is asked.
fn subsample(games: &[Game], share: Option<u8>) -> Vec<&Game> {
    let Some(share) = share else {
        return games.iter().collect();
    };
    let standard = games.iter().filter(|g| g.record.kind == 1).count() as u64;
    let uniform = games.len() as u64 - standard;
    let (p, q) = (u64::from(share), 100 - u64::from(share));
    // Keep (u', s') with s' * q == u' * p, each at most what there is.
    let (keep_uniform, keep_standard) = if standard * q > uniform * p {
        (uniform, (uniform * p).checked_div(q).unwrap_or(standard))
    } else {
        ((standard * q).checked_div(p).unwrap_or(uniform), standard)
    };
    let (mut u, mut s) = (0, 0);
    games
        .iter()
        .filter(|g| {
            if g.record.kind == 1 {
                s += 1;
                s <= keep_standard
            } else {
                u += 1;
                u <= keep_uniform
            }
        })
        .collect()
}

/// The positions of one game's moves that pass every filter, each before its move is played.
fn positions(
    request: &Request,
    game: &Game,
    counts: &mut Counts,
    kept: &mut Kept,
    seen: &mut BTreeSet<u64>,
) -> Result<(), String> {
    let record = &game.record;
    let fen = dfrc_fen(u32::from(record.white), u32::from(record.black));
    let mut board = Board::from_fen(&fen).map_err(|e| format!("{fen}: {e:?}"))?;
    for &bits in &record.plies {
        let m =
            decode_move(&generate_legal(&board), bits).ok_or("a random ply that is not legal")?;
        board.play(m);
    }
    counts.games += 1;
    let held = request.every > 0 && record.index.is_multiple_of(request.every);
    let mut decided = false;
    let mut this_game = BTreeSet::new();
    for (ply, &(bits, score)) in record.moves.iter().enumerate() {
        let m = decode_move(&generate_legal(&board), bits).ok_or("a move that is not legal")?;
        counts.visited += 1;
        let band = (ply / PLY_BAND).min(PLY_BANDS - 1);
        if !(FIRST_PLY..=LAST_PLY).contains(&ply) {
            counts.outside += 1;
        } else if board.in_check() {
            counts.in_check += 1;
        } else if m.is_noisy() {
            counts.noisy += 1;
        } else if score == NO_SCORE {
            counts.no_score += 1;
            counts.unscored[band] += 1;
        } else if score.unsigned_abs() >= MATE_BOUND.unsigned_abs() {
            counts.mate += 1;
        } else if request
            .score_cap
            .is_some_and(|cap| score.unsigned_abs() > cap.unsigned_abs())
        {
            counts.capped += 1;
        } else if decided {
            counts.swung += 1;
        } else {
            let key = board.key();
            let again = !this_game.insert(key);
            let before = seen.contains(&key);
            counts.within_game += u64::from(again);
            counts.across_games += u64::from(!again && before);
            if request.dedup && (again || before) {
                counts.repeated_dropped += 1;
            } else {
                seen.insert(key);
                let result = result_for_mover(
                    record.outcome,
                    board.side_to_move() == cadence_core::Colour::White,
                );
                let bytes = trainer::encode(&board, score, result);
                counts.plies[band] += 1;
                counts.zeros[band] += u64::from(score == 0);
                counts.scores[bucket(score)] += 1;
                if held {
                    kept.holdout.push(bytes);
                    kept.holdout_keys.push(key);
                    counts.holdout += 1;
                } else {
                    kept.train.push(bytes);
                    kept.train_keys.insert(key);
                    counts.train += 1;
                }
            }
        }
        // A score past the swing cap decides the game, and the positions after it are dropped.
        decided |= request
            .swing_cap
            .is_some_and(|cap| score != NO_SCORE && score.unsigned_abs() >= cap.unsigned_abs());
        board.play(m);
    }
    Ok(())
}

/// The record stores White's result; the trainer reads the side to move's.
fn result_for_mover(outcome: u8, white_to_move: bool) -> u8 {
    match (outcome, white_to_move) {
        (2, _) => DRAW,
        (0, true) | (1, false) => WIN,
        _ => LOSS,
    }
}

/// Fisher-Yates from the recorded seed, so the same seed gives the same bytes.
fn shuffle(records: &mut [[u8; RECORD_LEN]], rng: &mut Rng) {
    for i in (1..records.len()).rev() {
        let j = rng.below(i + 1);
        records.swap(i, j);
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a position count is far below 2^52"
)]
fn report(sets: &[Dataset], counts: &[[Counts; 2]], overlap: usize) -> String {
    let mut text = String::new();
    for (set, kinds) in sets.iter().zip(counts) {
        let _ = writeln!(text, "dataset {}", set.manifest.display());
        for (name, c) in ["uniform", "standard"].iter().zip(kinds) {
            let _ = writeln!(
                text,
                "  {name} games {} visited {} kept {} train {} holdout {}",
                c.games,
                c.visited,
                c.train + c.holdout,
                c.train,
                c.holdout
            );
            let _ = writeln!(
                text,
                "  {name} outside-plies-{FIRST_PLY}-{LAST_PLY} {} in-check {} noisy {} no-score {} mate {} \
                 score-cap {} swing-cap {} repeated-dropped {}",
                c.outside,
                c.in_check,
                c.noisy,
                c.no_score,
                c.mate,
                c.capped,
                c.swung,
                c.repeated_dropped
            );
            let _ = writeln!(
                text,
                "  {name} repeated positions within-game {} across-games {}",
                c.within_game, c.across_games
            );
            for (label, bands) in [
                ("kept", &c.plies),
                ("exact-zero", &c.zeros),
                ("no-score", &c.unscored),
            ] {
                let row: Vec<String> = bands.iter().map(u64::to_string).collect();
                let _ = writeln!(
                    text,
                    "  {name} {label} by ply band of {PLY_BAND}: {}",
                    row.join(" ")
                );
            }
            let total: u64 = c.scores.iter().sum();
            let row: Vec<String> = c
                .scores
                .iter()
                .map(|&n| format!("{n} ({:.1}%)", 100.0 * n as f64 / total.max(1) as f64))
                .collect();
            let _ = writeln!(
                text,
                "  {name} score by {BUCKET} from -{SPAN} to {SPAN}, tails at each end: {}",
                row.join(" ")
            );
        }
    }
    let _ = writeln!(
        text,
        "holdout positions whose key is also in training: {overlap}"
    );
    text
}

/// Staged in a directory of its own, each file synced and the manifest last, then renamed whole to
/// its version id; a failure before the rename publishes nothing.
fn publish(
    request: &Request,
    sets: &[Dataset],
    counts: &[[Counts; 2]],
    kept: &Kept,
    report: &str,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<PathBuf, String> {
    fs::create_dir_all(&request.out).map_err(|e| format!("{}: {e}", request.out.display()))?;
    let staging = request.out.join(format!(".staging-{}", std::process::id()));
    fs::create_dir(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let staged = (|| {
        let train: Vec<u8> = kept.train.concat();
        let holdout: Vec<u8> = kept.holdout.concat();
        let (train_sha, holdout_sha) = (sha256(&train), sha256(&holdout));
        write_synced(&staging.join("train.bin"), &train)?;
        write_synced(&staging.join("holdout.bin"), &holdout)?;
        write_synced(&staging.join("report.txt"), report.as_bytes())?;
        let manifest = manifest(
            request,
            sets,
            counts,
            kept,
            &train_sha,
            &holdout_sha,
            report,
        );
        write_synced(&staging.join("MANIFEST.tsv"), manifest.as_bytes())?;
        let id = hex(&sha256(&[train_sha, holdout_sha].concat()))[..16].to_string();
        let unit = request.out.join(&id);
        if unit.exists() {
            return Err(format!("{} already holds this unit", unit.display()));
        }
        rename(&staging, &unit).map_err(|e| format!("{}: {e}", unit.display()))?;
        Ok(unit)
    })();
    if staged.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    staged
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let failed = |e: io::Error| format!("{}: {e}", path.display());
    let mut file = File::create(path).map_err(failed)?;
    file.write_all(bytes).map_err(failed)?;
    file.sync_all().map_err(failed)
}

/// The request, the sources, the filters, the counts and each output's SHA-256.
fn manifest(
    request: &Request,
    sets: &[Dataset],
    counts: &[[Counts; 2]],
    kept: &Kept,
    train_sha: &[u8; 32],
    holdout_sha: &[u8; 32],
    report: &str,
) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "# cadence conversion manifest, written by cadence-tools convert"
    );
    let _ = writeln!(
        text,
        "converter-commit\t{}",
        env!("CADENCE_TOOLS_COMMIT_FULL")
    );
    let _ = writeln!(text, "converter-modified\t{}", env!("CADENCE_TOOLS_DIRTY"));
    let _ = writeln!(text, "shuffle-seed\t{:016x}", request.shuffle_seed);
    let _ = writeln!(
        text,
        "holdout\tgame index modulo {} within its chunk",
        request.every
    );
    let share = request
        .standard_share
        .map_or_else(|| "as generated".to_string(), |s| s.to_string());
    let _ = writeln!(text, "standard-share\t{share}");
    let mut filters =
        format!("plies {FIRST_PLY}-{LAST_PLY}, not in check, not noisy, not no-score, not mate");
    if let Some(cap) = request.score_cap {
        let _ = write!(filters, ", score cap {cap}");
    }
    if let Some(cap) = request.swing_cap {
        let _ = write!(filters, ", swing cap {cap}");
    }
    if request.dedup {
        filters.push_str(", repeated positions dropped");
    }
    let _ = writeln!(text, "filters\t{filters}");
    for (set, kinds) in sets.iter().zip(counts) {
        let _ = writeln!(text, "source\t{}\t{}", set.manifest.display(), set.sha256);
        for (name, c) in ["uniform", "standard"].iter().zip(kinds) {
            let _ = writeln!(
                text,
                "kept\t{}\t{name}\t{}\t{}",
                set.manifest.display(),
                c.train,
                c.holdout
            );
        }
    }
    let _ = writeln!(
        text,
        "file\ttrain.bin\t{}\t{}",
        hex(train_sha),
        kept.train.len()
    );
    let _ = writeln!(
        text,
        "file\tholdout.bin\t{}\t{}",
        hex(holdout_sha),
        kept.holdout.len()
    );
    let _ = writeln!(
        text,
        "file\treport.txt\t{}",
        hex(&sha256(report.as_bytes()))
    );
    text
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
