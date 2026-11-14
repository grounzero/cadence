// SPDX-License-Identifier: GPL-3.0-or-later

//! `build.rs` emits the commit, the exact tag and the modified flag raw; the string is composed
//! here, where it can be tested.

/// Empty when HEAD is at no annotated tag naming the version.
const TAG: &str = env!("CADENCE_TAG");

/// The whole workspace's version.
const PACKAGE: &str = env!("CARGO_PKG_VERSION");

/// Seven hex digits of the commit built, or `unknown`.
pub const COMMIT: &str = env!("CADENCE_COMMIT");

/// All forty, or `unknown`.
pub const COMMIT_FULL: &str = env!("CADENCE_COMMIT_FULL");

/// `1` when a tracked file differed from the commit, staged or not, and `0` otherwise.
const DIRTY: &str = env!("CADENCE_DIRTY");

/// Whether the tree built differed from its commit.
pub const MODIFIED: bool = DIRTY.len() == 1 && DIRTY.as_bytes()[0] == b'1';

/// The triple the build was compiled for.
pub const TARGET: &str = env!("CADENCE_TARGET");

/// The `-C target-feature` flags it was given, comma-separated.
pub const TARGET_FEATURES: &str = env!("CADENCE_TARGET_FEATURES");

/// Every target feature those flags resolve to, as `cfg(target_feature)` lists them.
pub const CFG_FEATURES: &str = env!("CADENCE_CFG_FEATURES");

/// The version half of the UCI `id name`, the release only at the exact tag with a clean tree.
pub const VERSION: &str = if is_release(TAG, PACKAGE) && !MODIFIED {
    PACKAGE
} else if MODIFIED {
    concat!(
        env!("CARGO_PKG_VERSION"),
        "-dev-",
        env!("CADENCE_COMMIT"),
        "-dirty"
    )
} else {
    concat!(env!("CARGO_PKG_VERSION"), "-dev-", env!("CADENCE_COMMIT"))
};

/// What `VERSION` is composed as, here where each shape can be tested.
#[must_use]
pub fn version_of(package: &str, tag: &str, commit: &str, modified: bool) -> String {
    if is_release(tag, package) && !modified {
        return package.to_string();
    }
    let dirty = if modified { "-dirty" } else { "" };
    format!("{package}-dev-{commit}{dirty}")
}

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
    use super::{
        COMMIT, COMMIT_FULL, DIRTY, MODIFIED, PACKAGE, TAG, TARGET, VERSION, is_release, version_of,
    };

    /// The release only at the exact tag with a clean tree; a dev build names its commit, and a
    /// modified tree says so.
    #[test]
    fn the_version_takes_four_shapes() {
        assert_eq!(version_of("0.6.0", "0.6.0", "abc1234", false), "0.6.0");
        assert_eq!(
            version_of("0.6.0", "", "abc1234", false),
            "0.6.0-dev-abc1234"
        );
        assert_eq!(
            version_of("0.6.0", "0.6.0", "abc1234", true),
            "0.6.0-dev-abc1234-dirty"
        );
        assert_eq!(
            version_of("0.6.0", "", "abc1234", true),
            "0.6.0-dev-abc1234-dirty"
        );
        assert_eq!(
            version_of("0.6.0", "", "unknown", false),
            "0.6.0-dev-unknown"
        );
        assert_eq!(VERSION, version_of(PACKAGE, TAG, COMMIT, MODIFIED));
    }

    /// The full hash is what a dataset names, and the short one is its first seven digits.
    #[test]
    fn the_full_commit_is_forty_hex_digits_or_unknown() {
        if COMMIT_FULL == "unknown" {
            assert_eq!(COMMIT, "unknown");
        } else {
            assert_eq!(COMMIT_FULL.len(), 40, "{COMMIT_FULL}");
            assert!(
                COMMIT_FULL.bytes().all(|b| b.is_ascii_hexdigit()),
                "{COMMIT_FULL}"
            );
            assert_eq!(COMMIT, &COMMIT_FULL[..7]);
        }
    }

    #[test]
    fn the_modified_flag_is_zero_or_one_and_the_target_a_triple() {
        assert!(DIRTY == "0" || DIRTY == "1", "`{DIRTY}`");
        assert!(TARGET.split('-').count() >= 3, "`{TARGET}`");
    }

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
            "0.3.0-dirty",
        ] {
            assert!(
                !is_release(tag, "0.3.0"),
                "`{tag}` must not read as a release"
            );
        }
    }

    /// Every form opens with the package version.
    #[test]
    fn the_version_string_is_one_of_three_shapes() {
        assert!(
            VERSION.starts_with(PACKAGE),
            "{VERSION} does not open with {PACKAGE}"
        );
        let suffix = &VERSION[PACKAGE.len()..];
        assert!(
            suffix.is_empty()
                || suffix == format!("-dev-{COMMIT}")
                || suffix == format!("-dev-{COMMIT}-dirty"),
            "{VERSION} is neither the bare version nor a dev build of it"
        );
    }

    /// So the dev form never trails off into `<version>-dev-`, which reads as a truncation, and
    /// names a commit at one length however large the repository grows.
    #[test]
    fn the_commit_is_always_something_of_one_length() {
        assert_ne!(COMMIT, "");
        assert!(
            COMMIT == "unknown"
                || (COMMIT.len() == 7 && COMMIT.bytes().all(|b| b.is_ascii_hexdigit())),
            "{COMMIT}"
        );
    }
}
