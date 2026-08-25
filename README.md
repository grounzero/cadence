# Cadence

Cadence is a UCI chess engine written in Rust, with standard chess and
Chess960/DFRC support. It runs as a command-line engine and can be loaded by
any chess GUI or match runner that supports UCI.

## Requirements

Install Git and Rust through `rustup`. The repository pins Rust 1.97.1 and
includes the required `rustfmt` and `clippy` components in
`rust-toolchain.toml`.

## Build

```sh
git clone https://github.com/grounzero/cadence.git
cd cadence
cargo build --release
```

The engine binary is written to `target/release/cadence`.

## Run

Start an interactive UCI session:

```sh
cargo run --release --bin cadence
```

A minimal session looks like this:

```text
uci
isready
position startpos
go depth 8
quit
```

To use Cadence in a GUI, select `target/release/cadence` as a UCI engine.
For Chess960 games, enable the GUI's Chess960 mode; it will set the
`UCI_Chess960` option.

Cadence also provides command-line perft and bench modes:

```sh
cargo run --release --bin cadence -- perft startpos 6
cargo run --release --bin cadence -- perft --divide "<fen>" <depth>
cargo run --release --bin cadence -- bench
```

The last line of `bench` should match the node count in `bench.txt`.

The published [perft corpus](docs/testing/perft.md) explains the positions,
their provenance and the DFRC castling convention.
Every claim is independently verifiable: tests read the extracted
[machine fixture](tests/fixtures/perft-corpus.txt), so every expected FEN,
move list and node count needed to reproduce a failure is public.

## Test

The normal workspace suite needs no setup beyond the pinned Rust toolchain:

```sh
cargo test --workspace
```

Long-running acceptance and deep perft tests are marked ignored:

```sh
cargo test --workspace --release -- --ignored
```

Before submitting a change, run the same formatting, lint and repository
checks used during development. `xtask` is deliberately excluded from the
workspace, so `--all` and `--workspace` do not reach it; its three legs below
are separate commands, not redundant ones, and CI runs every command in this
block:

```sh
cargo fmt --all -- --check
cargo fmt --manifest-path xtask/Cargo.toml -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --manifest-path xtask/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path xtask/Cargo.toml
cargo xtask check-headers
cargo xtask check-boundary
```

`check-boundary` also enforces the repository's punctuation and vocabulary
conventions: ASCII punctuation outside an exhaustive list of mathematical and
Greek notation, and no numbered planning labels in code, comments or test
names. Both rules, and what each cannot catch, are documented in
`xtask/src/main.rs`.

## Install the hooks

```sh
cargo xtask install-hooks
```

Run this once per clone. Git does not clone hooks, and the command only points
`core.hooksPath` at `.githooks/`, so a fresh checkout has none of them until
somebody runs it. What is skipped by skipping it:

- **`pre-commit`** runs the boundary, punctuation and vocabulary checks over
  the content being committed, reading the index rather than the working tree.
  Without it those rules are first checked in CI, on a branch that is already
  pushed.
- **`commit-msg`** enforces the `Bench: <n>` trailer against `bench.txt`, and
  refuses a numbered planning label in the message. Without it a commit can
  change the node count and declare nothing. CI still catches a count that
  disagrees with `bench.txt`, because it runs the bench and diffs it; it cannot
  catch a missing or wrong trailer, and it cannot see the message at all.

So the honest summary is that a clone without hooks is not less correct, it is
slower to find out: everything above except the trailer is also a CI step, and
CI is the gate. The hooks are the fast half. They are also a courtesy rather
than a guarantee, which is why nothing in this repository is designed on the
assumption that they ran.

## Test with OpenBench

Search and evaluation changes are match-tested with SPRT after the local suite
passes. Cadence uses an exact, reviewed revision of the official
[OpenBench](https://github.com/AndyGrant/OpenBench) client. The client is not
vendored or modified, and no OpenBench fork is required on a worker.

Running a worker requires an account on an OpenBench server configured for
Cadence. The server does not have to be on a particular machine, but it must
advertise the reviewed client and fastchess revisions recorded in
`openbench/pins.json`. Use HTTPS for an Internet-facing server; plain HTTP is
only suitable on a trusted LAN or VPN.

On Debian or Ubuntu, install the host tools and Rust:

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

On macOS, install the Xcode command-line tools and Rust through `rustup`. On
Windows, install Git, Python, Rust, and MSYS2 with Make and MinGW g++ on
`PATH`. The installer can launch the official client on Windows, but Cadence
workloads remain disabled there until the engine build has been verified.
Use `py -3 openbench/setup-worker.py` instead of `python3` in the command
below when that is how Python is installed.

First measure how many games the machine can sustain. Run concurrent copies
of one Cadence bench binary and retain the highest concurrency before the
per-copy nps spread jumps:

```sh
cargo xtask nps --binary PATH_TO_CADENCE --concurrency CANDIDATE_GAMES
```

The mean remains a useful run summary. For a test's `scale_nps`, use the
flat warm tail rather than a mean that mixes cold and warm pairs. Per-copy
minimum, maximum and spread answer the separate contention question. Supply
the measured concurrency to the installer rather than starting at one and
working up during live games:

```sh
python3 openbench/setup-worker.py \
  --server OPENBENCH_URL \
  --username WORKER_ACCOUNT \
  --threads MEASURED_GAMES
```

The installer prompts without echo for the OpenBench password and, while
Cadence is private, a fine-grained GitHub token with read access to Cadence.
It stores credentials outside both repositories, installs the pinned official
client in the platform data directory, builds the server-named fastchess
revision with bounded parallelism, and installs the platform user launcher.
Use `--dry-run` to inspect paths without writing, downloading, or starting
anything, or `--no-start` to configure the launcher without starting it.

When creating a workload:

1. Push the candidate on its own branch. Use a tag or full commit ID for the
   base; do not use a moving branch such as `main`.
2. Run `cargo run --release --bin cadence -- bench` for both revisions. The
   node counts must be reproducible, and each must match its declared OpenBench
   bench value.
3. On the reference worker, measure the base binary at the concurrency that
   worker will use:

   ```sh
   cargo xtask nps --binary PATH_TO_BASE_CADENCE --concurrency MEASURED_GAMES
   ```

4. In OpenBench, create a Cadence test with the candidate branch as dev, the
   fixed tag or commit ID as base, the measured `scale_nps`, and the appropriate
   STC or LTC preset. Confirm that dev and base resolve to different commits
   before starting the workers.

Workers build Cadence through the repository's `Makefile`, so `cargo`, `make`,
and a C++ compiler must remain available to the launcher.

## License

Cadence is licensed under GPL-3.0-or-later. See `LICENSE`.
