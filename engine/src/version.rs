// SPDX-License-Identifier: GPL-3.0-or-later

//! The version string the binary reports, composed at compile time.
//!
//! `build.rs` asks git what commit this is and whether HEAD is at an
//! annotated tag, and emits the two answers raw. The rule that turns them
//! into a name lives here, in the crate, because it is the part that can be
//! wrong in a way that matters and a build script has no tests.
//!
//! Two forms, and only two, where `<version>` is the package version:
//!
//! - `<version>` -- built at the annotated tag for this package version.
//! - `<version>-dev-<short commit>` -- anything else, carrying the commit it
//!   came from, or `<version>-dev-unknown` where there was no repository to
//!   ask.
//!
//! Written as a placeholder rather than spelled out: a patch is cut at every
//! promotion, so an example naming one version is an edit owed at each, and
//! the one thing a stale example here would misdescribe is the subject of
//! the file.
//!
//! **The bare form is reachable only through a tag that matches the package
//! version character for character.** Every way of not knowing arrives here
//! as an empty tag, and an empty string does not match a version, so no
//! failure inside `build.rs` can produce a build that reads as a release. The
//! guarantee is structural rather than careful: it does not depend on the
//! build script handling its errors correctly, only on it being unable to
//! invent a tag name.
//!
//! Nothing here reaches a decision path. The bench node count is a function
//! of the code alone, and a string constant that is printed once is not part
//! of it.

/// The tag HEAD is exactly at, or empty. Set by `build.rs`.
const TAG: &str = env!("CADENCE_TAG");

/// The package version, which is the whole workspace's version.
const PACKAGE: &str = env!("CARGO_PKG_VERSION");

/// What this build calls itself. The version half of the UCI `id name`.
pub const VERSION: &str = if is_release(TAG, PACKAGE) {
    PACKAGE
} else {
    concat!(env!("CARGO_PKG_VERSION"), "-dev-", env!("CADENCE_COMMIT"))
};

/// Whether `tag` releases `package`: exact equality and nothing looser.
///
/// Not a prefix test and not a "starts with the version" test. `0.3.0-rc1`
/// and `v0.3.0` are both tags a person might reasonably write and neither is
/// the `package` they are asked about, so neither releases it. The literals
/// here and in the test below are an example of the function and not of this
/// package, so they do not move when the version does.
const fn is_release(tag: &str, package: &str) -> bool {
    let (tag, package) = (tag.as_bytes(), package.as_bytes());
    if tag.is_empty() || tag.len() != package.len() {
        return false;
    }
    let mut i = 0;
    while i < tag.len() {
        if tag[i] != package[i] {
            return false;
        }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{PACKAGE, VERSION, is_release};

    /// The short commit, or `unknown`. `build.rs` sets it and never leaves it
    /// empty. It has no constant of its own outside these tests because the
    /// version string is built by `concat!`, which takes the `env!` directly.
    const COMMIT: &str = env!("CADENCE_COMMIT");

    /// The property the fallback rests on: nothing that is not the version
    /// itself grants the release form, and in particular nothing empty does.
    #[test]
    fn only_an_exact_tag_releases() {
        assert!(is_release("0.3.0", "0.3.0"));
        for tag in [
            "",
            "0.3",
            "0.3.0-rc1",
            "v0.3.0",
            "0.3.1",
            "0.30.0",
            " 0.3.0",
        ] {
            assert!(
                !is_release(tag, "0.3.0"),
                "`{tag}` must not read as a release"
            );
        }
    }

    /// Whichever form this build took, it is one of the two and it opens with
    /// the package version, so a reader who knows the version can always find
    /// it at the front.
    #[test]
    fn the_version_string_is_one_of_two_shapes() {
        assert!(
            VERSION.starts_with(PACKAGE),
            "{VERSION} does not open with {PACKAGE}"
        );
        let suffix = &VERSION[PACKAGE.len()..];
        assert!(
            suffix.is_empty() || suffix == format!("-dev-{COMMIT}"),
            "{VERSION} is neither the bare version nor a dev build of it"
        );
    }

    /// The commit is never empty, so the dev form can never trail off into
    /// `<version>-dev-`, which reads as a truncation rather than as a fact.
    #[test]
    fn the_commit_is_always_something() {
        assert!(!COMMIT.is_empty());
    }
}
