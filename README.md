# Cadence

[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)
[![CI](https://github.com/grounzero/cadence/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/grounzero/cadence/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/grounzero/cadence?label=latest%20release)](https://github.com/grounzero/cadence/releases/latest)
[![Pre-release](https://img.shields.io/github/v/release/grounzero/cadence?include_prereleases&label=pre-release)](https://github.com/grounzero/cadence/releases)

Cadence is a UCI chess engine written in Rust, for standard chess, Chess960
and double Chess960 (DFRC), using a classical alpha-beta search and a
hand-written evaluation tuned on its own games, with no neural network. It
reads Syzygy endgame tablebases through the `shakmaty-syzygy` and `shakmaty`
crates, which with the crates they use are its only outside dependencies.

You can play it on Lichess as the bot
[Mithandros](https://lichess.org/@/Mithandros). The bot also chats: a small
language model beside it answers messages and remarks on its thinking, with no
say in the moves.

## Strength

| Version | Estimate (own gauntlet) | CCRL Blitz | CCRL 40/15 | Released |
| --- | --- | --- | --- | --- |
| [0.6.0](https://github.com/grounzero/cadence/releases/tag/0.6.0) (measured on 0.6.0-rc.3) | 2796 ± 9 | - | - | 2026-11-14 |
| [0.4.8](https://github.com/grounzero/cadence/tree/0.4.8) | 2289 ± 14 | - | - | 2026-09-04 |

Estimates are from gauntlets against rated engines, not official
ratings. Method, games and caveats:
[docs/calibration/strength.md](docs/calibration/strength.md).

## Getting started

Or download a binary from the
[latest release](https://github.com/grounzero/cadence/releases/latest): the
AVX2 build for x86-64 machines from about 2015 on, the generic build if it
won't start. On macOS, make the downloaded binary executable and clear its
quarantine flag before running it:

```sh
chmod +x <file>
xattr -d com.apple.quarantine <file>
```

### With an install script

`install.sh` works on Linux, macOS, and Windows under Git Bash or WSL.
`install.ps1` works in Windows PowerShell.

```sh
curl -fsSLO https://github.com/grounzero/cadence/releases/latest/download/install.sh
bash install.sh
```

```powershell
Invoke-WebRequest https://github.com/grounzero/cadence/releases/latest/download/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File install.ps1
```

It downloads the latest release binary, checks its checksum and
bench, and installs it to `~/.local/bin` (`%LOCALAPPDATA%\Cadence` on
Windows), or offers to build from source if none fits. It asks before anything
big: building, installing Rust, changing your PATH or downloading tablebases.

Options: `--version X.Y.Z`, `--dir DIR`, `--generic`, `--source`, `--update`,
`--uninstall`, `--tables` and `--yes`. If a run fails, attach the log it
names to a GitHub issue.

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

### Options

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
```

The second line runs the slow tests, the deep perft runs among them. The rest
are CI's checks; `xtask` is a separate crate, so it has lines of its own.

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
| `cadence datagen run --out DIR --run-seed HEX\|fresh [--threads N] [--chunk-games N] [--standard-share P] [--host NAME]` | Writes packed chunks of games until a `STOP` file appears in `DIR`; a `PAUSE` file pauses it. Built only with the `packed-datagen` feature. |
| `cadence datagen repair CHUNK.tmp` | Closes a chunk a killed run left, keeping its whole records. Built only with the `packed-datagen` feature. |
| `cadence datagen verify PATH... [--replay 1/N] [--threads N] [--manifest]` | Checks chunks against their sidecars, this build's settings and the rules, regenerating every Nth game; `--manifest` writes a directory's `MANIFEST.tsv` once every chunk in it passes. Built only with the `packed-datagen` feature. |
| `cadence texel <data> [--holdout N \| --holdout-file PATH] [--iterations N] [--rate R] ...` | Fits the evaluation's weights to positions, each a FEN, a `\|` and the result. |
| `cadence-tools magics [seed]` | A second binary, from `tools/`: finds the sliding-attack magics and prints them as Rust. |

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
