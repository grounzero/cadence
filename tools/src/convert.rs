// SPDX-License-Identifier: GPL-3.0-or-later

//! Datasets of packed chunks to the trainer's position records: each chunk held to its manifest
//! first, then filtered, split by game and shuffled from a recorded seed. The outputs are published
//! as one unit, a directory whose manifest is written last.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use cadence_core::chess960::dfrc_fen;
use cadence_core::generate_legal;
use cadence_core::position::Board;
use cadence_core::rng::{Rng, splitmix64};
use cadence_dataset::chunk::{Chunk, GameRecord, Header, NO_SCORE, decode_move, game_key};
use cadence_dataset::constants::{FIRST_PLY, LAST_PLY, MATE_BOUND};
use cadence_dataset::durable::{self, Disk, Steps, UNSYNCED};
use cadence_dataset::sha256::{Sha256, hex, sha256};
use cadence_dataset::trainer::{self, DRAW, LOSS, RECORD_LEN, WIN};

const USAGE: &str = "usage: cadence-tools convert --manifest FILE [--manifest FILE]... --out DIR \
     --shuffle-seed HEX|fresh [--every 10] [--standard-share P] [--score-cap N] [--swing-cap N] \
     [--dedup] [--max-copies N] [--report-sample N]";

/// The last band holds every ply past it, as `datagen extract`'s does.
const PLY_BAND: usize = 10;
const PLY_BANDS: usize = 30;

/// 100 centipawns a bucket over +-2,000, and a tail at each end.
const BUCKET: i32 = 100;
const SPAN: i32 = 2_000;
const BUCKETS: usize = (2 * SPAN / BUCKET) as usize + 2;

/// A kind's games may repeat an earlier game from another seed up to one in this many, where chance
/// gives standard starts about one in sixty thousand at two million games, rising with the count.
/// Past it the seeds are suspect and the conversion refuses.
const REPEAT_BAR: u64 = 1_000;

/// The unit published, and the line reporting it when the sync after its rename failed.
#[derive(Debug)]
pub struct Converted {
    pub unit: PathBuf,
    pub unconfirmed: Option<String>,
}

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
    /// At most this many training copies of a network input, the first read kept.
    pub max_copies: Option<u8>,
    /// The report's figures by key and by input read from 1 key in this many, and scaled by it.
    pub report_sample: u32,
}

/// # Errors
///
/// A command line that does not parse, or a conversion that fails; nothing is published then.
pub fn run(args: &[String]) -> Result<ExitCode, String> {
    let request = request_from(args).map_err(|e| format!("{e}\n{USAGE}"))?;
    let converted = convert(&request, &mut Disk);
    let mut stdout = io::stdout().lock();
    let mut unexamined = vec![
        "the trainer's own reading of the records (the train crate's round trip)",
        "each game's ending and its regeneration (datagen verify's)",
        "each manifest row's games, plies and kinds (a chunk is held to its SHA-256 and its header)",
    ];
    unexamined.extend(UNSYNCED);
    let converted = match converted {
        Ok(converted) => converted,
        Err(e) => {
            for spot in &unexamined {
                let _ = writeln!(stdout, "  not examined: {spot}");
            }
            return Err(e);
        }
    };
    let unit = &converted.unit;
    let report = fs::read_to_string(unit.join("report.txt")).unwrap_or_default();
    let _ = write!(stdout, "{report}");
    for spot in unexamined {
        let _ = writeln!(stdout, "  not examined: {spot}");
    }
    let published = converted
        .unconfirmed
        .clone()
        .unwrap_or_else(|| format!("published {}", unit.display()));
    let _ = writeln!(stdout, "{published}");
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
        max_copies: None,
        report_sample: 1,
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
            "--every" => {
                request.every = value.parse().map_err(|_| bad())?;
                if request.every < 2 {
                    return Err(format!(
                        "--every {value} is refused: 0 holds out nothing and 1 trains on nothing"
                    ));
                }
            }
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
            "--max-copies" => match value.parse::<u8>() {
                Ok(0) => return Err("--max-copies 0 is refused: it trains on nothing".to_string()),
                Ok(cap) => request.max_copies = Some(cap),
                Err(_) => return Err(format!("bad {arg} `{value}`: 1 to 255")),
            },
            "--report-sample" => match value.parse::<u32>() {
                Ok(n) if n >= 1 => request.report_sample = n,
                _ => return Err(format!("bad {arg} `{value}`: 1 reads every key")),
            },
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

/// One dataset's manifest: its provenance, and each chunk with its SHA-256.
struct Dataset {
    manifest: PathBuf,
    dir: PathBuf,
    sha256: String,
    provenance: BTreeMap<String, String>,
    chunks: Vec<(String, String)>,
}

fn read_manifest(path: &Path) -> Result<Dataset, String> {
    let text = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let digest = hex(&sha256(&text));
    let text = String::from_utf8(text).map_err(|_| format!("{}: not text", path.display()))?;
    let (mut chunks, mut provenance) = (Vec::new(), BTreeMap::new());
    let mut rows = false;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        if line.starts_with("chunk\t") {
            rows = true;
        } else if !rows {
            if let Some((field, value)) = line.split_once('\t') {
                provenance.insert(field.to_string(), value.to_string());
            }
        } else {
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
        provenance,
        chunks,
    })
}

/// A chunk's header against its manifest's provenance, so a manifest edited with every hash left
/// matching is still refused.
fn check_provenance(set: &Dataset, name: &str, h: &Header) -> Result<(), String> {
    let fields = [
        ("commit", hex(&h.commit)),
        ("version", h.version.clone()),
        ("nodes", h.nodes.to_string()),
        ("screen-nodes", h.screen_nodes.to_string()),
        ("hash", h.hash_mb.to_string()),
        ("random-plies", h.random_plies.to_string()),
        ("window", h.window.to_string()),
        ("standard-share", h.standard_share.to_string()),
        ("chunk-games", h.games_planned.to_string()),
        ("run-seeds", format!("{:016x}", h.run_seed)),
    ];
    for (field, have) in fields {
        let want = set
            .provenance
            .get(field)
            .ok_or_else(|| format!("{}: names no {field}", set.manifest.display()))?;
        let agrees = if field == "run-seeds" {
            want.split_whitespace().any(|seed| seed == have)
        } else {
            *want == have
        };
        if !agrees {
            return Err(format!(
                "{name}: {field} {have} in its header, and the manifest says {want}"
            ));
        }
    }
    Ok(())
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
    /// Its place in its run, which the holdout is taken by.
    number: u64,
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
    past_copies: u64,
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

/// The positions kept, split by game: their records dealt to buckets on disk, their keys and
/// network inputs held in memory for the report.
struct Kept {
    train: Buckets,
    holdout: Buckets,
    train_keys: Vec<u64>,
    holdout_keys: Vec<u64>,
    /// By start kind and network input, a reading only: each sorted once every position is kept.
    train_inputs: [Vec<u64>; 2],
    holdout_inputs: [Vec<u64>; 2],
    copies: CopyCounts,
}

impl Kept {
    fn new(dir: &Path, seed: u64, copies: CopyCounts) -> Result<Kept, String> {
        Ok(Kept {
            train: Buckets::new(&dir.join("train"), TRAIN_BUCKETS, splitmix64(seed).0)?,
            holdout: Buckets::new(&dir.join("holdout"), HOLDOUT_BUCKETS, splitmix64(!seed).0)?,
            train_keys: Vec::new(),
            holdout_keys: Vec::new(),
            train_inputs: [Vec::new(), Vec::new()],
            holdout_inputs: [Vec::new(), Vec::new()],
            copies,
        })
    }
}

/// Files each split's records are dealt to as they are kept, each read whole later: about 0.5 GB
/// a training bucket at two billion positions, and 144 files open, inside a default limit of 256.
const TRAIN_BUCKETS: usize = 128;
const HOLDOUT_BUCKETS: usize = 16;

/// A two-pass shuffle: each record dealt to a bucket by a seeded draw, then each bucket shuffled
/// whole and appended in turn, a uniform permutation when the deal and each shuffle are.
struct Buckets {
    dir: PathBuf,
    files: Vec<BufWriter<File>>,
    counts: Vec<u64>,
    deal: Rng,
}

impl Buckets {
    fn new(dir: &Path, n: usize, seed: u64) -> Result<Buckets, String> {
        let failed = |e: io::Error| format!("{}: {e}", dir.display());
        fs::create_dir_all(dir).map_err(failed)?;
        let files = (0..n)
            .map(|i| File::create(dir.join(format!("{i:04}"))).map(BufWriter::new))
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)?;
        Ok(Buckets {
            dir: dir.to_path_buf(),
            files,
            counts: vec![0; n],
            deal: Rng::new(seed),
        })
    }

    fn push(&mut self, record: &[u8; RECORD_LEN]) -> Result<(), String> {
        let b = self.deal.below(self.files.len());
        self.files[b]
            .write_all(record)
            .map_err(|e| format!("{}: {e}", self.dir.display()))?;
        self.counts[b] += 1;
        Ok(())
    }

    fn len(&self) -> u64 {
        self.counts.iter().sum()
    }

    /// Each bucket shuffled with `rng` and appended to `out` in turn, then removed; the file is
    /// synced and its SHA-256 returned.
    fn drain(self, out: &Path, rng: &mut Rng) -> Result<[u8; 32], String> {
        let failed = |e: io::Error| format!("{}: {e}", out.display());
        let n = self.files.len();
        for file in self.files {
            file.into_inner()
                .map_err(|e| format!("{}: {}", self.dir.display(), e.error()))?;
        }
        let mut writer = BufWriter::new(File::create(out).map_err(failed)?);
        let mut hash = Sha256::new();
        for i in 0..n {
            let path = self.dir.join(format!("{i:04}"));
            let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mut records = bytes.as_chunks::<RECORD_LEN>().0.to_vec();
            drop(bytes);
            shuffle(&mut records, rng);
            for record in &records {
                hash.update(record);
                writer.write_all(record).map_err(failed)?;
            }
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        let file = writer.into_inner().map_err(|e| failed(e.into_error()))?;
        file.sync_all().map_err(failed)?;
        fs::remove_dir(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        Ok(hash.finish())
    }
}

/// Position keys by open addressing, at most about 19 bytes a key where a tree set holds several
/// times that; zero, an empty slot's mark, is held apart.
struct KeySet {
    slots: Vec<u64>,
    len: usize,
    zero: bool,
}

impl KeySet {
    fn new() -> KeySet {
        KeySet {
            slots: vec![0; 1 << 16],
            len: 0,
            zero: false,
        }
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "masked below the slot count, a usize"
    )]
    fn slot(slots: &[u64], key: u64) -> usize {
        let mask = slots.len() - 1;
        let mut i = (splitmix64(key).0 & mask as u64) as usize;
        while slots[i] != 0 && slots[i] != key {
            i = (i + 1) & mask;
        }
        i
    }

    fn contains(&self, key: u64) -> bool {
        if key == 0 {
            return self.zero;
        }
        self.slots[KeySet::slot(&self.slots, key)] == key
    }

    /// Whether the key was new.
    fn insert(&mut self, key: u64) -> bool {
        if key == 0 {
            return !std::mem::replace(&mut self.zero, true);
        }
        // Doubled past 85% full, where linear probing starts to slow sharply.
        if (self.len + 1) * 20 > self.slots.len() * 17 {
            let mut grown = vec![0; self.slots.len() * 2];
            for &k in self.slots.iter().filter(|&&k| k != 0) {
                let i = KeySet::slot(&grown, k);
                grown[i] = k;
            }
            self.slots = grown;
        }
        let i = KeySet::slot(&self.slots, key);
        if self.slots[i] == key {
            return false;
        }
        self.slots[i] = key;
        self.len += 1;
        true
    }
}

/// Training copies of each network input, nine bytes a slot kept under 85% full; a count stops at
/// 255, past any cap the command line takes.
struct CopyCounts {
    slots: Vec<u64>,
    counts: Vec<u8>,
    len: usize,
    zero: u8,
}

impl CopyCounts {
    fn new() -> CopyCounts {
        CopyCounts::with_capacity(0)
    }

    /// Room for `inputs` keys under the load bar from the start, so a bound on the inputs a run can
    /// meet means the table never doubles, the doubling holding both tables at once.
    fn with_capacity(inputs: u64) -> CopyCounts {
        let wanted = usize::try_from(inputs.saturating_mul(20) / 17 + 1).unwrap_or(usize::MAX);
        let n = wanted.max(1 << 16);
        CopyCounts {
            slots: vec![0; n],
            counts: vec![0; n],
            len: 0,
            zero: 0,
        }
    }

    /// A key's slot by multiply-high over any table length, so the table need not round up to a
    /// power of two.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the high half of a product with the slot count is below it, a usize"
    )]
    fn slot(slots: &[u64], key: u64) -> usize {
        let n = slots.len();
        let mut i = ((u128::from(splitmix64(key).0) * n as u128) >> 64) as usize;
        while slots[i] != 0 && slots[i] != key {
            i += 1;
            if i == n {
                i = 0;
            }
        }
        i
    }

    /// The input's count with this copy.
    fn bump(&mut self, key: u64) -> u8 {
        if key == 0 {
            self.zero = self.zero.saturating_add(1);
            return self.zero;
        }
        let mut i = CopyCounts::slot(&self.slots, key);
        if self.slots[i] != key {
            if (self.len + 1) * 20 > self.slots.len() * 17 {
                self.double();
                i = CopyCounts::slot(&self.slots, key);
            }
            self.slots[i] = key;
            self.len += 1;
        }
        self.counts[i] = self.counts[i].saturating_add(1);
        self.counts[i]
    }

    /// Twice the slots, every key moved; only a new key past the load bar calls it, never a repeat.
    fn double(&mut self) {
        let mut slots = vec![0; self.slots.len() * 2];
        let mut counts = vec![0; self.slots.len() * 2];
        for (&k, &n) in self
            .slots
            .iter()
            .zip(&self.counts)
            .filter(|&(&k, _)| k != 0)
        {
            let i = CopyCounts::slot(&slots, k);
            slots[i] = k;
            counts[i] = n;
        }
        self.slots = slots;
        self.counts = counts;
    }
}

/// One start kind's positions read by network input, its holdout against both kinds' training.
#[derive(Clone, Copy, Debug, Default)]
struct ByInput {
    train: usize,
    holdout: usize,
    overlap: usize,
    /// Positions in a split beyond the first with their input.
    train_repeats: usize,
    holdout_repeats: usize,
    /// Training positions by their input's copies: 1, 2 to 8, 9 to 64, 65 to 512, and more.
    copies: [u64; 5],
}

/// Entries of a sorted list beyond the first of each value.
fn repeats(sorted: &[u64]) -> usize {
    sorted.windows(2).filter(|w| w[0] == w[1]).count()
}

/// A sorted list's entries by how many times their value appears, in `ByInput::copies`' bands.
fn copies(sorted: &[u64]) -> [u64; 5] {
    let mut held = [0; 5];
    for run in sorted.chunk_by(|a, b| a == b) {
        let n = run.len() as u64;
        let band = match n {
            1 => 0,
            2..=8 => 1,
            9..=64 => 2,
            65..=512 => 3,
            _ => 4,
        };
        held[band] += n;
    }
    held
}

/// The whole conversion up to its published unit's path; `steps` renames the unit and syncs, so a
/// test can make the rename fail or read the order.
///
/// # Errors
///
/// Any refusal before a record is read, a copied game or chunk, repeats past `REPEAT_BAR`, a record
/// that does not replay, or a file that cannot be written; nothing is published then.
pub fn convert(request: &Request, steps: &mut dyn Steps) -> Result<Converted, String> {
    let sets = request
        .manifests
        .iter()
        .map(|m| read_manifest(m))
        .collect::<Result<Vec<_>, _>>()?;
    for set in &sets {
        check_listing(set)?;
    }
    let plan = plan_games(&sets, request.standard_share, request.every)?;
    fs::create_dir_all(&request.out).map_err(|e| format!("{}: {e}", request.out.display()))?;
    let staging = request.out.join(format!(".staging-{}", std::process::id()));
    fs::create_dir(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let converted = convert_into(request, &sets, &plan, &staging, steps);
    if converted.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    converted
}

/// The two passes over the positions, into `staging`, and the unit published from it: each file
/// synced and the manifest last, then the directory synced, renamed whole to its version id and
/// its parent synced, so a failure before the rename publishes nothing.
fn convert_into(
    request: &Request,
    sets: &[Dataset],
    plan: &GamePlan,
    staging: &Path,
    steps: &mut dyn Steps,
) -> Result<Converted, String> {
    let buckets = staging.join("buckets");
    let mut counts = vec![[Counts::new(), Counts::new()]; sets.len()];
    let counter = if request.max_copies.is_some() {
        CopyCounts::with_capacity(plan.train_moves)
    } else {
        CopyCounts::new()
    };
    let mut kept = Kept::new(&buckets, request.shuffle_seed, counter)?;
    let mut seen = KeySet::new();
    for_each_game(sets, plan, |game| {
        positions(
            request,
            game,
            &mut counts[game.dataset][usize::from(game.record.kind)],
            &mut kept,
            &mut seen,
        )
        .map_err(|e| format!("{} game {}: {e}", game.chunk, game.record.index))
    })?;
    drop(seen);
    let (train_len, holdout_len) = (kept.train.len(), kept.holdout.len());
    if train_len == 0 {
        return Err("no training position is left after the filters; nothing was published".into());
    }
    if holdout_len == 0 {
        return Err(
            "no validation position is left after the filters; nothing was published".into(),
        );
    }
    kept.train_keys.sort_unstable();
    let overlap = kept
        .holdout_keys
        .iter()
        .filter(|k| kept.train_keys.binary_search(k).is_ok())
        .count();
    for inputs in kept.train_inputs.iter_mut().chain(&mut kept.holdout_inputs) {
        inputs.sort_unstable();
    }
    let in_training = |k: &u64| kept.train_inputs.iter().any(|t| t.binary_search(k).is_ok());
    let by_input: [ByInput; 2] = std::array::from_fn(|kind| {
        let (train, holdout) = (&kept.train_inputs[kind], &kept.holdout_inputs[kind]);
        ByInput {
            train: train.len(),
            holdout: holdout.len(),
            overlap: holdout.iter().filter(|k| in_training(k)).count(),
            train_repeats: repeats(train),
            holdout_repeats: repeats(holdout),
            copies: copies(train),
        }
    });
    let report = report(sets, &counts, plan, overlap, &by_input, request);
    let Kept {
        train,
        holdout,
        train_keys,
        holdout_keys,
        train_inputs,
        holdout_inputs,
        copies,
    } = kept;
    drop((
        train_keys,
        holdout_keys,
        train_inputs,
        holdout_inputs,
        copies,
    ));
    let mut rng = Rng::new(request.shuffle_seed);
    let train_sha = train.drain(&staging.join("train.bin"), &mut rng)?;
    let holdout_sha = holdout.drain(&staging.join("holdout.bin"), &mut rng)?;
    fs::remove_dir(&buckets).map_err(|e| format!("{}: {e}", buckets.display()))?;
    write_synced(&staging.join("report.txt"), report.as_bytes())?;
    let manifest = manifest(
        request,
        sets,
        &counts,
        (train_len, holdout_len),
        (&train_sha, &holdout_sha),
        &report,
    );
    write_synced(&staging.join("MANIFEST.tsv"), manifest.as_bytes())?;
    let id = hex(&sha256(&[train_sha, holdout_sha].concat()))[..16].to_string();
    let unit = request.out.join(&id);
    if unit.exists() {
        return Err(format!("{} already holds this unit", unit.display()));
    }
    steps
        .sync_dir(staging)
        .map_err(|e| format!("{}: {e}", staging.display()))?;
    let published =
        durable::publish(steps, staging, &unit).map_err(|e| format!("{}: {e}", unit.display()))?;
    Ok(Converted {
        unconfirmed: published.note(&unit),
        unit,
    })
}

/// Games of one start kind read, and those dropped as repeating an earlier game from another seed.
#[derive(Clone, Copy, Debug, Default)]
struct Repeats {
    read: u64,
    dropped: u64,
}

/// What the first read of the chunks decides: whether each game, in reading order, is converted,
/// and the repeats it dropped.
struct GamePlan {
    chosen: Vec<bool>,
    repeats: [Repeats; 2],
    /// The moves of the chosen training games, more than the training positions any filter keeps.
    train_moves: u64,
}

/// Every chunk read once to choose the games: no run seed and chunk index twice, and no game copied.
/// A game repeating an earlier one from another seed is kept once, up to `REPEAT_BAR`.
fn plan_games(sets: &[Dataset], share: Option<u8>, every: u32) -> Result<GamePlan, String> {
    let mut chunks: BTreeMap<(u64, u32), String> = BTreeMap::new();
    let mut names: Vec<&str> = Vec::new();
    let mut keys: BTreeMap<[u8; 16], (usize, u32, u64)> = BTreeMap::new();
    let mut repeats = [Repeats::default(); 2];
    let (mut chosen, mut kinds) = (Vec::new(), Vec::new());
    // Each game's moves where it trains, and none where it is held out.
    let mut trains = Vec::new();
    for set in sets {
        for (name, _) in &set.chunks {
            let bytes = fs::read(set.dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
            let chunk = Chunk::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
            check_provenance(set, name, &chunk.header)?;
            let at = (chunk.header.run_seed, chunk.header.chunk_index);
            if let Some(first) = chunks.insert(at, name.clone()) {
                return Err(format!(
                    "{first} and {name} carry run seed {:016x} and chunk index {}",
                    at.0, at.1
                ));
            }
            names.push(name.as_str());
            for record in &chunk.records {
                let key = game_key(record);
                let kind = &mut repeats[usize::from(record.kind)];
                kind.read += 1;
                kinds.push(record.kind);
                let number = game_number(&chunk.header, record.index);
                let held = every > 0 && number.is_multiple_of(u64::from(every));
                trains.push(if held { 0 } else { record.moves.len() as u64 });
                if let Some(&(first, index, seed)) = keys.get(&key) {
                    if seed == record.seed {
                        return Err(format!(
                            "{name} game {} is the same game as {} game {index}",
                            record.index, names[first]
                        ));
                    }
                    kind.dropped += 1;
                    chosen.push(false);
                    continue;
                }
                keys.insert(key, (names.len() - 1, record.index, record.seed));
                chosen.push(true);
            }
        }
    }
    for (name, kind) in ["uniform", "standard"].iter().zip(&repeats) {
        if kind.dropped * REPEAT_BAR > kind.read {
            return Err(format!(
                "{} of {} {name} games repeat an earlier game from another seed, more than one in \
                 {REPEAT_BAR}",
                kind.dropped, kind.read
            ));
        }
    }
    subsample(&mut chosen, &kinds, share);
    let train_moves = chosen
        .iter()
        .zip(&trains)
        .filter(|&(&c, _)| c)
        .map(|(_, &m)| m)
        .sum();
    Ok(GamePlan {
        chosen,
        repeats,
        train_moves,
    })
}

/// The chosen games in reading order, each chunk read again, so one chunk's games are in memory at
/// a time; a chunk whose games differ in number from the first read is refused.
fn for_each_game(
    sets: &[Dataset],
    plan: &GamePlan,
    mut each: impl FnMut(&Game) -> Result<(), String>,
) -> Result<(), String> {
    let mut at = 0;
    for (dataset, set) in sets.iter().enumerate() {
        for (name, _) in &set.chunks {
            let bytes = fs::read(set.dir.join(name)).map_err(|e| format!("{name}: {e}"))?;
            let Chunk {
                header, records, ..
            } = Chunk::read(&bytes).map_err(|e| format!("{name}: {e}"))?;
            for record in records {
                let chosen = *plan
                    .chosen
                    .get(at)
                    .ok_or_else(|| format!("{name} changed between its two reads"))?;
                at += 1;
                if chosen {
                    each(&Game {
                        dataset,
                        chunk: name.clone(),
                        number: game_number(&header, record.index),
                        record,
                    })?;
                }
            }
        }
    }
    if at != plan.chosen.len() {
        return Err("a chunk changed between its two reads".into());
    }
    Ok(())
}

/// A game's place in its run, from its chunk's place and its own within the chunk.
fn game_number(header: &Header, index: u32) -> u64 {
    u64::from(header.chunk_index) * u64::from(header.games_planned) + u64::from(index)
}

/// Chosen games of the more common kind dropped from the end until the standard share is the one
/// asked, or every game kept when none is asked.
fn subsample(chosen: &mut [bool], kinds: &[u8], share: Option<u8>) {
    let Some(share) = share else {
        return;
    };
    let standard = chosen
        .iter()
        .zip(kinds)
        .filter(|&(&c, &k)| c && k == 1)
        .count() as u64;
    let uniform = chosen.iter().filter(|&&c| c).count() as u64 - standard;
    let (p, q) = (u64::from(share), 100 - u64::from(share));
    // Keep (u', s') with s' * q == u' * p, each at most what there is.
    let (keep_uniform, keep_standard) = if standard * q > uniform * p {
        (uniform, (uniform * p).checked_div(q).unwrap_or(standard))
    } else {
        ((standard * q).checked_div(p).unwrap_or(uniform), standard)
    };
    let (mut u, mut s) = (0, 0);
    for (c, &k) in chosen.iter_mut().zip(kinds) {
        if !*c {
            continue;
        }
        *c = if k == 1 {
            s += 1;
            s <= keep_standard
        } else {
            u += 1;
            u <= keep_uniform
        };
    }
}

/// The positions of one game's moves that pass every filter, each before its move is played.
fn positions(
    request: &Request,
    game: &Game,
    counts: &mut Counts,
    kept: &mut Kept,
    seen: &mut KeySet,
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
    let held = request.every > 0 && game.number.is_multiple_of(u64::from(request.every));
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
            // Deduplication needs every key; the report alone reads its sample.
            let tracked = request.dedup || in_sample(key, request.report_sample);
            let before = tracked && seen.contains(key);
            counts.within_game += u64::from(again);
            counts.across_games += u64::from(!again && before);
            if request.dedup && (again || before) {
                counts.repeated_dropped += 1;
            } else {
                if tracked {
                    seen.insert(key);
                }
                let result = result_for_mover(
                    record.outcome,
                    board.side_to_move() == cadence_core::Colour::White,
                );
                let bytes = trainer::encode(&board, score, result);
                let input = input_key(&bytes, board.side_to_move() == cadence_core::Colour::White);
                let kind = usize::from(record.kind);
                // The cap reads training only, so validation keeps every copy it held before.
                if !held
                    && request
                        .max_copies
                        .is_some_and(|cap| kept.copies.bump(input) > cap)
                {
                    counts.past_copies += 1;
                } else {
                    counts.plies[band] += 1;
                    counts.zeros[band] += u64::from(score == 0);
                    counts.scores[bucket(score)] += 1;
                    let n = request.report_sample;
                    let (keys, inputs) = if held {
                        kept.holdout.push(&bytes)?;
                        counts.holdout += 1;
                        (&mut kept.holdout_keys, &mut kept.holdout_inputs[kind])
                    } else {
                        kept.train.push(&bytes)?;
                        counts.train += 1;
                        (&mut kept.train_keys, &mut kept.train_inputs[kind])
                    };
                    if in_sample(key, n) {
                        keys.push(key);
                    }
                    if in_sample(input, n) {
                        inputs.push(input);
                    }
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

/// The network's input, its 768 piece-square features and the side to move, hashed: the record's
/// first 24 bytes are the pieces as the mover sees them, so the mover completes it.
fn input_key(record: &[u8; RECORD_LEN], white_to_move: bool) -> u64 {
    let word = |i: usize| {
        let mut w = [0; 8];
        w.copy_from_slice(&record[i..i + 8]);
        u64::from_le_bytes(w)
    };
    let mix = |x: u64| splitmix64(x).0;
    mix(word(0) ^ mix(word(8) ^ mix(word(16) ^ u64::from(white_to_move))))
}

/// Whether the report reads this key, 1 key in `n` by a hash apart from the key set's own, so a
/// sampled key's every copy is read and a sampled key set probes no denser than a whole one.
fn in_sample(key: u64, n: u32) -> bool {
    n == 1
        || splitmix64(key ^ 0x5851_f42d_4c95_7f2d)
            .0
            .is_multiple_of(u64::from(n))
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
fn report(
    sets: &[Dataset],
    counts: &[[Counts; 2]],
    plan: &GamePlan,
    overlap: usize,
    by_input: &[ByInput; 2],
    request: &Request,
) -> String {
    let mut text = String::new();
    let n = request.report_sample;
    // Figures read from a sample are scaled to the whole and say so; at 1 the text is unchanged.
    let (s, note) = if n > 1 {
        (n as usize, format!(" (1 in {n} keys, scaled)"))
    } else {
        (1, String::new())
    };
    for (name, kind) in ["uniform", "standard"].iter().zip(&plan.repeats) {
        let _ = writeln!(
            text,
            "{name} games repeating an earlier game from another seed, dropped: {} of {}",
            kind.dropped, kind.read
        );
    }
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
                "  {name} repeated positions within-game {} across-games {}{note}",
                c.within_game,
                c.across_games * s as u64
            );
            if let Some(cap) = request.max_copies {
                let _ = writeln!(
                    text,
                    "  {name} training positions past {cap} copies of their network input, dropped: {}",
                    c.past_copies
                );
            }
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
    if request.max_copies.is_some() {
        let _ = writeln!(
            text,
            "copy counter sized for {} training positions, the chosen training games' moves",
            plan.train_moves
        );
    }
    sampled_figures(&mut text, overlap, by_input, s, &note);
    text
}

/// The report's figures read by key and by network input, each scaled by `s` and marked with `note`
/// when read from a sample.
fn sampled_figures(
    text: &mut String,
    overlap: usize,
    by_input: &[ByInput; 2],
    s: usize,
    note: &str,
) {
    let _ = writeln!(
        *text,
        "holdout positions whose key is also in training: {}{note}",
        overlap * s
    );
    let input_overlap: usize = by_input.iter().map(|b| b.overlap).sum();
    let _ = writeln!(
        *text,
        "holdout positions whose network input (768 features and the side to move) is also in \
         training: {}{note}",
        input_overlap * s
    );
    for (name, b) in ["uniform", "standard"].iter().zip(by_input) {
        let _ = writeln!(
            *text,
            "{name} holdout positions whose network input is also in training: {} of {}{note}",
            b.overlap * s,
            b.holdout * s
        );
        let _ = writeln!(
            *text,
            "{name} repeats by network input within training {} of {}, within holdout {} of \
             {}{note}",
            b.train_repeats * s,
            b.train * s,
            b.holdout_repeats * s,
            b.holdout * s
        );
        let [one, few, some, many, most] = b.copies.map(|c| c * s as u64);
        let _ = writeln!(
            *text,
            "{name} training positions by copies of their network input: 1 {one} 2-8 {few} 9-64 \
             {some} 65-512 {many} over-512 {most}{note}"
        );
    }
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
    (train_len, holdout_len): (u64, u64),
    (train_sha, holdout_sha): (&[u8; 32], &[u8; 32]),
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
        "holdout\tgame number in its run modulo {}",
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
    if let Some(cap) = request.max_copies {
        let _ = write!(
            filters,
            ", at most {cap} training copies of a network input"
        );
    }
    if request.report_sample > 1 {
        let _ = write!(
            filters,
            "; report figures by key and input from 1 key in {}",
            request.report_sample
        );
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
    let _ = writeln!(text, "file\ttrain.bin\t{}\t{}", hex(train_sha), train_len);
    let _ = writeln!(
        text,
        "file\tholdout.bin\t{}\t{}",
        hex(holdout_sha),
        holdout_len
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
    use std::path::{Path, PathBuf};

    use super::{
        BUCKETS, Buckets, CopyCounts, KeySet, RECORD_LEN, Request, bucket, convert, copies,
        game_number, in_sample, input_key, request_from, result_for_mover,
    };
    use cadence_core::chess960::dfrc_fen;
    use cadence_core::generate_legal;
    use cadence_core::position::Board;
    use cadence_core::rng::Rng;
    use cadence_dataset::chunk::{Chunk, GameRecord, Header, chunk_name, encode_move};
    use cadence_dataset::durable::{Logged, Step};
    use cadence_dataset::sha256::{hex, sha256};
    use cadence_dataset::trainer::{DRAW, LOSS, WIN};

    /// The header of chunk 0 of run 1, planned at two games.
    fn test_header() -> Header {
        Header {
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
            games_planned: 2,
            host: "test".to_string(),
            start_time: 0,
            net_sha256: [0; 32],
            modified: 0,
            cfg_sha256: [0; 32],
        }
    }

    /// Two short games in a chunk of `dir`, its sidecar and manifest, and the request that converts
    /// them to `dir/out`.
    fn fixture(dir: &Path) -> Request {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).expect("a scratch directory");
        let header = test_header();
        let games = [short_game(0, 3), short_game(1, 4)];
        let bytes = Chunk::bytes(&header, &games).expect("a chunk");
        let (name, digest) = (chunk_name(1, 0), hex(&sha256(&bytes)));
        std::fs::write(dir.join(&name), &bytes).expect("writes");
        std::fs::write(
            dir.join(format!("{name}.sha256")),
            format!("{digest}  {name}\n"),
        )
        .expect("writes");
        let manifest = dir.join("MANIFEST.tsv");
        let provenance = format!(
            "commit\t{}\nversion\t0.0.0-test\nnodes\t5000\nscreen-nodes\t5000\nhash\t16\n\
             random-plies\t8\nwindow\t200\nstandard-share\t50\nchunk-games\t2\n\
             run-seeds\t{:016x}\n",
            hex(&[1; 20]),
            1
        );
        let rows = format!("chunk\tsha256\n{name}\t{digest}\n");
        std::fs::write(&manifest, provenance + &rows).expect("writes");
        let out = dir.join("out");
        Request {
            manifests: vec![manifest],
            out,
            every: 10,
            shuffle_seed: 1,
            standard_share: None,
            score_cap: None,
            swing_cap: None,
            dedup: false,
            max_copies: None,
            report_sample: 1,
        }
    }

    /// A rename that fails leaves neither the unit nor its staging behind, and one that works has the
    /// unit's entries synced before it and the unit's name synced after it.
    #[test]
    fn a_failed_rename_publishes_nothing() {
        let dir = std::env::temp_dir().join(format!("cadence-convert-unit-{}", std::process::id()));
        let request = fixture(&dir);
        let out = request.out.clone();
        let failed = convert(&request, &mut Logged::failing_at(0));
        let left: Vec<PathBuf> = std::fs::read_dir(&out)
            .expect("lists")
            .map(|e| e.expect("an entry").path())
            .collect();
        let mut logged = Logged::default();
        let published = convert(&request, &mut logged);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(failed.is_err(), "a failed rename was taken as published");
        assert_eq!(left, [] as [PathBuf; 0]);
        let unit = published
            .expect("the same request publishes once the rename works")
            .unit;
        let staging = out.join(format!(".staging-{}", std::process::id()));
        assert_eq!(
            logged.steps,
            [Step::Sync(staging), Step::Rename(unit), Step::Sync(out)],
            "the unit's entries synced before its rename, and its name after"
        );
    }

    /// A directory sync that fails before the unit's rename publishes nothing, and one that fails
    /// after it leaves the unit published and says its durability is unconfirmed.
    #[test]
    fn a_failed_sync_publishes_nothing_before_the_rename_and_is_reported_after_it() {
        let dir = std::env::temp_dir().join(format!("cadence-convert-sync-{}", std::process::id()));
        let request = fixture(&dir);
        let before = convert(&request, &mut Logged::failing_sync(0));
        let left: Vec<PathBuf> = std::fs::read_dir(&request.out).map_or_else(
            |_| Vec::new(),
            |d| d.map(|e| e.expect("an entry").path()).collect(),
        );
        let after = convert(&request, &mut Logged::failing_sync(1));
        let landed = after
            .as_ref()
            .is_ok_and(|c| c.unit.join("train.bin").exists());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            before.is_err(),
            "a sync before the rename failed and a unit was published"
        );
        assert_eq!(left, [] as [PathBuf; 0]);
        let after = after.expect("published once the rename is made");
        assert!(landed, "the unit is not in place");
        let note = after.unconfirmed.expect("the failed sync is reported");
        assert!(note.contains("published, durability unconfirmed"), "{note}");
    }

    /// Eight random plies and 32 random moves from the standard start, each move scored 5.
    fn short_game(index: u32, seed: u64) -> GameRecord {
        let mut rng = Rng::new(seed);
        let mut board = Board::from_fen(&dfrc_fen(518, 518)).expect("the start");
        let (mut plies, mut moves) = (Vec::new(), Vec::new());
        for ply in 0..40 {
            let legal = generate_legal(&board);
            let Some(&m) = legal.as_slice().get(rng.below(legal.len().max(1))) else {
                break;
            };
            if ply < 8 {
                plies.push(encode_move(m));
            } else {
                moves.push((encode_move(m), 5));
            }
            board.play(m);
        }
        GameRecord {
            index,
            seed,
            kind: 1,
            white: 518,
            black: 518,
            plies,
            moves,
            outcome: 2,
            ending: 3,
            refused: 0,
        }
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
    /// Two records whose second and third words differ so that a fold adding a constant between
    /// XORs would give them one key, as the first form of this key did, still get two.
    #[test]
    fn inputs_that_cancel_in_an_additive_fold_get_distinct_keys() {
        let golden = 0x9E37_79B9_7F4A_7C15_u64;
        let (first, second, third): (u64, u64, u64) = (
            0x0123_4567_89ab_cdef,
            0x0f0f_f0f0_0f0f_f0f0,
            0x1111_2222_3333_4444,
        );
        let moved = third ^ 0x80;
        let cancelled = second ^ third.wrapping_add(golden) ^ moved.wrapping_add(golden);
        let record = |words: [u64; 3]| {
            let mut r = [0_u8; RECORD_LEN];
            for (i, w) in words.iter().enumerate() {
                r[8 * i..8 * i + 8].copy_from_slice(&w.to_le_bytes());
            }
            r
        };
        let a = record([first, second, third]);
        let b = record([first, cancelled, moved]);
        assert_ne!(a, b);
        assert_ne!(input_key(&a, false), input_key(&b, false));
        assert_ne!(input_key(&a, false), input_key(&a, true));
    }

    #[test]
    fn the_result_is_the_movers() {
        assert_eq!(result_for_mover(0, true), WIN);
        assert_eq!(result_for_mover(0, false), LOSS);
        assert_eq!(result_for_mover(1, false), WIN);
        assert_eq!(result_for_mover(1, true), LOSS);
        assert_eq!(result_for_mover(2, true), DRAW);
    }

    /// A game's number is its place in the run whatever size the run's chunks are, so the holdout
    /// takes every tenth game of the run and not every tenth of each chunk.
    #[test]
    fn a_games_number_is_its_place_in_the_run_at_any_chunk_size() {
        for chunk_games in [1_u32, 5, 1_000] {
            let games = 2 * chunk_games.max(10);
            let numbers: Vec<u64> = (0..games)
                .map(|g| {
                    let header = Header {
                        chunk_index: g / chunk_games,
                        games_planned: chunk_games,
                        ..test_header()
                    };
                    game_number(&header, g % chunk_games)
                })
                .collect();
            let first_wrong = numbers.iter().zip(0..).position(|(&n, g)| n != g);
            assert_eq!(first_wrong, None, "chunks of {chunk_games}");
        }
    }

    /// Every record dealt and drained comes out once, in an order its two seeds fix, and the digest
    /// returned is the file's.
    #[test]
    fn buckets_return_every_record_once_in_an_order_the_seeds_fix() {
        let dir =
            std::env::temp_dir().join(format!("cadence-convert-buckets-{}", std::process::id()));
        let records: Vec<[u8; RECORD_LEN]> = (0..1_000u32)
            .map(|i| {
                let mut record = [0; RECORD_LEN];
                record[..4].copy_from_slice(&i.to_le_bytes());
                record
            })
            .collect();
        let drained = |deal: u64, shuffle: u64, name: &str| {
            let mut buckets = Buckets::new(&dir.join(name), 7, deal).expect("buckets");
            for record in &records {
                buckets.push(record).expect("pushes");
            }
            assert_eq!(buckets.len(), 1_000);
            let out = dir.join(format!("{name}.bin"));
            let digest = buckets.drain(&out, &mut Rng::new(shuffle)).expect("drains");
            let bytes = std::fs::read(&out).expect("reads");
            assert_eq!(digest, sha256(&bytes));
            assert!(!dir.join(name).exists(), "the buckets were left behind");
            bytes
        };
        let (first, again, reshuffled, redealt) = (
            drained(1, 2, "a"),
            drained(1, 2, "b"),
            drained(1, 3, "c"),
            drained(4, 2, "d"),
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(first, again, "the same seeds gave another order");
        assert_ne!(
            first, reshuffled,
            "another shuffle seed gave the same order"
        );
        assert_ne!(first, redealt, "another deal seed gave the same order");
        let sorted = |bytes: &[u8]| {
            let mut all: Vec<&[u8]> = bytes.chunks(RECORD_LEN).collect();
            all.sort_unstable();
            all.concat()
        };
        let original = sorted(&records.concat());
        for out in [&first, &reshuffled, &redealt] {
            assert_eq!(sorted(out), original, "a record was lost or repeated");
        }
    }

    /// Three records through four buckets come out in each of their six orders about as often.
    #[test]
    fn the_two_pass_shuffle_is_uniform_over_three_records() {
        let dir =
            std::env::temp_dir().join(format!("cadence-convert-uniform-{}", std::process::id()));
        let mut seen = std::collections::BTreeMap::new();
        for seed in 0..1_200u64 {
            let mut buckets = Buckets::new(&dir.join("b"), 4, seed).expect("buckets");
            for i in 0..3u8 {
                buckets.push(&[i; RECORD_LEN]).expect("pushes");
            }
            let out = dir.join("out.bin");
            buckets.drain(&out, &mut Rng::new(!seed)).expect("drains");
            let order: Vec<u8> = std::fs::read(&out)
                .expect("reads")
                .chunks(RECORD_LEN)
                .map(|r| r[0])
                .collect();
            *seen.entry(order).or_insert(0u32) += 1;
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(seen.len(), 6, "{seen:?}");
        // 200 each is expected, its standard deviation about 13.
        for (order, n) in &seen {
            assert!((140..=260).contains(n), "{order:?} came {n} times in 1,200");
        }
    }

    /// Each key is new once and held after, through the table's growth, zero included.
    #[test]
    fn the_key_set_holds_every_key_once_through_growth() {
        let mut set = KeySet::new();
        assert!(set.insert(0));
        assert!(!set.insert(0));
        let mut rng = Rng::new(9);
        let keys: Vec<u64> = (0..200_000).map(|_| rng.next_u64() | 1).collect();
        for &k in &keys {
            assert!(set.insert(k), "{k} was taken as seen");
        }
        for &k in &keys {
            assert!(!set.insert(k), "{k} was taken as new twice");
            assert!(set.contains(k));
        }
        assert!(set.contains(0));
        assert!(!set.contains(2), "an even key was never inserted");
        assert_eq!(set.len, 200_000);
    }

    #[test]
    fn copies_count_positions_by_how_often_their_value_appears() {
        let mut sorted = vec![1, 2, 2, 3, 3, 3];
        sorted.extend(std::iter::repeat_n(4, 9));
        sorted.extend(std::iter::repeat_n(5, 600));
        assert_eq!(copies(&sorted), [1, 5, 9, 0, 600]);
    }

    /// Each input's count rises by one a copy through the table's growth, zero's too, and stops at
    /// 255.
    #[test]
    fn copy_counts_count_each_input_through_growth() {
        let mut counts = CopyCounts::new();
        for round in 1..=3u8 {
            for key in 0..100_000u64 {
                assert_eq!(
                    counts.bump(key * 7),
                    round,
                    "key {} in round {round}",
                    key * 7
                );
            }
        }
        assert_eq!(counts.bump(1), 1, "a key never bumped starts at one");
        let last = (0..300).map(|_| counts.bump(5)).last();
        assert_eq!(last, Some(255));
    }

    /// A table sized for `n` inputs holds `n` distinct inputs without growing, each counted, and
    /// takes about 20 slots for every 17 of them rather than a power of two's rounding.
    #[test]
    fn copy_counts_sized_for_their_inputs_never_grow() {
        let n = 300_000u64;
        let mut counts = CopyCounts::with_capacity(n);
        let slots = counts.slots.len();
        assert_eq!(slots as u64, n * 20 / 17 + 1);
        for round in 1..=2u8 {
            for key in 1..=n {
                assert_eq!(
                    counts.bump(key * 11),
                    round,
                    "key {} in round {round}",
                    key * 11
                );
            }
        }
        assert_eq!(counts.slots.len(), slots, "the table grew");
        assert_eq!(counts.len as u64, n);
    }

    /// The report's sample reads about one key in `n`, and every key at one.
    #[test]
    fn the_report_sample_reads_about_one_key_in_n() {
        let read = |n: u32| (0..1_000_000u64).filter(|&k| in_sample(k, n)).count();
        assert_eq!(read(1), 1_000_000);
        let sixteenth = read(16);
        assert!(
            (61_250..=63_750).contains(&sixteenth),
            "{sixteenth} of a million keys at 1 in 16"
        );
    }

    /// The cap is one to 255 copies, and absent unless asked.
    #[test]
    fn the_copies_cap_takes_one_to_255() {
        let ask = |cap: &str| {
            let args: Vec<String> = [
                "--manifest",
                "m",
                "--out",
                "o",
                "--shuffle-seed",
                "1",
                "--max-copies",
                cap,
            ]
            .iter()
            .map(ToString::to_string)
            .collect();
            request_from(&args).map(|r| r.max_copies)
        };
        assert_eq!(ask("32"), Ok(Some(32)));
        assert_eq!(ask("255"), Ok(Some(255)));
        assert!(ask("0").is_err_and(|e| e.contains("trains on nothing")));
        assert!(ask("256").is_err_and(|e| e.contains("1 to 255")));
        let none: Vec<String> = ["--manifest", "m", "--out", "o", "--shuffle-seed", "1"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(request_from(&none).map(|r| r.max_copies), Ok(None));
    }
}
