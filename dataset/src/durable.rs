// SPDX-License-Identifier: GPL-3.0-or-later

//! A rename that publishes a file, then a sync of the directory that holds it: a file's own sync
//! makes its contents durable and not its name.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// Where the directory sync is skipped, the "not examined" line that says so.
pub const UNSYNCED: Option<&str> = if cfg!(unix) {
    None
} else {
    Some(
        "each published name's durability after a crash (Windows cannot open a directory to sync it)",
    )
};

/// The two steps a publish takes, so a test can record the order they are taken in.
pub trait Steps {
    /// # Errors
    ///
    /// The rename's.
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()>;

    /// # Errors
    ///
    /// Opening or syncing the directory.
    fn sync_dir(&mut self, dir: &Path) -> io::Result<()>;
}

/// The file system itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct Disk;

impl Steps for Disk {
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        sync_dir(dir)
    }
}

/// # Errors
///
/// Opening or syncing the directory, on Unix.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    if cfg!(unix) {
        File::open(dir)?.sync_all()
    } else {
        // Windows cannot open a directory to sync it, so the sync is skipped and the tools say so.
        Ok(())
    }
}

/// Renamed into place, then the directory holding `to` synced.
///
/// # Errors
///
/// The rename's, or the sync's once the rename is made.
pub fn publish(steps: &mut dyn Steps, from: &Path, to: &Path) -> io::Result<()> {
    steps.rename(from, to)?;
    steps.sync_dir(parent(to))
}

/// The directory a path's entry is in, `.` for a bare name.
#[must_use]
pub fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

/// One step, by the path it acted on: a rename's destination, or the directory synced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Rename(PathBuf),
    Sync(PathBuf),
}

/// Each step made on the file system and recorded in order, with one rename failing where a test
/// chooses.
#[derive(Debug, Default)]
pub struct Logged {
    pub steps: Vec<Step>,
    fail_rename: Option<usize>,
    renames: usize,
}

impl Logged {
    /// Rename `n`, counted from 0, fails and is not made.
    #[must_use]
    pub fn failing_at(n: usize) -> Logged {
        Logged {
            fail_rename: Some(n),
            ..Logged::default()
        }
    }
}

impl Steps for Logged {
    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        let n = self.renames;
        self.renames += 1;
        if self.fail_rename == Some(n) {
            return Err(io::Error::other("injected"));
        }
        std::fs::rename(from, to)?;
        self.steps.push(Step::Rename(to.to_path_buf()));
        Ok(())
    }

    fn sync_dir(&mut self, dir: &Path) -> io::Result<()> {
        sync_dir(dir)?;
        self.steps.push(Step::Sync(dir.to_path_buf()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Logged, Step, UNSYNCED, parent, publish, sync_dir};
    use std::path::Path;

    #[test]
    fn a_publish_syncs_the_directory_after_its_rename() {
        let dir = std::env::temp_dir().join(format!("cadence-durable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let (from, to) = (dir.join("a.partial"), dir.join("a"));
        std::fs::write(&from, "a").expect("writes");
        let mut logged = Logged::default();
        let published = publish(&mut logged, &from, &to);
        let landed = std::fs::read_to_string(&to);
        let _ = std::fs::remove_dir_all(&dir);
        published.expect("publishes");
        assert_eq!(landed.expect("lands"), "a");
        assert_eq!(logged.steps, [Step::Rename(to), Step::Sync(dir)]);
    }

    /// Unix opens the directory to sync it, so a missing one fails there; elsewhere nothing is
    /// opened and the skip is said.
    #[test]
    fn the_directory_is_opened_on_unix_and_the_skip_said_elsewhere() {
        let missing = std::env::temp_dir().join(format!("cadence-no-dir-{}", std::process::id()));
        assert_eq!(sync_dir(&missing).is_err(), cfg!(unix));
        assert_eq!(UNSYNCED.is_some(), cfg!(not(unix)));
        assert_eq!(parent(Path::new("a")), Path::new("."));
    }
}
