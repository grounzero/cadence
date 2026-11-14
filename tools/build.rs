// SPDX-License-Identifier: GPL-3.0-or-later

//! The converter's own commit and whether its tree was modified, which its output manifest names.
//! A tree with no `.git` names neither.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from);
    let (commit, dirty) = match root.filter(|r| r.join(".git").exists()) {
        Some(root) => {
            watch(&root);
            let commit =
                git(&root, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
            let status = Command::new("git")
                .current_dir(&root)
                .args(["diff", "--quiet", "HEAD", "--"])
                .status()
                .ok()
                .and_then(|s| s.code());
            (commit, u8::from(status != Some(0)))
        }
        None => ("unknown".to_string(), 0),
    };
    println!("cargo::rustc-env=CADENCE_TOOLS_COMMIT_FULL={commit}");
    println!("cargo::rustc-env=CADENCE_TOOLS_DIRTY={dirty}");
}

/// HEAD, the refs and every top-level tracked path, so a commit or an edit reruns the script.
fn watch(root: &Path) {
    for (dir, name) in [
        ("--git-dir", "HEAD"),
        ("--git-common-dir", "refs"),
        ("--git-common-dir", "packed-refs"),
    ] {
        if let Some(dir) = git(root, &["rev-parse", dir]) {
            let path = root.join(dir).join(name);
            if path.exists() {
                println!("cargo::rerun-if-changed={}", path.display());
            }
        }
    }
    for name in git(root, &["ls-tree", "--name-only", "HEAD"])
        .unwrap_or_default()
        .lines()
    {
        println!("cargo::rerun-if-changed={}", root.join(name).display());
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}
