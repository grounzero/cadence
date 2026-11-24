# Cadence

[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)
[![CI](https://github.com/grounzero/cadence/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/grounzero/cadence/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/grounzero/cadence?label=latest%20release)](https://github.com/grounzero/cadence/releases/latest)
[![Pre-release](https://img.shields.io/github/v/release/grounzero/cadence?include_prereleases&label=pre-release)](https://github.com/grounzero/cadence/releases)

Cadence is a chess engine written in Rust. It plays standard chess, Chess960
and double Chess960 (DFRC), using an alpha-beta search and a neural network
(NNUE) trained only on its own games. The handcrafted evaluation it started
with is still available as an option.

The network has been trained three times, each on games the one before it
played: net 1 learned from games played by the handcrafted evaluation, net 2
from net 1's games, and net 3, the one built in, from net 2's and net 1's.

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
  `windows-x86-64-generic.exe` if it won't start (older processors), or
  `windows-x86-64-avx512.exe` on a processor with AVX-512. The file isn't
  signed, so Windows SmartScreen may stop it the first time: click "More
  info", then "Run anyway".
- **macOS**: the file ending `macos-arm64` (Apple silicon) or
  `macos-x86-64` (Intel). Make it executable and clear its quarantine flag
  before running it:

  ```sh
  chmod +x cadence-<version>-macos-arm64
  xattr -d com.apple.quarantine cadence-<version>-macos-arm64
  ```

  For the Intel file:

  ```sh
  chmod +x cadence-<version>-macos-x86-64
  xattr -d com.apple.quarantine cadence-<version>-macos-x86-64
  ```

  The Intel build needs a processor with AVX2.
- **Linux**: the file ending `linux-x86-64-avx2`, or `linux-x86-64-generic`
  if it won't start (older processors), or `linux-x86-64-avx512` on a
  processor with AVX-512, or `linux-aarch64` for 64-bit ARM.
  The Linux builds need glibc 2.34 or later. Make the file executable:

  ```sh
  chmod +x cadence-<version>-linux-x86-64-avx2
  ```

Then add the file to your GUI as a new engine.

### Install with a script

Under WSL the script installs a Linux binary, which a Windows GUI can't load;
for a Windows GUI, use the PowerShell script.

The install script picks the right build for your system, checks its
checksum and bench, and can update or uninstall it later.

It installs to `~/.local/bin` (`%LOCALAPPDATA%\Cadence` on Windows). If a
download does not match the release's checksum, attestation or bench, it
stops and builds nothing in its place. It builds from source only with
`--source`, or where the release has no binary for your system, such as
Windows on ARM64. It asks before anything big: building, installing Rust,
changing your PATH or downloading tablebases. If a run fails, attach the
log it names to a GitHub issue.

#### Windows (PowerShell)

```powershell
Invoke-WebRequest https://github.com/grounzero/cadence/releases/latest/download/install.ps1 -OutFile install.ps1 -UseBasicParsing
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
| `--uninstall` | Removes Cadence, and the PATH entry and tablebases if the script added them, and names any compiler or Rust it installed. |
| `--tables` | Downloads the 3-4-5 piece tablebases without asking. |
| `--yes` | Takes every default without asking. |

### Building from source

```sh
git clone https://github.com/grounzero/cadence.git
cd cadence
git checkout <version>   # optional: a release instead of main
cargo build --release
```

You need Git, a C linker (on macOS the Xcode Command Line Tools, on Linux
`gcc` or `clang`, on Windows the Visual Studio C++ build tools) and Rust from
[rustup](https://rustup.rs), which fetches the pinned toolchain. Add `target/release/cadence` to your GUI as a UCI engine. On
x86-64 Linux, Windows and Intel Macs it needs AVX2; for an older processor,
add `--config .cargo/x86-64-generic.toml` (POPCNT only). On Linux or Windows
with AVX-512, `--config .cargo/x86-64-avx512.toml` adds AVX-512F and
AVX-512BW. Build from the repository's root, where Cargo finds
`.cargo/config.toml`.

### UCI options

| Option | Default | What it does |
|---|---|---|
| `Hash` | 16 | Memory for the search table, in MB (1 to 4096). |
| `Threads` | 1 | Search threads. Only one thread gives the same result every time. |
| `MultiPV` | 1 | How many best lines to show. More lines cost search time. |
| `Ponder` | off | Think on the opponent's time. Rating lists turn this off. |
| `Move Overhead` | 5 | Milliseconds kept back from every move for the GUI and the connection (0 to 5000). |
| `Clear Hash` | button | Empties the search table. |
| `UCI_LimitStrength` | off | Play weaker, at the level set by `UCI_Elo`. |
| `UCI_Elo` | 1800 | The level to play at when `UCI_LimitStrength` is on (1000 to 1800). The levels are approximate, not yet measured against rated play. |
| `UCI_Chess960` | off | Chess960 castling. GUIs set this for you. |
| `SyzygyPath` | empty | Folders of Syzygy tablebase files, separated by `:` (`;` on Windows). Empty means no tablebases. |
| `SyzygyProbeLimit` | 7 | The most pieces a position can have for the search to look it up (0 to 7). |
| `Use NNUE` | on | Score positions with a network instead of the handcrafted evaluation: the one `EvalFile` loaded, or else the one built into the engine. Off plays with the handcrafted evaluation. |
| `EvalFile` | empty | A network file to use in place of the built-in one, which emptying this restores. The engine reads networks of 256 or 512 units a perspective, as the file's header names. A file for another network shape, or one whose weights could overflow, is refused with the reason and leaves the built-in network, if there is one. |
| `UCI_EngineAbout` | read only | The engine's name, version, author and repository. |
| `reverse_futility_margin`, `lmp_multiplier`, `futility_margin`, `probcut_margin`, `aspiration_delta` | 81, 0.878, 144, 62, 30 | Search constants offered to tuners (50 to 400, 0 to 2, 50 to 400, 25 to 250, 8 to 64). Leave them alone for play. |

On macOS, raise the open-file limit with `ulimit -n` before starting the
engine or the GUI: the 3-4-5 piece tables are 290 files, and macOS allows 256.
A 6-piece set on Linux may need a higher limit too. Keep the tables on a local
disk: a stalled network drive can delay `stop`.

### Unusual commands

- `go searchmoves` searches only the listed legal moves, and answers
  `bestmove 0000` if none of them is legal or the list is empty.
- `setoption name Hash` during a search stops that search first, so `bestmove` comes at once.
- After a `position` command fails, or is longer than 1 MiB and discarded,
  `go` answers `bestmove 0000` until a valid `position`.

## How it is tested

Every strength change is played against the current
version on [OpenBench](https://github.com/AndyGrant/OpenBench), usually for
thousands of games, until it is clear whether it helps. A change is kept if it
wins its test; a correctness fix can be kept on evidence that it costs no
strength. Every SPRT, passed or failed, is in
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
```

The second line runs the slow tests, the deep perft runs among them.
[.github/workflows/ci.yml](.github/workflows/ci.yml) has CI's full set of checks.

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

Engine commands, from `cadence`:

| Command | What it is for |
|---|---|
| `cadence bench` | The fixed benchmark whose node count must match `bench.txt`. |
| `cadence perft` | Counting the legal move tree, to check move generation. |
| `cadence spsa` | Printing the input an SPSA tuner needs. |
| `cadence datagen` | Playing self-play games and cutting them into training positions. |

Data and training tools, from `cadence-tools` (in `tools/`) and `cadence-train` (in `train/`, outside the workspace):

| Command | What it is for |
|---|---|
| `cadence-tools magics` | Finding the sliding-attack magic numbers. |
| `cadence-tools convert` | Turning game datasets into the trainer's records. |
| `cadence-tools forward` | Checking a network file's quantised evaluation. |
| `cadence-train run` | Training a network (needs a GPU). |
| `cadence-train validate` | Scoring saved checkpoints on the holdout (needs a GPU). |
| `cadence-train export` | Writing a checkpoint as a network file. |
| `cadence-train book` | Turning a tuning set into the trainer's records. |
| `cadence-train eval` | Printing a checkpoint's scores for positions (needs a GPU). |
| `cadence-train overfit` | Checking a training setup before a full run. |

Run any of them with `--help` for its options.
`cadence texel`, which fitted the handcrafted evaluation's weights, is still
there but no longer part of the workflow.

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
