# SPDX-License-Identifier: GPL-3.0-or-later
#
# OpenBench build shim. Not the build system: that is Cargo, and this file
# knows nothing the workspace does not. It exists because the OpenBench
# worker builds every engine the same way -- `make -j EXE=<name>` in the
# configured build path, after which the binary must exist at `./<name>` --
# and Cargo puts it under target/. The two lines below close that gap and
# nothing else.
#
# The release profile (Cargo.toml: fat LTO, one codegen unit, incremental
# off) is the one the bench contract is measured under, and it is the one
# the worker gets, unmodified. There is deliberately no RUSTFLAGS and no
# `-C target-cpu=native` here, which is where this file parts company with
# the shims the other Rust engines on OpenBench ship. Those build for the
# host CPU, and for them that is free Elo. For Cadence it would break the
# bench determinism contract: the node count must be a
# function of the code alone, and it is checked for agreement on every
# machine that runs it, dev against base, across the fleet. A binary whose
# code generation depends on the host it was compiled on is the kind of
# non-reproducibility that contract exists to exclude -- it does not
# reproduce under investigation -- so every worker builds the same portable
# binary, and any CPU-specific dispatch the engine ever wants is done at
# run time, by the engine, where it can be tested.
#
# `cargo` itself (rustup's proxy, honouring rust-toolchain.toml) must be on
# the worker's PATH. For a private engine the server does not check that a
# worker has the compiler before handing it the job, so a worker without
# cargo does not decline the work: it crashes when it reaches this file.
# The worker's service definition puts ~/.cargo/bin on PATH for that reason.

EXE ?= cadence

ifeq ($(OS),Windows_NT)
    EXT := .exe
else
    EXT :=
endif

.PHONY: all clean

all:
	cargo build --release -p cadence-engine
	cp target/release/cadence$(EXT) $(EXE)$(EXT)

clean:
	cargo clean
	rm -f $(EXE)$(EXT)
