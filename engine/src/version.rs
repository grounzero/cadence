// SPDX-License-Identifier: GPL-3.0-or-later

//! `build.rs` emits the commit and the exact tag raw; the string is composed here, where it can be
//! tested.

/// Empty when HEAD is at no annotated tag.
const TAG: &str = env!("CADENCE_TAG");

/// The whole workspace's version.
const PACKAGE: &str = env!("CARGO_PKG_VERSION");

/// The version half of the UCI `id name`.
pub const VERSION: &str = if is_release(TAG, PACKAGE) {
    PACKAGE
} else {
    concat!(env!("CARGO_PKG_VERSION"), "-dev-", env!("CADENCE_COMMIT"))
};

/// Exact equality: not a prefix test.
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

    /// `build.rs` never leaves it empty.
    const COMMIT: &str = env!("CADENCE_COMMIT");

    /// Nothing but the version itself grants the release form, and in particular nothing empty
    /// does.
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

    /// Either form opens with the package version.
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

    /// So the dev form never trails off into `<version>-dev-`, which reads as a truncation.
    #[test]
    fn the_commit_is_always_something() {
        assert!(!COMMIT.is_empty());
    }
}
