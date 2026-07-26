//! The filesystem seam.
//!
//! Every path the IPC layer touches goes through [`Filesystem`], so the state
//! machine in [`crate::client`] and the [`crate::testing::FakeBridge`] can be
//! driven against an in-memory tree ([`MemFs`]) with no REAPER, no temp files
//! and no timing dependence — while production uses [`RealFs`], which is a thin
//! wrapper over `std::fs`.
//!
//! # The one invariant that matters
//!
//! [`Filesystem::write_sync`] must flush **and** `sync_all()` the file handle
//! before returning, and [`Filesystem::rename`] must be a same-directory
//! rename. That pair is what makes `<id>.tmp` → `<id>.command.json` atomic: a
//! reader either sees no `.command.json` at all, or sees a complete one.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The operations the IPC layer needs from a filesystem.
///
/// Implementations must be safe to share across threads: the MCP server holds
/// one client and may call it from a request thread while a heartbeat thread
/// reads liveness.
pub trait Filesystem: Send + Sync {
    /// Creates `path` and any missing parents. Succeeds if it already exists.
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;

    /// Writes `bytes` to `path`, then flushes and `sync_all()`s the handle.
    ///
    /// Truncates an existing file. The durability barrier is mandatory: the
    /// rename that follows is only atomic with respect to a reader if the
    /// content is on disk first.
    fn write_sync(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;

    /// Renames `from` to `to`. Must fail when `from` does not exist.
    ///
    /// This is the primitive both sides use to publish a file atomically and
    /// the primitive the bridge uses to *claim* a command: exactly one renamer
    /// wins.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;

    /// Reads the whole file.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Deletes a file. Errors with [`io::ErrorKind::NotFound`] when absent.
    fn remove_file(&self, path: &Path) -> io::Result<()>;

    /// True when `path` names an existing file.
    fn exists(&self, path: &Path) -> bool;

    /// Size of `path` in bytes.
    fn file_len(&self, path: &Path) -> io::Result<u64>;

    /// Immediate file names inside `path`, sorted. Directories are excluded.
    fn list_dir(&self, path: &Path) -> io::Result<Vec<String>>;
}

/// The production [`Filesystem`]: `std::fs` with an explicit `sync_all`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealFs;

impl Filesystem for RealFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn write_sync(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(path)?;
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn file_len(&self, path: &Path) -> io::Result<u64> {
        Ok(std::fs::metadata(path)?.len())
    }

    fn list_dir(&self, path: &Path) -> io::Result<Vec<String>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                out.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        out.sort();
        Ok(out)
    }
}

/// An in-memory [`Filesystem`] for tests.
///
/// Paths are normalised (`.` components removed, `..` resolved lexically) so a
/// path-escape attempt is observable rather than silently landing outside the
/// tree. Directories are tracked explicitly, so `list_dir` on a directory that
/// was never created is a `NotFound` just as it is on a real filesystem.
#[derive(Clone, Debug, Default)]
pub struct MemFs {
    inner: Arc<Mutex<MemState>>,
}

#[derive(Debug, Default)]
struct MemState {
    files: BTreeMap<PathBuf, Vec<u8>>,
    dirs: BTreeSet<PathBuf>,
    /// Number of `write_sync` calls that actually reached the durability
    /// barrier; asserted by the atomic-write tests.
    synced: u64,
}

impl MemFs {
    /// An empty tree.
    pub fn new() -> MemFs {
        MemFs::default()
    }

    /// How many durable writes have happened. Used to assert the write
    /// discipline without reaching for a real disk.
    pub fn sync_count(&self) -> u64 {
        self.inner.lock().expect("memfs lock").synced
    }

    /// Every file path currently present, sorted. Test-only introspection.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.inner
            .lock()
            .expect("memfs lock")
            .files
            .keys()
            .cloned()
            .collect()
    }

    /// Writes a file without the durability barrier, creating parents.
    ///
    /// Used by tests that need to plant a file (a heartbeat, a partial `.tmp`)
    /// without pretending it went through the protocol's write path.
    pub fn plant(&self, path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
        let path = normalize(path.as_ref());
        let mut st = self.inner.lock().expect("memfs lock");
        st.add_dirs(&path);
        st.files.insert(path, bytes.as_ref().to_vec());
    }
}

impl MemState {
    fn add_dirs(&mut self, file: &Path) {
        let mut cur = file.parent();
        while let Some(p) = cur {
            if p.as_os_str().is_empty() {
                break;
            }
            if !self.dirs.insert(p.to_path_buf()) {
                break;
            }
            cur = p.parent();
        }
    }
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("no such file: {}", path.display()),
    )
}

/// Lexically normalises a path: removes `.`, resolves `..` against the prefix.
///
/// This is deliberately lexical, not `canonicalize`: the point is to make an
/// escape attempt visible as a path outside the IPC root, not to follow
/// symlinks.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl Filesystem for MemFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        let path = normalize(path);
        let mut st = self.inner.lock().expect("memfs lock");
        let mut cur = PathBuf::new();
        for c in path.components() {
            cur.push(c.as_os_str());
            st.dirs.insert(cur.clone());
        }
        Ok(())
    }

    fn write_sync(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let path = normalize(path);
        let mut st = self.inner.lock().expect("memfs lock");
        match path.parent() {
            Some(p) if !p.as_os_str().is_empty() && !st.dirs.contains(p) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("parent directory does not exist: {}", p.display()),
                ));
            }
            _ => {}
        }
        st.files.insert(path, bytes.to_vec());
        st.synced += 1;
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let from = normalize(from);
        let to = normalize(to);
        let mut st = self.inner.lock().expect("memfs lock");
        let Some(bytes) = st.files.remove(&from) else {
            return Err(not_found(&from));
        };
        match to.parent() {
            Some(p) if !p.as_os_str().is_empty() && !st.dirs.contains(p) => {
                st.files.insert(from.clone(), bytes);
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("parent directory does not exist: {}", p.display()),
                ));
            }
            _ => {}
        }
        st.files.insert(to, bytes);
        Ok(())
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let path = normalize(path);
        let st = self.inner.lock().expect("memfs lock");
        st.files.get(&path).cloned().ok_or_else(|| not_found(&path))
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        let path = normalize(path);
        let mut st = self.inner.lock().expect("memfs lock");
        st.files
            .remove(&path)
            .map(|_| ())
            .ok_or_else(|| not_found(&path))
    }

    fn exists(&self, path: &Path) -> bool {
        let path = normalize(path);
        self.inner
            .lock()
            .expect("memfs lock")
            .files
            .contains_key(&path)
    }

    fn file_len(&self, path: &Path) -> io::Result<u64> {
        let path = normalize(path);
        let st = self.inner.lock().expect("memfs lock");
        st.files
            .get(&path)
            .map(|b| b.len() as u64)
            .ok_or_else(|| not_found(&path))
    }

    fn list_dir(&self, path: &Path) -> io::Result<Vec<String>> {
        let path = normalize(path);
        let st = self.inner.lock().expect("memfs lock");
        if !st.dirs.contains(&path) {
            return Err(not_found(&path));
        }
        let mut out = Vec::new();
        for p in st.files.keys() {
            if p.parent() == Some(path.as_path()) {
                if let Some(name) = p.file_name() {
                    out.push(name.to_string_lossy().into_owned());
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

/// A self-deleting temporary directory, for tests that must exercise
/// [`RealFs`] (fsync, real `rename` semantics) rather than [`MemFs`].
///
/// Names are unique per process and per call; the directory is removed
/// recursively on drop.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl TempDir {
    /// Creates a fresh temporary directory under the platform temp dir.
    pub fn new(tag: &str) -> io::Result<TempDir> {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("reaper-ipc-{tag}-{pid}-{stamp}-{n}"));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memfs_write_read_round_trip() {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/ipc/commands"))
            .expect("mkdir");
        fs.write_sync(Path::new("/ipc/commands/a.tmp"), b"hello")
            .expect("write");
        assert_eq!(
            fs.read(Path::new("/ipc/commands/a.tmp")).expect("read"),
            b"hello"
        );
        assert_eq!(
            fs.file_len(Path::new("/ipc/commands/a.tmp")).expect("len"),
            5
        );
        assert_eq!(fs.sync_count(), 1);
    }

    #[test]
    fn memfs_write_requires_an_existing_parent() {
        let fs = MemFs::new();
        let err = fs
            .write_sync(Path::new("/ipc/commands/a.tmp"), b"x")
            .expect_err("no parent");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn memfs_rename_is_a_move_and_fails_when_the_source_is_gone() {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/d")).expect("mkdir");
        fs.write_sync(Path::new("/d/a"), b"x").expect("write");
        fs.rename(Path::new("/d/a"), Path::new("/d/b"))
            .expect("rename");
        assert!(!fs.exists(Path::new("/d/a")));
        assert!(fs.exists(Path::new("/d/b")));
        let err = fs
            .rename(Path::new("/d/a"), Path::new("/d/c"))
            .expect_err("gone");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn memfs_rename_is_a_single_winner() {
        // Two claimers race for one file; exactly one rename can succeed.
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/c")).expect("mkdir");
        fs.create_dir_all(Path::new("/p")).expect("mkdir");
        fs.write_sync(Path::new("/c/x.command.json"), b"{}")
            .expect("write");
        let first = fs.rename(
            Path::new("/c/x.command.json"),
            Path::new("/p/x.processing.json"),
        );
        let second = fs.rename(
            Path::new("/c/x.command.json"),
            Path::new("/p/x.processing.json"),
        );
        assert!(first.is_ok());
        assert!(second.is_err());
    }

    #[test]
    fn memfs_list_dir_is_sorted_and_shallow() {
        let fs = MemFs::new();
        fs.create_dir_all(Path::new("/d/sub")).expect("mkdir");
        fs.write_sync(Path::new("/d/b"), b"").expect("w");
        fs.write_sync(Path::new("/d/a"), b"").expect("w");
        fs.write_sync(Path::new("/d/sub/c"), b"").expect("w");
        assert_eq!(fs.list_dir(Path::new("/d")).expect("ls"), vec!["a", "b"]);
    }

    #[test]
    fn memfs_list_dir_on_a_missing_directory_is_not_found() {
        let fs = MemFs::new();
        let err = fs.list_dir(Path::new("/nope")).expect_err("missing");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn memfs_remove_reports_missing_files() {
        let fs = MemFs::new();
        assert_eq!(
            fs.remove_file(Path::new("/gone"))
                .expect_err("missing")
                .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn normalize_resolves_dot_and_dotdot_lexically() {
        assert_eq!(normalize(Path::new("/a/./b")), PathBuf::from("/a/b"));
        assert_eq!(normalize(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(
            normalize(Path::new("/ipc/commands/../../etc/passwd")),
            PathBuf::from("/etc/passwd")
        );
    }

    #[test]
    fn realfs_write_rename_read_delete_round_trip() {
        let dir = TempDir::new("realfs").expect("temp dir");
        let fs = RealFs;
        let tmp = dir.path().join("a.tmp");
        let final_path = dir.path().join("a.command.json");
        fs.write_sync(&tmp, b"{\"a\":1}").expect("write");
        fs.rename(&tmp, &final_path).expect("rename");
        assert!(!fs.exists(&tmp));
        assert_eq!(fs.read(&final_path).expect("read"), b"{\"a\":1}");
        assert_eq!(fs.list_dir(dir.path()).expect("ls"), vec!["a.command.json"]);
        fs.remove_file(&final_path).expect("remove");
        assert!(fs.list_dir(dir.path()).expect("ls").is_empty());
    }

    #[test]
    fn temp_dirs_are_unique_and_removed_on_drop() {
        let a = TempDir::new("uniq").expect("a");
        let b = TempDir::new("uniq").expect("b");
        assert_ne!(a.path(), b.path());
        let path = a.path().to_path_buf();
        drop(a);
        assert!(!path.exists());
    }
}
