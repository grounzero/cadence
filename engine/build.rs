// SPDX-License-Identifier: GPL-3.0-or-later

//! The answers are emitted raw and `version.rs` composes the string. Both fall back toward less
//! released: a build from a zipball with no `.git`, which is every SPRT build, reports `unknown`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // Emitting anything turns off the default rerun heuristic, so the script and the refs are named
    // explicitly.
    println!("cargo::rerun-if-changed=build.rs");

    let facts = repo_root().map_or_else(Facts::unknown, |root| {
        watch_refs(&root);
        Facts::read(&root)
    });

    println!("cargo::rustc-env=CADENCE_COMMIT={}", facts.commit);
    println!("cargo::rustc-env=CADENCE_TAG={}", facts.tag);
}

/// `commit` is never empty; `tag` is empty when HEAD is not at an annotated tag or nobody could be
/// asked.
struct Facts {
    commit: String,
    tag: String,
}

impl Facts {
    /// Every route to not knowing produces the same pair.
    fn unknown() -> Self {
        Self {
            commit: "unknown".to_string(),
            tag: String::new(),
        }
    }

    fn read(root: &Path) -> Self {
        let commit = git(root, &["rev-parse", "--short", "HEAD"]);
        // The archived binaries are filed under the short commit, so a build and its archive share
        // one searchable string.
        let Some(commit) = commit else {
            return Self::unknown();
        };
        Self {
            // `--exact-match` without `--tags`: annotated tags only, so a lightweight tag left by
            // hand cannot make a release.
            tag: git(root, &["describe", "--exact-match", "HEAD"]).unwrap_or_default(),
            commit,
        }
    }
}

/// Checked against the workspace root: git searches upward, and a tree extracted inside an
/// unrelated repository would be stamped with a wrong hash, which is worse than `unknown`.
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

/// A linked worktree's `.git` is a file, so HEAD is resolved through `--git-dir` and the shared
/// refs and `packed-refs` through `--git-common-dir`. Each path is named only if it exists, because
/// a missing one reruns the script on every build.
fn watch_refs(root: &Path) {
    let own = git(root, &["rev-parse", "--git-dir"]);
    let common = git(root, &["rev-parse", "--git-common-dir"]);
    for (dir, name) in [(&own, "HEAD"), (&common, "refs"), (&common, "packed-refs")] {
        let Some(dir) = dir else { continue };
        // Either answer may be relative to the root, and joining an absolute one replaces it.
        let path = root.join(dir).join(name);
        if path.exists() {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }
}

/// `None` if git is missing, failed or answered empty: all one thing to the caller.
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
