// SPDX-License-Identifier: GPL-3.0-or-later

//! What commit this binary was built from, asked of git at build time.
//!
//! The engine reported its package version and nothing else, so every build
//! of every commit answered `id name Cadence 0.2.0`: an archived release
//! build, `main`, and a branch under test were one name in a GUI, in a PGN,
//! and in any log that records the identity a match was played under.
//!
//! This script asks git two questions and emits the answers as environment
//! variables. It does not decide what the version string looks like --
//! `version.rs` does, from these two values, because that decision is the
//! part with a failure mode and a build script cannot be unit-tested.
//!
//! - `CADENCE_COMMIT`: the short commit, or the literal `unknown`.
//! - `CADENCE_TAG`: the annotated tag HEAD is exactly at, or empty.
//!
//! **Both fall back toward "less released", never toward more.** There is no
//! source for the commit other than git, and the test server's worker builds
//! from a GitHub zipball that has no `.git` in it: it downloads
//! `api.github.com/.../zipball/<sha>`, unzips it, and runs `make` in the
//! result with nothing added to the environment. So `unknown` is not an edge
//! case, it is what every SPRT build reports. What matters is that an
//! unanswered question can only produce `unknown` and an empty tag, and
//! `version.rs` grants the release form only on a tag that matches the
//! package version exactly. An empty string never does.
//!
//! The tag question is asked without `--tags`, so it sees annotated tags
//! only. Versions here are annotated, carrying what the version marks, and a
//! lightweight tag left by hand is therefore not enough to make a build call
//! itself a release. The cost is that mistagging shows up as a build that
//! still says `-dev-`, which is the direction that is safe to be wrong in.
//!
//! There is no `-dirty` marker. It would need `git status` on every build,
//! and cargo's rerun tracking cannot see a working-tree edit that leaves the
//! refs alone, so the marker would be stale exactly when it was wanted.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // The default heuristic -- rerun when any file in the package changes --
    // is off the moment anything is emitted here, so the script's own source
    // is named explicitly along with the refs the answers depend on.
    println!("cargo::rerun-if-changed=build.rs");

    let facts = repo_root().map_or_else(Facts::unknown, |root| {
        watch_refs(&root);
        Facts::read(&root)
    });

    println!("cargo::rustc-env=CADENCE_COMMIT={}", facts.commit);
    println!("cargo::rustc-env=CADENCE_TAG={}", facts.tag);
}

/// What git had to say. `commit` is never empty; `tag` is empty when HEAD is
/// not at an annotated tag, and when there was nobody to ask.
struct Facts {
    commit: String,
    tag: String,
}

impl Facts {
    /// The answer when there is no repository to ask, no git to ask it with,
    /// or a question that failed. One function so that every route to "we do
    /// not know" produces the same pair.
    fn unknown() -> Self {
        Self {
            commit: "unknown".to_string(),
            tag: String::new(),
        }
    }

    fn read(root: &Path) -> Self {
        let commit = git(root, &["rev-parse", "--short", "HEAD"]);
        // The short commit is the same token the archived binaries are filed
        // under, so `Cadence <version>-dev-<short commit>` and the directory
        // holding that build are searchable by one string. That is worth more
        // here than matching the eight characters other engines print.
        let Some(commit) = commit else {
            return Self::unknown();
        };
        Self {
            // `--exact-match` and not `--tags`: annotated tags only.
            tag: git(root, &["describe", "--exact-match", "HEAD"]).unwrap_or_default(),
            commit,
        }
    }
}

/// The repository this package is in, or `None` if it is not in one.
///
/// The toplevel is compared against the workspace root rather than trusted.
/// git searches upward, so an extracted source tree sitting anywhere inside
/// some unrelated repository would otherwise be stamped with *that*
/// repository's commit -- a wrong hash, which is worse than `unknown`,
/// because `unknown` is visibly not an answer and a wrong hash is not.
fn repo_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .to_path_buf();
    if !root.join(".git").exists() {
        return None;
    }
    let toplevel = git(&root, &["rev-parse", "--show-toplevel"])?;
    let toplevel = std::fs::canonicalize(toplevel).ok()?;
    (toplevel == std::fs::canonicalize(&root).ok()?).then_some(root)
}

/// Rerun when the refs move: a new commit, a checkout, a tag being cut.
///
/// `refs/` covers branches and tags together and is a directory, which cargo
/// walks; `HEAD` covers switching between them; `packed-refs` covers the same
/// refs after `git gc` has folded them into one file. Each is named only if
/// it exists, because a rerun-if-changed on a path that does not exist reruns
/// the script on every build.
fn watch_refs(root: &Path) {
    for rel in [".git/HEAD", ".git/refs", ".git/packed-refs"] {
        let path = root.join(rel);
        if path.exists() {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
}

/// One git question, trimmed. `None` if git is missing, the command failed,
/// or the answer was empty -- all of which mean the same thing to the caller.
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}
