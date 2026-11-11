# Cadence

[![CI](https://github.com/grounzero/cadence/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/grounzero/cadence/actions/workflows/ci.yml)
[![Version](https://img.shields.io/github/v/tag/grounzero/cadence?sort=semver&label=version)](https://github.com/grounzero/cadence/tags)
[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)
[![Lichess bot](https://img.shields.io/badge/lichess-Mithandros-white?logo=lichess&logoColor=black)](https://lichess.org/@/Mithandros)

Cadence is a chess engine written in Rust. It speaks UCI, so it works in any
chess GUI that supports UCI engines. It plays standard chess and Chess960,
including double Chess960 (DFRC).

It uses a classical alpha-beta search and a hand-written evaluation. The
evaluation's weights are tuned on Cadence's own games. There is no neural
network. It can read Syzygy endgame tablebases, set with `SyzygyPath`. It
reads them through the `shakmaty-syzygy` and `shakmaty` crates, which with
the crates they use are its only dependencies from outside this repository.

You can play it on Lichess, where it runs as the bot
[Mithandros](https://lichess.org/@/Mithandros). The bot also chats. A small
language model running beside it answers messages and remarks on its own
thinking, such as how deep it searched. The model is separate from the engine
and has no say in the moves, and which model it uses is configurable.

## Getting started

You need Git and Rust. Install Rust with [rustup](https://rustup.rs). The
repository pins the Rust version it builds with, and rustup fetches it for you.

```sh
git clone https://github.com/grounzero/cadence.git
cd cadence
cargo build --release
```

The engine is now at `target/release/cadence`. In your GUI, add it as a UCI
engine. For Chess960, turn on the GUI's Chess960 mode and it will tell the
engine.

On x86-64 Linux and Windows this builds for processors with AVX2, using the
flags in `.cargo/config.toml`. On a processor without AVX2 that engine stops
at once with an illegal instruction. For one of those, build the generic
version instead, which needs only POPCNT:

```sh
cargo build --release --config .cargo/x86-64-generic.toml
```

Build from the repository's root. Cargo reads the per-target flags in
`.cargo/config.toml` from the directory it runs in, so a build started
elsewhere, with `cargo install --path` or `--manifest-path` from a parent
directory, gets none of them.

To talk to the engine directly, run it and type UCI commands:

```text
$ ./target/release/cadence
uci
isready
position startpos
go depth 8
quit
```

### With an install script

`install.sh` installs Cadence on Linux, macOS, and Windows under Git Bash or
WSL. `install.ps1` does the same on Windows with PowerShell alone. Each asks
before every step. It reads the system, and on x86-64 which of the two builds
the processor runs, then takes the latest release's binary for it. The binary
is used only once its SHA-256 matches the release's `SHA256SUMS`, its bench
count matches the release's `bench.txt` and it names the release's version.
Where no binary fits or one fails that check, the script says why and offers
to build the release from source: it checks for Git, a C linker and Rust
installed through rustup, offers to install whichever is missing, and holds
the build to the same bench and version. The default folder is
`~/.local/bin`, or `%LOCALAPPDATA%\Cadence` on Windows. It can also download
the 3-4-5 piece Syzygy tablebases, held to their published checksums.

```sh
curl -fsSLO https://raw.githubusercontent.com/grounzero/cadence/main/install.sh
bash install.sh
```

On Windows without a Unix shell, in PowerShell:

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/grounzero/cadence/main/install.ps1 -OutFile install.ps1
powershell -ExecutionPolicy Bypass -File install.ps1
```

Both take `--version X.Y.Z`, `--prefix DIR`, `--source` to build from source
even where a binary fits, `--update`, `--uninstall`, `--yes` to take every
default without asking, `--keep-log`, and on x86-64 `--build avx2` or `--build
generic`. The release workflow publishes both scripts and `bench.txt` with
each release and lists their SHA-256s beside the binaries'. A run that fails
keeps its log and says where it is: attach that file to a GitHub issue.

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
| `reverse_futility_margin` | 150 | A search constant offered to tuners (50 to 300). Leave it alone for play. |
| `lmp_multiplier` | 0.500 | A search constant offered to tuners (0 to 2). Leave it alone for play. |

Cadence keeps each tablebase file open once it has read it, until `SyzygyPath`
is set again. So the system's limit on open files must be above the number of
files. The 3-4-5 piece set is 290 files, and macOS starts programs with a
limit of 256. Raise it with `ulimit -n` in the shell that starts the engine or
the GUI. A file that cannot be read is named in an `info string`, and the
search goes on without it.

### Unusual commands

`go searchmoves` limits the search to the moves listed. A listed move that is
not legal in the position is named in an `info string` and left out. If no
listed move is legal, the engine searches every legal move, as a plain `go`
would.

`setoption name Hash` sent while a search runs stops that search first, so
its `bestmove` comes at once and the new table is allocated after it.

A `position` command can fail: its FEN may be one the engine cannot accept,
its `moves` list may hold an illegal move, or it may name neither `startpos`
nor `fen`. The engine says why in an `info string` and remembers the failure.
Every `go` after it answers `bestmove 0000` and searches nothing, until a
valid `position` command clears it.

## Strength

There are two numbers here. They come from different places and cannot be
compared with each other. Neither is an official CCRL rating, because CCRL
has not tested Cadence yet.

### Against rated engines: about 2289

This is an estimate of where Cadence 0.4.8 would sit on the CCRL Blitz list,
measured on 2026-09-28.

Cadence played 2,000 games at 2 minutes plus 1 second a move. It played 400
games against each of five engines that are on that list. The program Ordo
then worked out the rating that best fits those results (2289, give or take 14
from luck in the games).

Treat it as rough. Each of the five opponents, taken alone, gives a different
answer: anywhere from 2241 to 2318. And the games ran on a Mac, which is not
the computer CCRL uses, so a minute of thinking is not the same amount of
work.

Cadence has improved since 0.4.8, but the newer versions have not been
measured this way. Gains against its own earlier versions do not translate
into rating points, so there is no estimate for the current version.

You can check the figure yourself. The games, the ratings used and Ordo's
output are in [`docs/calibration/`](docs/calibration). With Ordo 1.2.6:

```sh
ordo -Q -D -s 2000 \
  -m docs/calibration/ccrl-blitz-anchors-2026-09-05.csv \
  -p docs/calibration/ccrl-blitz-gauntlet-2026-09-28.pgn
```

The "give or take" figure can change a little between runs, because Ordo
estimates it by simulation.

### On Lichess: 2222 blitz

This is the bot's Lichess blitz rating on 2026-10-03, from 1,257 rated games
(give or take about 90).

Lichess ratings are their own scale, built from games against people and
other bots. They are not CCRL ratings. The bot has also played several
versions of Cadence over that time, with settings a rating list would turn
off: it thinks on the opponent's time and uses an opening book. So this
number does not belong to any one version.

## How it is tested

Each change that is meant to make Cadence stronger is played against the
current version, usually for thousands of games. The test keeps going until
it is clear whether the change helps. The change is kept only if it wins. If
it loses, or the result stays unclear, it is thrown away. Six of the fourteen
changes tested between versions 0.4.0 and 0.4.8 were thrown away this way.

Each new patch version is one change that passed. The
[changelog](CHANGELOG.md) lists every version and its result.

Some checks you can see for yourself:

- The engine has a fixed benchmark: `cadence bench`. It always searches the
  same positions the same way, so its node count only changes when the
  engine's thinking changes. Every commit that changes the count records the
  new number, and CI checks it on two kinds of processor.
- Move generation is checked against a published set of positions with known
  answers. See [the perft corpus](docs/testing/perft.md).
- Since version 0.5.3, before the evaluation is retuned, the expected result
  is written down and saved first. That way the prediction cannot be bent to
  fit the result afterwards.

The match results and tuning records are kept, but they are private. So for
those, the changelog's figures are a report, not something you can check.
The benchmark, the perft corpus and the rating games are public.

## For developers

### Tests

```sh
cargo test --workspace
```

The slow tests, including the deep perft runs, are switched off by default.
Run them with:

```sh
cargo test --workspace --release -- --ignored
```

Before sending a change, run the same checks as CI. `xtask` is a separate
crate, so the workspace commands do not reach it, and it needs its own
three lines:

```sh
cargo fmt --all -- --check
cargo fmt --manifest-path xtask/Cargo.toml -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --manifest-path xtask/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path xtask/Cargo.toml
cargo xtask check-headers
cargo xtask check-boundary
cargo xtask check-changelog
```

`check-changelog` fails if a version tag, or the version in `Cargo.toml`, has
no entry in the changelog. So a new version needs its changelog entry in the
same change that sets the version.

`check-boundary` also checks the repository's writing rules. Prose and
comments use ASCII punctuation, apart from a short list of maths and Greek
symbols. Code and comments do not refer to numbered plan steps. The rules are
explained in `xtask/src/main.rs`.

### Benchmark and perft

```sh
cargo run --release --bin cadence -- bench
cargo run --release --bin cadence -- perft startpos 6
cargo run --release --bin cadence -- perft --divide "<fen>" <depth>
```

The last line of `bench` should match the number in `bench.txt`. A commit
that changes it must end with a `Bench: <n>` line giving the new count.

The [perft corpus](docs/testing/perft.md) explains where its positions come
from and how DFRC castling is written. The tests read the
[machine-readable copy](tests/fixtures/perft-corpus.txt), so everything
needed to reproduce a failure is public.

### Tools

Besides UCI, the binary has these subcommands. Each says what it expects
when it is given arguments it cannot use.

- `cadence bench`: the fixed benchmark above.
- `cadence perft [--divide] [--threads N] <fen | startpos> <depth>`: counts
  the legal moves to a depth.
- `cadence spsa`: prints the input an SPSA tuner reads, one line for each
  search constant a tune may move. They are the tuning options in the table
  above.
- `cadence datagen play --games N [--seed S] [--threads N] [--out PATH]`:
  plays games against itself and writes one record a game. The same seed and
  number of games give the same file at any thread count.
- `cadence datagen extract <games> --train PATH --holdout PATH [--every N]`:
  cuts those games into positions, sending every Nth game to the holdout set.
- `cadence texel <data> [--holdout N | --holdout-file PATH] [--iterations N]
  [--rate R] ...`: fits the evaluation's weights to a file of positions, each
  a FEN, a `|` and the result, and prints the table.

`cadence-tools magics [seed]`, a second binary built from `tools/`, searches
for the sliding-attack magic numbers and prints them as Rust. Nothing it does
ships.

### Git hooks

```sh
cargo xtask install-hooks
```

Run this once in each clone, because Git does not copy hooks. The
`pre-commit` hook runs the boundary and punctuation checks on what you are
committing. The `commit-msg` hook checks the message: the `Bench:` line
against `bench.txt`, no other trailer, no link, no numbered plan step, and a
subject of one line.

The hooks only make problems show up sooner. CI runs every check, the commit
messages included: it reads each message a change adds, and a pull request's
title as the merge message it becomes. So a clone without hooks is not less
correct. It just finds out later.

### Running an OpenBench worker

Changes are tested on [OpenBench](https://github.com/AndyGrant/OpenBench).
Cadence uses the official client at an exact, reviewed version, unmodified.
The versions a server must offer are listed in `openbench/pins.json`.

You need an account on an OpenBench server set up for Cadence. Use HTTPS if
the server is on the internet. Plain HTTP is only safe on a private network.

On Debian or Ubuntu, install the tools and Rust:

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  ca-certificates curl git build-essential python3 python3-venv

if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    sh -s -- -y --profile minimal
fi

. "$HOME/.cargo/env"
```

On macOS, install the Xcode command-line tools and Rust. On Windows, install
Git, Python, Rust and MSYS2, with Make and MinGW g++ on `PATH`, and use
`py -3` where the commands below say `python3`. The installer can start the
client on Windows, but Cadence tests stay switched off there until the
Windows build has been checked.

First, find how many games the machine can play at once. Run several copies
of one Cadence binary and raise the number until the speed of each copy
starts to vary a lot:

```sh
cargo xtask nps --binary PATH_TO_CADENCE --concurrency CANDIDATE_GAMES
```

Then install the worker with the number you found:

```sh
python3 openbench/setup-worker.py \
  --server OPENBENCH_URL \
  --username WORKER_ACCOUNT \
  --threads MEASURED_GAMES
```

The installer asks for the OpenBench password without showing it. If the
engine repository is private, it also asks for a read-only GitHub token. It
keeps these outside the repository, installs the pinned client and
`fastchess`, and sets the worker up to run in the background. Add `--dry-run`
to see what it would do, or `--no-start` to set it up without starting it.

To create a test:

1. Push the change on its own branch. Name the base by tag or full commit
   ID, never by a branch such as `main`, because a branch moves.
2. Run `cargo run --release --bin cadence -- bench` on both versions. Each
   count must match the bench value given to OpenBench.
3. On the reference worker, measure the base version at the number of games
   that worker plays at once:

   ```sh
   cargo xtask nps --binary PATH_TO_BASE_CADENCE --concurrency MEASURED_GAMES
   ```

   Use the steady figure from the later rounds, not an average that includes
   the first, slower ones.
4. In OpenBench, create the test with the branch as dev, the tag or commit as
   base, the measured speed as `scale_nps`, and the short or long time
   control preset. Check that dev and base are different commits before the
   workers start.

Workers build Cadence with the repository's `Makefile`, so `cargo`, `make`
and a C++ compiler must stay installed.

## License

Cadence is licensed under GPL-3.0-or-later. See `LICENSE`.
