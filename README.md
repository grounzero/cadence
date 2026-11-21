# Cadence

[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)
[![CI](https://github.com/grounzero/cadence/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/grounzero/cadence/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/grounzero/cadence?label=latest%20release)](https://github.com/grounzero/cadence/releases/latest)
[![Pre-release](https://img.shields.io/github/v/release/grounzero/cadence?include_prereleases&label=pre-release)](https://github.com/grounzero/cadence/releases)

Cadence is a chess engine written in Rust. It plays standard chess, Chess960
and double Chess960 (DFRC), using an alpha-beta search and a neural network
(NNUE) trained only on its own games. The handcrafted evaluation it started
with is still available as an option.

Play it on Lichess as [Mithandros](https://lichess.org/@/Mithandros). The bot
also chats: a small language model answers messages and remarks on its
thinking, with no say in its moves.

## Strength

| Version | Estimate (own gauntlet) | CCRL Blitz | CCRL 40/15 | Released |
| --- | --- | --- | --- | --- |
| [0.6.0](https://github.com/grounzero/cadence/releases/tag/0.6.0) | - | [2801 ± 23](https://computerchess.org.uk/404/cgi/engine_details.cgi?print=Details&each_game=1&eng=Cadence%200.6.0%2064-bit), as of 2026-11-21 | - | 2026-11-14 |
| [0.6.0-rc.3](https://github.com/grounzero/cadence/releases/tag/0.6.0-rc.3) | 2796 ± 9 | - | - | - |
| [0.4.8](https://github.com/grounzero/cadence/tree/0.4.8) | 2289 ± 14 | - | - | 2026-09-04 |

Estimates are from gauntlets against rated engines. CCRL columns are CCRL's official ratings.

Details: [docs/calibration/strength.md](docs/calibration/strength.md).

## Getting started

Cadence has no board of its own: you play it inside a free chess GUI such as
[En Croissant](https://encroissant.org),
[Cute Chess](https://github.com/cutechess/cutechess),
[Arena](http://www.playwitharena.de) or
[BanksiaGUI](https://banksiagui.com).

### Download

Download the file for your system from the
[latest release](https://github.com/grounzero/cadence/releases/latest).
Each is named `cadence-<version>-` followed by the ending below.

- **Windows**: the file ending `windows-x86-64-avx2.exe`, or
  `windows-x86-64-generic.exe` if it won't start (older processors). The
  file isn't signed, so Windows SmartScreen may stop it the first time:
  click "More info", then "Run anyway".
- **macOS**: the file ending `macos-arm64` (Apple silicon) or
  `macos-x86-64` (Intel). Make it executable and clear its quarantine flag
  before running it:

  ```sh
  chmod +x cadence-<version>-macos-arm64
  xattr -d com.apple.quarantine cadence-<version>-macos-arm64
  ```

- **Linux**: the file ending `linux-x86-64-avx2`, or `linux-x86-64-generic`
  if it won't start (older processors), or `linux-aarch64` for 64-bit ARM.

Then add the file to your GUI as a new engine.

### Install with a script

The install script picks the right build for your system, checks its
checksum and bench, and can update or uninstall it later.

It installs to `~/.local/bin` (`%LOCALAPPDATA%\Cadence` on Windows), or
offers to build from source if no release binary fits. It asks before
anything big: building, installing Rust, changing your PATH or downloading
tablebases. If a run fails, attach the log it names to a GitHub issue.

#### Windows (PowerShell)

```powershell
Invoke-WebRequest https://github.com/grounzero/cadence/releases/latest/download/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File install.ps1
```

#### Linux, macOS, or Windows with Git Bash or WSL

```sh
curl -fsSLO https://github.com/grounzero/cadence/releases/latest/download/install.sh
bash install.sh
```

#### Install script options

Both scripts take the same flags.

| Flag | What it does |
|---|---|
| `--version X.Y.Z` | Installs that release instead of the latest. |
| `--dir DIR` | Installs into `DIR` instead of the default folder. |
| `--generic` | On x86-64, installs the generic build, for processors without AVX2. |
| `--source` | Builds from source even where a release binary fits. |
| `--update` | Upgrades an existing install, replacing the file even if this script did not put it there (it asks first). |
| `--uninstall` | Removes Cadence, and the PATH entry and tablebases if the script added them. |
| `--tables` | Downloads the 3-4-5 piece tablebases without asking. |
| `--yes` | Takes every default without asking. |

### Building from source

```sh
git clone https://github.com/grounzero/cadence.git
cd cadence
cargo build --release
```

You need Git and Rust from [rustup](https://rustup.rs), which fetches the
pinned toolchain. Add `target/release/cadence` to your GUI as a UCI engine. On
x86-64 Linux, Windows and Intel Macs it needs AVX2; for an older processor,
add `--config .cargo/x86-64-generic.toml` (POPCNT only). Build from the
repository's root, where Cargo finds `.cargo/config.toml`.

### UCI options

| Option | Default | What it does |
|---|---|---|
| `Hash` | 16 | Memory for the search table, in MB (1 to 4096). |
| `Threads` | 1 | Search threads. Only one thread gives the same result every time. |
| `MultiPV` | 1 | How many best lines to show. More lines cost search time. |
| `Ponder` | off | Think on the opponent's time. Rating lists turn this off. |
| `Move Overhead` | 20 | Milliseconds kept back from every move for the GUI and the connection (0 to 5000). |
| `Clear Hash` | button | Empties the search table. |
| `UCI_LimitStrength` | off | Play weaker, at the level set by `UCI_Elo`. |
| `UCI_Elo` | 1800 | The level to play at when `UCI_LimitStrength` is on (1000 to 1800). |
| `UCI_Chess960` | off | Chess960 castling. GUIs set this for you. |
| `SyzygyPath` | empty | Folders of Syzygy tablebase files, separated by `:` (`;` on Windows). Empty means no tablebases. |
| `SyzygyProbeLimit` | 7 | The most pieces a position can have for the search to look it up (0 to 7). |
| `Use NNUE` | on | Score positions with a network instead of the handcrafted evaluation: the one `EvalFile` loaded, or else the one built into the engine. Off plays with the handcrafted evaluation. |
| `EvalFile` | empty | A network file to use in place of the built-in one, which emptying this restores. The engine reads networks of 256 or 512 units a perspective, as the file's header names. A file for another network shape, or one whose weights could overflow, is refused with the reason and leaves the built-in network, if there is one. |
| `UCI_EngineAbout` | read only | The engine's name, version, author and repository. |
| `reverse_futility_margin`, `lmp_multiplier` | 150, 0.500 | Search constants offered to tuners (50 to 300, 0 to 2). Leave them alone for play. |

On macOS, raise the open-file limit with `ulimit -n` before starting the
engine or the GUI: the 3-4-5 piece tables are 290 files, and macOS allows 256.

### Unusual commands

- `go searchmoves` searches only the listed legal moves, or every move if none is legal.
- `setoption name Hash` during a search stops that search first, so `bestmove` comes at once.
- After a `position` command fails, `go` answers `bestmove 0000` until a valid `position`.

## How it is tested

Every strength change is played against the current
version on [OpenBench](https://github.com/AndyGrant/OpenBench), usually for
thousands of games, until it is clear whether it helps. It is kept only if it
wins. Every SPRT, passed or failed, is in
[docs/testing/sprt-log.csv](docs/testing/sprt-log.csv). Each result is
measured against the previous version, so the gains do not add up to a
rating; the [strength page](docs/calibration/strength.md) has the ratings.

You can also check:

- **The benchmark.** `cadence bench` searches fixed positions the same way
  every time, so its node count changes only when the engine's thinking
  does. CI checks it against `bench.txt`.
- **Move generation**, against a published set of positions with known
  answers: the [perft corpus](docs/testing/perft.md).

The worker setup is in `openbench/`; run
`python3 openbench/setup-worker.py --help` for its options.

## For developers

### Tests

```sh
cargo test --workspace
cargo test --workspace --release -- --ignored
cargo fmt --all -- --check
cargo fmt --manifest-path xtask/Cargo.toml -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --workspace -- -D warnings
cargo clippy --manifest-path xtask/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path xtask/Cargo.toml
cargo xtask check-headers
cargo xtask check-boundary
cargo xtask check-changelog
cargo fmt --manifest-path train/Cargo.toml -- --check
cargo clippy --manifest-path train/Cargo.toml --all-targets -- -D warnings
cargo build -p cadence-tools
CADENCE_TOOLS=target/debug/cadence-tools cargo test --manifest-path train/Cargo.toml
```

The second line runs the slow tests, the deep perft runs among them. The rest
are CI's checks; `xtask` and `train` are separate crates, so they have lines of
their own, and `train`'s tests run the `cadence-tools` built before them.

### Benchmark and perft

```sh
cargo run --release --bin cadence -- bench
cargo run --release --bin cadence -- perft startpos 6
cargo run --release --bin cadence -- perft --divide "<fen>" <depth>
```

The last line of `bench` must match `bench.txt`, and a commit that changes it
ends with a `Bench: <n>` line. The tests check perft against the
[perft corpus](docs/testing/perft.md).

### Tools

| Command | What it does |
|---|---|
| `cadence bench` | The fixed benchmark. |
| `cadence perft [--divide] [--threads N] <fen \| startpos> <depth>` | Counts the legal moves to a depth. |
| `cadence spsa` | Prints an SPSA tuner's input, one line per tuning option above. |
| `cadence datagen play --games N --seed S [--threads N] [--out PATH]` | Plays games against itself; the same seed and game count give the same file at any thread count. |
| `cadence datagen extract <games> --train PATH --holdout PATH [--every N]` | Cuts games into positions, every Nth game to the holdout set. |
| `cadence datagen run --out DIR --run-seed HEX\|fresh [--threads N] [--chunk-games N] [--standard-share P] [--host NAME]` | Writes packed chunks of games until a `STOP` file appears in `DIR`; a `PAUSE` file pauses it. The games are played with the network built into the engine, whose SHA-256 every chunk's header names. Built only with the `packed-datagen` feature. |
| `cadence datagen repair CHUNK.tmp` | Closes a chunk a killed run left, keeping its whole records. Built only with the `packed-datagen` feature. |
| `cadence datagen verify PATH... [--replay 1/N] [--threads N] [--manifest]` | Checks chunks against their sidecars, this build's settings and network and the rules, regenerating every Nth game; `--manifest` writes a directory's `MANIFEST.tsv` once every chunk in it passes. Built only with the `packed-datagen` feature. |
| `cadence texel <data> [--holdout N \| --holdout-file PATH] [--iterations N] [--rate R] ...` | Fits the evaluation's weights to positions, each a FEN, a `\|` and the result. |
| `cadence-tools magics [seed]` | A second binary, from `tools/`: finds the sliding-attack magics and prints them as Rust. |
| `cadence-tools convert --manifest FILE... --out DIR --shuffle-seed HEX\|fresh [--every 10] [--standard-share P] [--score-cap N] [--swing-cap N] [--dedup] [--max-copies N] [--report-sample N]` | Turns datasets of packed chunks into the trainer's position records, split by game and shuffled, published under `DIR` as one directory named by its contents. `--max-copies N` keeps at most N training copies of a position's network input, the first read, and counts the rest. `--report-sample N` reads the report's repeats, overlaps and copies from one key in N and scales them, for a conversion too large to hold every key; the records are the same. |
| `cadence-tools forward --net FILE [--raw FILE [--against FILE]] [--positions N] [--seed HEX]` | Scores at least 1,000 random legal positions with a net file's quantised evaluation and with an `f32` one written from the same file, and passes when no position differs by more than 5 centipawns and the mean by no more than 1. `--raw` also compares the trainer's unquantised weights, a reading for training time that decides nothing, and `--against` compares them with the trainer's own scores as `cadence-train eval` prints them, another such reading. |
| `cadence-train run --data FILE... --manifest FILE... --holdout FILE --out DIR --id NAME [--superbatches N] [--batches N] [--result-share F] [--validation-positions N] [--hidden 256\|512]` | A third binary, from `train/`, outside the workspace: trains the network on those records with [bullet](https://github.com/jw1912/bullet). Each data file must hash to the training file a manifest names, `cadence-tools convert`'s or `cadence-train book`'s, or the run stops before it starts; two files with the same records are refused, the holdout must be the holdout split of a manifest a data file matched, and a run never shares its directory with another's. The manifests it verified are listed by SHA-256 in `verified-manifests` beside the checkpoints. After training, each saved checkpoint is scored over the holdout's first 20,000 records by the trainer's own forward pass, and its validation loss printed beside the training loss it logged. The defaults are the net's recipe: 40 superbatches of 6,104 batches, the target half result and half score, 256 units a perspective. `--hidden 512` trains the wider network, its output weights clipped to 1.0 so that the engine can load it. It needs a GPU and the `cuda` feature. |
| `cadence-train export --checkpoint DIR --manifest FILE --provenance NAME --out FILE` | Writes one checkpoint as the file `EvalFile` loads, naming the data's manifest and the commit that trained it; the manifest must be one its run verified. |
| `cadence-train book --in FILE --out FILE --seed HEX [--every N]` | Turns a tuning set's text, one position a line as a FEN and White's result in brackets, into the trainer's records, scored zero, every Nth line (by default the tenth) held out to `FILE.holdout`, each part shuffled from the seed, with a manifest beside them naming both that an export hashes into the net's header. |
| `cadence-train validate --checkpoint DIR... --holdout FILE --manifest FILE... [--validation-positions N] [--result-share F]` | The validation loss `run` prints, for checkpoints already saved. It needs a GPU and the `cuda` feature. |
| `cadence-train eval --checkpoint DIR --fens FILE` | Prints the trainer's own score for each FEN in the file, in centipawns, from one saved checkpoint, the same network `run` builds. It needs a GPU and the `cuda` feature. |

### Git hooks

```sh
cargo xtask install-hooks
```

Run it once in each clone, since Git does not copy hooks: `pre-commit` runs
the boundary and punctuation checks, and `commit-msg` checks the message. The
hooks only find problems sooner, because CI runs every check, the messages
included.

## Acknowledgements

- The [Chess Programming Wiki](https://www.chessprogramming.org), which
  explains nearly every technique Cadence uses.
- [OpenBench](https://github.com/AndyGrant/OpenBench), which runs every
  strength test, and [fastchess](https://github.com/Disservin/fastchess),
  which plays the games.
- [Ordo](https://github.com/michiguel/Ordo), which fits the ratings on the
  strength page.
- [Bullet](https://github.com/jw1912/bullet), which trains the networks.
- [shakmaty](https://github.com/niklasf/shakmaty) and
  [shakmaty-syzygy](https://github.com/niklasf/shakmaty/tree/main/shakmaty-syzygy),
  which read the Syzygy endgame tablebases.
- The [8moves_v3 and UHO_Lichess_4852_v1](https://github.com/official-stockfish/books)
  opening books, used for testing.
- The [CCRL](https://computerchess.org.uk/), whose published ratings
  anchor the gauntlets, and the engines played in them, listed on the
  [strength page](docs/calibration/strength.md).
- [Lichess](https://lichess.org) and
  [lichess-bot](https://github.com/lichess-bot-devs/lichess-bot), where
  Cadence plays as Mithandros.
- The authors of [Ethereal](https://github.com/AndyGrant/Ethereal),
  [Obsidian](https://github.com/gab8192/Obsidian),
  [Reckless](https://github.com/codedeliveryservice/Reckless),
  [Stockfish](https://github.com/official-stockfish/Stockfish) and
  [Weiss](https://github.com/TerjeKir/weiss), whose open-source engines were
  studied to understand the techniques Cadence implements in its own code.

## License

Cadence is licensed under GPL-3.0-or-later. See `LICENSE`.
