//! Emptying a directory that other programs share: the contents of a cache, temp or log
//! directory, optionally only what has not been touched for a while. The directory itself
//! always stays.
//!
//! Unlike removing an artifact, sweeping expects company: files can be in use, appear or
//! be replaced while the walk runs. So it never follows a symlink or reparse point, never
//! leaves the root's filesystem, skips what it cannot or should not delete (in use, cloud
//! placeholders, devices, sockets) and keeps going, accounting for every byte it freed or
//! left behind.
//!
//! On Unix the walk holds a handle on each directory and works relative to it (`openat`,
//! `fstatat`, `unlinkat` without following links), so nothing swapped in mid-walk is
//! followed. Windows has no handle-relative unlink in safe Rust; there the walk is
//! path-based and relies on never entering reparse points, which is sound for directories
//! the user owns. Callers should not sweep as root or an elevated administrator.

use crate::Cancel;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::Serialize;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Directories nested deeper than this are left alone (and reported).
const MAX_DEPTH: usize = 128;
/// Unexpected errors kept with their paths; the rest are only counted.
const MAX_FAILURES: usize = 20;

#[derive(Debug, Clone, Default)]
pub struct SweepOptions {
    /// Only remove entries not modified for at least this long.
    pub older_than: Option<Duration>,
    /// Names directly under the root that are never touched; `*`, `?` and `{a,b}` work.
    pub keep: Vec<String>,
    /// Report what would be removed without removing anything.
    pub dry_run: bool,
}

/// Why an entry was left in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Skip {
    /// Modified more recently than `older_than`.
    Recent,
    /// Listed in `keep`.
    Kept,
    /// Locked or busy (another program has it open).
    InUse,
    /// Permission denied.
    Denied,
    /// A cloud placeholder: deleting it could delete the cloud copy, or download it first.
    Cloud,
    /// On another filesystem (a mount point below the root).
    MountPoint,
    /// A device, socket or reparse point.
    Special,
    /// Replaced or refilled while the walk was looking at it.
    Changed,
    /// Nested deeper than the walk goes.
    TooDeep,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Tally {
    pub entries: u64,
    /// Allocated bytes, where known (directories count as 0).
    pub bytes: u64,
}

impl Tally {
    fn add(&mut self, bytes: u64) {
        self.entries += 1;
        self.bytes += bytes;
    }

    fn merge(&mut self, other: Tally) {
        self.entries += other.entries;
        self.bytes += other.bytes;
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SweepReport {
    /// Files and directories removed (or that would be, on a dry run).
    pub removed: Tally,
    /// What was left in place, by reason.
    pub skipped: BTreeMap<Skip, Tally>,
    /// Unexpected errors: counted in full, the first few kept with their paths.
    pub failed: Tally,
    pub failures: Vec<Failure>,
    /// The walk stopped early because it was cancelled.
    pub cancelled: bool,
}

impl SweepReport {
    /// Adds `other` to this report, as when sweeping several roots.
    pub fn merge(&mut self, other: SweepReport) {
        self.removed.merge(other.removed);
        for (reason, tally) in other.skipped {
            self.skipped.entry(reason).or_default().merge(tally);
        }
        self.failed.merge(other.failed);
        let room = MAX_FAILURES.saturating_sub(self.failures.len());
        self.failures.extend(other.failures.into_iter().take(room));
        self.cancelled |= other.cancelled;
    }

    /// Records that `root` could not be swept at all (missing roots are not recorded).
    pub fn root_failed(&mut self, root: &Path, error: &io::Error) {
        if error.kind() != io::ErrorKind::NotFound {
            self.error(root, error, 0);
        }
    }

    fn skip(&mut self, reason: Skip, bytes: u64) {
        self.skipped.entry(reason).or_default().add(bytes);
    }

    fn fail(&mut self, path: &Path, error: &io::Error, bytes: u64) {
        self.failed.add(bytes);
        if self.failures.len() < MAX_FAILURES {
            self.failures.push(Failure {
                path: path.to_path_buf(),
                error: error.to_string(),
            });
        }
    }

    /// Records an error that is expected while sweeping shared directories as a skip, and
    /// anything else as a failure.
    fn error(&mut self, path: &Path, error: &io::Error, bytes: u64) {
        match classify(error) {
            Some(reason) => self.skip(reason, bytes),
            None => self.fail(path, error, bytes),
        }
    }
}

fn classify(error: &io::Error) -> Option<Skip> {
    match error.kind() {
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            return Some(Skip::Denied);
        }
        io::ErrorKind::ResourceBusy | io::ErrorKind::ExecutableFileBusy => {
            return Some(Skip::InUse);
        }
        io::ErrorKind::DirectoryNotEmpty => return Some(Skip::Changed),
        _ => {}
    }
    if cfg!(windows) {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
        if matches!(error.raw_os_error(), Some(32 | 33)) {
            return Some(Skip::InUse);
        }
    }
    None
}

/// Removes the contents of `root`, which must be a directory and not a symlink, according
/// to `options`. Fails only when `root` itself cannot be opened; everything below it is
/// reported.
pub fn sweep(root: &Path, options: &SweepOptions, cancel: &Cancel) -> io::Result<SweepReport> {
    sweep_at(root, options, cancel, SystemTime::now())
}

/// [`sweep`] as if it were `now`. Tests use a later `now` to age entries, since nothing can
/// set a file's change time back.
pub(crate) fn sweep_at(
    root: &Path,
    options: &SweepOptions,
    cancel: &Cancel,
    now: SystemTime,
) -> io::Result<SweepReport> {
    let mut keep = GlobSetBuilder::new();
    for pattern in &options.keep {
        let glob = GlobBuilder::new(pattern)
            .case_insensitive(cfg!(windows))
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        keep.add(glob);
    }
    let rules = Rules {
        cutoff: options.older_than.map(|older_than| {
            let now = now.duration_since(UNIX_EPOCH).unwrap_or_default();
            now.saturating_sub(older_than).as_secs() as i64
        }),
        keep: keep
            .build()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?,
        dry_run: options.dry_run,
    };
    imp::sweep(root, &rules, cancel)
}

/// [`SweepOptions`] prepared for the walk.
struct Rules {
    /// Seconds since the Unix epoch before which entries count as old.
    cutoff: Option<i64>,
    keep: GlobSet,
    dry_run: bool,
}

impl Rules {
    /// Whether `name`, directly under the root, is kept.
    fn kept(&self, name: &OsStr) -> bool {
        self.keep.is_match(Path::new(name))
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use rustix::fs::{self, AtFlags, Dir, FileType, Mode, OFlags, Stat};
    use rustix::io::Errno;
    use std::ffi::CStr;
    use std::os::fd::OwnedFd;
    use std::os::unix::ffi::OsStrExt;

    const DIR_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::NOFOLLOW)
        .union(OFlags::CLOEXEC);

    /// macOS `SF_DATALESS`: an iCloud or File Provider file whose contents are not local.
    #[cfg(target_os = "macos")]
    const SF_DATALESS: u32 = 0x4000_0000;

    pub(super) fn sweep(root: &Path, rules: &Rules, cancel: &Cancel) -> io::Result<SweepReport> {
        let fd = fs::open(root, DIR_FLAGS, Mode::empty()).map_err(|e| match e {
            Errno::LOOP => io::Error::other(format!("{} is a symlink", root.display())),
            e => io::Error::from(e),
        })?;
        let stat = fs::fstat(&fd)?;
        let mut walk = Walk {
            dev: stat.st_dev,
            rules,
            cancel,
            path: root.to_path_buf(),
            report: SweepReport::default(),
        };
        walk.dir(&fd, 0);
        Ok(walk.report)
    }

    struct Walk<'a> {
        /// The root's filesystem; the walk never leaves it.
        dev: fs::Dev,
        rules: &'a Rules,
        cancel: &'a Cancel,
        /// The entry being looked at, for reports.
        path: PathBuf,
        report: SweepReport,
    }

    impl Walk<'_> {
        /// Sweeps the directory open as `fd`; true when everything in it was removed.
        fn dir(&mut self, fd: &OwnedFd, depth: usize) -> bool {
            let entries = match Dir::read_from(fd) {
                Ok(entries) => entries,
                Err(e) => {
                    self.report.error(&self.path, &e.into(), 0);
                    return false;
                }
            };
            let mut empty = true;
            for entry in entries {
                if self.cancel.is_cancelled() {
                    self.report.cancelled = true;
                    return false;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        self.report.error(&self.path, &e.into(), 0);
                        return false;
                    }
                };
                let name = entry.file_name();
                if matches!(name.to_bytes(), b"." | b"..") {
                    continue;
                }
                self.path.push(OsStr::from_bytes(name.to_bytes()));
                empty &= self.entry(fd, name, depth);
                self.path.pop();
            }
            empty
        }

        /// Handles one entry of `dir`; true when it was removed.
        fn entry(&mut self, dir: &OwnedFd, name: &CStr, depth: usize) -> bool {
            if depth == 0 && self.rules.kept(OsStr::from_bytes(name.to_bytes())) {
                self.report.skip(Skip::Kept, 0);
                return false;
            }
            let stat = match fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => stat,
                Err(Errno::NOENT) => return true,
                Err(e) => {
                    self.report.error(&self.path, &e.into(), 0);
                    return false;
                }
            };
            if stat.st_dev != self.dev {
                self.report.skip(Skip::MountPoint, 0);
                return false;
            }
            if dataless(&stat) {
                self.report.skip(Skip::Cloud, 0);
                return false;
            }
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::Directory => self.subdir(dir, name, &stat, depth),
                // Unlinking never opens the file, so named pipes are fine; a symlink is
                // removed itself, never followed.
                FileType::RegularFile | FileType::Symlink | FileType::Fifo => {
                    let bytes = allocated(&stat);
                    if self.recent(&stat, true) {
                        self.report.skip(Skip::Recent, bytes);
                        return false;
                    }
                    // Other hard links keep the data alive.
                    let freed = if stat.st_nlink <= 1 { bytes } else { 0 };
                    self.unlink(dir, name, AtFlags::empty(), freed)
                }
                // Sockets may be live; devices are never ours.
                _ => {
                    self.report.skip(Skip::Special, 0);
                    false
                }
            }
        }

        fn subdir(&mut self, dir: &OwnedFd, name: &CStr, stat: &Stat, depth: usize) -> bool {
            if depth + 1 >= MAX_DEPTH {
                self.report.skip(Skip::TooDeep, 0);
                return false;
            }
            let sub = match fs::openat(dir, name, DIR_FLAGS, Mode::empty()) {
                Ok(sub) => sub,
                Err(Errno::NOENT) => return true,
                // Replaced by a symlink or file since it was looked at.
                Err(Errno::LOOP | Errno::NOTDIR) => {
                    self.report.skip(Skip::Changed, 0);
                    return false;
                }
                Err(e) => {
                    self.report.error(&self.path, &e.into(), 0);
                    return false;
                }
            };
            match fs::fstat(&sub) {
                Ok(now) if now.st_ino == stat.st_ino && now.st_dev == stat.st_dev => {}
                _ => {
                    self.report.skip(Skip::Changed, 0);
                    return false;
                }
            }
            // Judged before the walk, which updates the directory's own times.
            let recent = self.recent(stat, false);
            let empty = self.dir(&sub, depth + 1);
            drop(sub);
            empty && !recent && self.unlink(dir, name, AtFlags::REMOVEDIR, 0)
        }

        fn recent(&self, stat: &Stat, file: bool) -> bool {
            let Some(cutoff) = self.rules.cutoff else {
                return false;
            };
            // A file's change time catches contents restored with an old mtime; a
            // directory's moves whenever its entries do, so it is not used.
            #[allow(
                clippy::useless_conversion,
                reason = "time_t is 32 bits on some targets"
            )]
            let mut newest = i64::from(stat.st_mtime);
            if file {
                #[allow(
                    clippy::useless_conversion,
                    reason = "time_t is 32 bits on some targets"
                )]
                let changed = i64::from(stat.st_ctime);
                newest = newest.max(changed);
            }
            newest > cutoff
        }

        fn unlink(&mut self, dir: &OwnedFd, name: &CStr, flags: AtFlags, bytes: u64) -> bool {
            if self.rules.dry_run {
                self.report.removed.add(bytes);
                return true;
            }
            match fs::unlinkat(dir, name, flags) {
                Ok(()) | Err(Errno::NOENT) => {
                    self.report.removed.add(bytes);
                    true
                }
                Err(e) => {
                    self.report.error(&self.path, &e.into(), bytes);
                    false
                }
            }
        }
    }

    fn allocated(stat: &Stat) -> u64 {
        (stat.st_blocks as u64).saturating_mul(512)
    }

    #[cfg(target_os = "macos")]
    fn dataless(stat: &Stat) -> bool {
        stat.st_flags & SF_DATALESS != 0
    }

    #[cfg(not(target_os = "macos"))]
    fn dataless(_: &Stat) -> bool {
        false
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::fs::{self, Metadata};
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    /// Offline, recall-on-open, pinned, unpinned and recall-on-data-access: OneDrive and
    /// other cloud placeholders. Deleting one can delete the cloud copy.
    const CLOUD: u32 = 0x1000 | 0x4_0000 | 0x8_0000 | 0x10_0000 | 0x40_0000;

    pub(super) fn sweep(root: &Path, rules: &Rules, cancel: &Cancel) -> io::Result<SweepReport> {
        let meta = fs::symlink_metadata(root)?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other(format!(
                "{} is a link or reparse point",
                root.display()
            )));
        }
        if !meta.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is not a directory", root.display()),
            ));
        }
        let mut walk = Walk {
            rules,
            cancel,
            report: SweepReport::default(),
        };
        walk.dir(root, 0);
        Ok(walk.report)
    }

    struct Walk<'a> {
        rules: &'a Rules,
        cancel: &'a Cancel,
        report: SweepReport,
    }

    impl Walk<'_> {
        fn dir(&mut self, dir: &Path, depth: usize) -> bool {
            let entries = match fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) => {
                    self.report.error(dir, &e, 0);
                    return false;
                }
            };
            let mut empty = true;
            for entry in entries {
                if self.cancel.is_cancelled() {
                    self.report.cancelled = true;
                    return false;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        self.report.error(dir, &e, 0);
                        return false;
                    }
                };
                empty &= self.entry(&entry, depth);
            }
            empty
        }

        fn entry(&mut self, entry: &fs::DirEntry, depth: usize) -> bool {
            let path = entry.path();
            if depth == 0 && self.rules.kept(&entry.file_name()) {
                self.report.skip(Skip::Kept, 0);
                return false;
            }
            // From the directory listing: never opens the entry, so never recalls a
            // placeholder, and never follows a link.
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
                Err(e) => {
                    self.report.error(&path, &e, 0);
                    return false;
                }
            };
            let attributes = meta.file_attributes();
            if attributes & CLOUD != 0 {
                self.report.skip(Skip::Cloud, 0);
                return false;
            }
            // Symlinks, junctions, mount points and other reparse points are never entered;
            // never entering them also keeps the walk on one volume.
            if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                self.report.skip(Skip::Special, 0);
                return false;
            }
            if meta.is_dir() {
                if depth + 1 >= MAX_DEPTH {
                    self.report.skip(Skip::TooDeep, 0);
                    return false;
                }
                let recent = self.recent(&meta);
                let empty = self.dir(&path, depth + 1);
                return empty && !recent && self.remove(&path, true, 0);
            }
            let bytes = meta.len();
            if self.recent(&meta) {
                self.report.skip(Skip::Recent, bytes);
                return false;
            }
            self.remove(&path, false, bytes)
        }

        fn recent(&self, meta: &Metadata) -> bool {
            let Some(cutoff) = self.rules.cutoff else {
                return false;
            };
            let secs = |t: io::Result<SystemTime>| {
                t.ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64)
            };
            secs(meta.modified()).max(secs(meta.created())) > cutoff
        }

        fn remove(&mut self, path: &Path, dir: bool, bytes: u64) -> bool {
            if self.rules.dry_run {
                self.report.removed.add(bytes);
                return true;
            }
            let result = if dir {
                fs::remove_dir(path)
            } else {
                fs::remove_file(path)
            };
            match result {
                Ok(()) => {
                    self.report.removed.add(bytes);
                    true
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    self.report.removed.add(bytes);
                    true
                }
                Err(e) => {
                    self.report.error(path, &e, bytes);
                    false
                }
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::*;

    pub(super) fn sweep(_: &Path, _: &Rules, _: &Cancel) -> io::Result<SweepReport> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "sweeping is not supported on this platform",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const DAY: u64 = 86_400;

    fn write(root: &Path, rel: &str, contents: &str) -> PathBuf {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    /// Sets `path`'s modification time `days` from now.
    fn touch_in(path: &Path, days: u64) {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + days * DAY;
        let time = filetime::FileTime::from_unix_time(secs as i64, 0);
        filetime::set_file_mtime(path, time).unwrap();
    }

    fn run(root: &Path, options: SweepOptions) -> SweepReport {
        sweep(root, &options, &Cancel::new()).unwrap()
    }

    fn names(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn empties_the_directory_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cache");
        write(&root, "a/b/c.bin", &"x".repeat(10_000));
        write(&root, "top.txt", "t");
        let report = run(&root, SweepOptions::default());
        assert!(root.is_dir());
        assert!(names(&root).is_empty());
        assert_eq!(report.removed.entries, 4, "{report:?}");
        assert!(report.removed.bytes >= 10_000);
        assert!(report.failures.is_empty());
    }

    #[test]
    fn age_filter_keeps_recent_entries() {
        // The sweep runs "30 days from now", so whatever was just created is 30 days old,
        // and entries touched 29 days ahead are a day old.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "old.log", "o");
        let new = write(root, "new.log", "n");
        // An old file in a recently changed directory goes; the directory stays.
        write(root, "busy/stale.log", "s");
        let fresh = write(root, "busy/fresh.log", "f");
        // An old directory holding only old files goes entirely.
        write(root, "dead/x.log", "x");
        touch_in(&new, 29);
        touch_in(&fresh, 29);
        touch_in(&root.join("busy"), 29);
        let later = SystemTime::now() + Duration::from_secs(30 * DAY);
        let options = SweepOptions {
            older_than: Some(Duration::from_secs(7 * DAY)),
            ..SweepOptions::default()
        };
        let report = sweep_at(root, &options, &Cancel::new(), later).unwrap();
        assert_eq!(names(root), ["busy", "new.log"]);
        assert_eq!(names(&root.join("busy")), ["fresh.log"]);
        assert_eq!(report.skipped[&Skip::Recent].entries, 2, "{report:?}");
    }

    #[test]
    fn keeps_named_entries_at_the_top() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "keep/data", "k");
        write(root, "mesa_shader_cache_db/x", "m");
        write(root, "other/keep", "o");
        let report = run(
            root,
            SweepOptions {
                keep: vec!["keep".into(), "mesa_shader_cache*".into()],
                ..SweepOptions::default()
            },
        );
        assert_eq!(names(root), ["keep", "mesa_shader_cache_db"]);
        assert_eq!(report.skipped[&Skip::Kept].entries, 2);
    }

    #[test]
    fn dry_run_removes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a/b.bin", &"x".repeat(5_000));
        let report = run(
            root,
            SweepOptions {
                dry_run: true,
                ..SweepOptions::default()
            },
        );
        assert_eq!(names(root), ["a"]);
        assert_eq!(report.removed.entries, 2);
        assert!(report.removed.bytes >= 5_000);
    }

    #[test]
    fn stops_when_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a", "a");
        let cancel = Cancel::new();
        cancel.cancel();
        let report = sweep(dir.path(), &SweepOptions::default(), &cancel).unwrap();
        assert!(report.cancelled);
        assert_eq!(names(dir.path()), ["a"]);
    }

    #[test]
    fn missing_root_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            sweep(
                &dir.path().join("nope"),
                &SweepOptions::default(),
                &Cancel::new()
            )
            .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        write(&outside, "precious", "keep");
        let root = dir.path().join("cache");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        run(&root, SweepOptions::default());
        assert!(names(&root).is_empty(), "the link itself goes");
        assert!(outside.join("precious").exists(), "its target stays");
        // A symlinked root is refused outright.
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        assert!(sweep(&linked, &SweepOptions::default(), &Cancel::new()).is_err());
        assert!(outside.join("precious").exists());
    }

    #[cfg(unix)]
    #[test]
    fn handles_special_and_unwritable_entries() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Unlinked without being opened: no hang on a pipe, no trouble with mode 000.
        let _ = std::process::Command::new("mkfifo")
            .arg(root.join("pipe"))
            .status();
        let unreadable = write(root, "unreadable", "x");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();
        // Sockets may be live: left alone.
        let _socket = std::os::unix::net::UnixListener::bind(root.join("sock")).unwrap();
        // A directory we may not write: its child is skipped, the walk goes on.
        write(root, "sealed/inner", "i");
        fs::set_permissions(root.join("sealed"), fs::Permissions::from_mode(0o555)).unwrap();
        write(root, "z-after", "z");
        let report = run(root, SweepOptions::default());
        fs::set_permissions(root.join("sealed"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(names(root), ["sealed", "sock"], "{report:?}");
        assert_eq!(report.skipped[&Skip::Special].entries, 1);
        assert_eq!(report.skipped[&Skip::Denied].entries, 1, "{report:?}");
        assert!(report.failures.is_empty(), "{report:?}");
    }
}
