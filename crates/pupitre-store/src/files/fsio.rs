// SPDX-License-Identifier: MIT OR Apache-2.0
//! The file layer of the backend `files` (2.32). Each access is relative to an open folder
//! and uses `O_NOFOLLOW`, so a symbolic link is an error. A write makes a new file in the
//! folder `.tmp`, calls `fsync` on it, renames it, and calls `fsync` on the target folder.
//! This module is the only place that makes, writes, renames or removes files and folders.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use rustix::fd::OwnedFd;
use rustix::fs::{self, AtFlags, FileType, FlockOperation, Mode, OFlags, Stat};
use rustix::io::Errno;

use super::faults::{FaultPoint, Faults};
use crate::types::{FileSig, StoreError};

/// The modes of the files and folders that a store makes in one of its three folders (2.32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Modes {
    /// The mode of a file.
    pub file: u32,
    /// The mode of a folder.
    pub dir: u32,
}

impl Modes {
    /// 0644 and 0755: the documents folder.
    pub(crate) const DOCUMENTS: Modes = Modes { file: 0o644, dir: 0o755 };
    /// 0600 and 0700: the secrets folder.
    pub(crate) const SECRETS: Modes = Modes { file: 0o600, dir: 0o700 };
    /// 0640 and 0750: the runs folder.
    pub(crate) const RUNS: Modes = Modes { file: 0o640, dir: 0o750 };
}

/// An error of the file layer: the error number and the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FsErr {
    pub(crate) errno: Errno,
    pub(crate) path: String,
}

impl FsErr {
    pub(crate) fn not_found(&self) -> bool {
        self.errno == Errno::NOENT
    }
}

impl From<FsErr> for StoreError {
    fn from(e: FsErr) -> StoreError {
        match e.errno {
            Errno::LOOP | Errno::NOTDIR => {
                StoreError::Corrupt(format!("{}: a symbolic link, or a file in place of a folder", e.path))
            }
            other => StoreError::Unavailable(format!("{}: {}", e.path, std::io::Error::from(other))),
        }
    }
}

/// The type of a folder entry, read without a follow of links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FType {
    Dir,
    File,
    Symlink,
    Other,
}

impl FType {
    fn of_mode(mode: u32) -> FType {
        match FileType::from_raw_mode(mode) {
            FileType::RegularFile => FType::File,
            FileType::Directory => FType::Dir,
            FileType::Symlink => FType::Symlink,
            _ => FType::Other,
        }
    }

    /// A short text for the reason of an ignored entry.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            FType::Dir => "a folder",
            FType::File => "a file",
            FType::Symlink => "a symbolic link",
            FType::Other => "a special file",
        }
    }
}

/// One entry of a folder listing.
#[derive(Debug, Clone)]
pub(crate) struct DirItem {
    /// The name. A name that is not UTF-8 is converted with loss, and `name_ok` is false.
    pub(crate) name: String,
    pub(crate) name_ok: bool,
    pub(crate) ftype: FType,
}

/// The attributes of one entry.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FileStat {
    pub(crate) sig: FileSig,
    pub(crate) mode: u32,
    pub(crate) ftype: FType,
}

fn to_u64<T: TryInto<u64>>(v: T) -> u64 {
    v.try_into().unwrap_or(0)
}

impl FileStat {
    pub(crate) fn of(st: &Stat) -> FileStat {
        let mtime_ns = i128::from(st.st_mtime) * 1_000_000_000 + i128::from(st.st_mtime_nsec);
        let ctime_ns = i128::from(st.st_ctime) * 1_000_000_000 + i128::from(st.st_ctime_nsec);
        FileStat {
            sig: FileSig {
                dev: to_u64(st.st_dev),
                ino: to_u64(st.st_ino),
                size: to_u64(st.st_size),
                mtime_ns,
                ctime_ns,
            },
            mode: st.st_mode & 0o7777,
            ftype: FType::of_mode(st.st_mode),
        }
    }
}

/// An open folder. A clone shares the descriptor.
#[derive(Debug, Clone)]
pub(crate) struct Dir {
    fd: Arc<OwnedFd>,
    label: Arc<str>,
    faults: Faults,
    modes: Modes,
}

fn dir_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

fn temp_name() -> Result<String, Errno> {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).map_err(|_| Errno::IO)?;
    Ok(format!("{:016x}.tmp", u64::from_be_bytes(b)))
}

impl Dir {
    /// Open the root of a folder with `O_DIRECTORY | O_NOFOLLOW`.
    pub(crate) fn open_root(path: &Path, faults: Faults, modes: Modes) -> Result<Dir, FsErr> {
        let label = path.display().to_string();
        let fd = fs::open(path, dir_flags(), Mode::empty()).map_err(|errno| FsErr { errno, path: label.clone() })?;
        Ok(Dir { fd: Arc::new(fd), label: label.into(), faults, modes })
    }

    /// Open the root of a folder. A folder that does not exist is made, with the mode of the
    /// folder, when its parent exists.
    pub(crate) fn open_or_create_root(path: &Path, faults: Faults, modes: Modes) -> Result<Dir, FsErr> {
        match Dir::open_root(path, faults.clone(), modes) {
            Err(e) if e.not_found() => {}
            other => return other,
        }
        let (Some(parent), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else {
            return Dir::open_root(path, faults, modes);
        };
        Dir::open_root(parent, faults, modes)?.open_dir_create(name)
    }

    /// The text of the path for a message.
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    /// Pass a fault point.
    pub(crate) fn fault(&self, point: FaultPoint) -> Result<(), FsErr> {
        self.faults.check(point).map_err(|e| self.err("", e))
    }

    fn err(&self, name: &str, errno: Errno) -> FsErr {
        let path = if name.is_empty() { self.label.to_string() } else { format!("{}/{name}", self.label) };
        FsErr { errno, path }
    }

    fn child(&self, name: &str, fd: OwnedFd) -> Dir {
        Dir {
            fd: Arc::new(fd),
            label: format!("{}/{name}", self.label).into(),
            faults: self.faults.clone(),
            modes: self.modes,
        }
    }

    /// Open a sub-folder. A link gives an error.
    pub(crate) fn open_dir(&self, name: &str) -> Result<Dir, FsErr> {
        let fd = fs::openat(&*self.fd, name, dir_flags(), Mode::empty()).map_err(|e| self.err(name, e))?;
        Ok(self.child(name, fd))
    }

    /// Open a sub-folder, or give `None` when it does not exist.
    pub(crate) fn open_dir_opt(&self, name: &str) -> Result<Option<Dir>, FsErr> {
        match self.open_dir(name) {
            Ok(d) => Ok(Some(d)),
            Err(e) if e.not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Open a sub-folder, or make it with the mode of the folder.
    pub(crate) fn open_dir_create(&self, name: &str) -> Result<Dir, FsErr> {
        match self.open_dir(name) {
            Err(e) if e.not_found() => {}
            other => return other,
        }
        match fs::mkdirat(&*self.fd, name, Mode::from_raw_mode(0o700)) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(e) => return Err(self.err(name, e)),
        }
        let dir = self.open_dir(name)?;
        fs::fchmod(&*dir.fd, Mode::from_raw_mode(self.modes.dir)).map_err(|e| self.err(name, e))?;
        self.fsync()?;
        Ok(dir)
    }

    /// Open the folder at a relative path. With `create`, make each missing folder. Without
    /// it, a missing folder gives `None`.
    pub(crate) fn open_path(&self, segments: &[&str], create: bool) -> Result<Option<Dir>, FsErr> {
        let mut dir = self.clone();
        for segment in segments {
            dir = if create {
                dir.open_dir_create(segment)?
            } else {
                match dir.open_dir(segment) {
                    Ok(d) => d,
                    Err(e) if e.not_found() => return Ok(None),
                    Err(e) => return Err(e),
                }
            };
        }
        Ok(Some(dir))
    }

    /// The attributes of the folder itself.
    pub(crate) fn stat_self(&self) -> Result<FileStat, FsErr> {
        fs::fstat(&*self.fd).map(|st| FileStat::of(&st)).map_err(|e| self.err("", e))
    }

    /// The attributes of an entry, with no follow of a link.
    pub(crate) fn stat(&self, name: &str) -> Result<FileStat, FsErr> {
        fs::statat(&*self.fd, name, AtFlags::SYMLINK_NOFOLLOW)
            .map(|st| FileStat::of(&st))
            .map_err(|e| self.err(name, e))
    }

    /// The attributes of an entry, or `None` when it does not exist.
    pub(crate) fn stat_opt(&self, name: &str) -> Result<Option<FileStat>, FsErr> {
        match self.stat(name) {
            Ok(st) => Ok(Some(st)),
            Err(e) if e.not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// List the entries, with no `.` and no `..`. The order is not defined.
    pub(crate) fn list(&self) -> Result<Vec<DirItem>, FsErr> {
        let mut dir = fs::Dir::read_from(&*self.fd).map_err(|e| self.err("", e))?;
        let mut out = Vec::new();
        while let Some(entry) = dir.read() {
            let entry = entry.map_err(|e| self.err("", e))?;
            let raw = entry.file_name().to_bytes();
            if raw == b"." || raw == b".." {
                continue;
            }
            let (name, name_ok) = match std::str::from_utf8(raw) {
                Ok(s) => (s.to_owned(), true),
                Err(_) => (String::from_utf8_lossy(raw).into_owned(), false),
            };
            let ftype = match entry.file_type() {
                FileType::RegularFile => FType::File,
                FileType::Directory => FType::Dir,
                FileType::Symlink => FType::Symlink,
                FileType::Unknown => match self.stat_opt(&name)? {
                    Some(st) => st.ftype,
                    None => continue,
                },
                _ => FType::Other,
            };
            out.push(DirItem { name, name_ok, ftype });
        }
        Ok(out)
    }

    /// Read a regular file: at most `limit` plus 1 bytes. A link gives an error.
    pub(crate) fn read_file(&self, name: &str, limit: usize) -> Result<(Vec<u8>, FileStat), FsErr> {
        self.fault(FaultPoint::Read).map_err(|mut e| {
            e.path = format!("{}/{name}", self.label);
            e
        })?;
        let fd = fs::openat(
            &*self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|e| self.err(name, e))?;
        let st = fs::fstat(&fd).map(|st| FileStat::of(&st)).map_err(|e| self.err(name, e))?;
        if st.ftype != FType::File {
            return Err(self.err(name, Errno::NOTDIR));
        }
        let mut bytes = Vec::new();
        let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
        File::from(fd)
            .take(cap)
            .read_to_end(&mut bytes)
            .map_err(|e| self.err(name, e.raw_os_error().map_or(Errno::IO, Errno::from_raw_os_error)))?;
        Ok((bytes, st))
    }

    /// Read a regular file, or `None` when it does not exist.
    pub(crate) fn read_file_opt(&self, name: &str, limit: usize) -> Result<Option<(Vec<u8>, FileStat)>, FsErr> {
        match self.read_file(name, limit) {
            Ok(r) => Ok(Some(r)),
            Err(e) if e.not_found() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Read a part of a regular file: at most `max` bytes from `offset`, and the length of the
    /// file. A file that does not exist is an empty file.
    pub(crate) fn read_range(&self, name: &str, offset: u64, max: usize) -> Result<(Vec<u8>, u64), FsErr> {
        self.fault(FaultPoint::Read)?;
        let fd = match fs::openat(
            &*self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::NOENT) => return Ok((Vec::new(), 0)),
            Err(e) => return Err(self.err(name, e)),
        };
        let st = fs::fstat(&fd).map(|st| FileStat::of(&st)).map_err(|e| self.err(name, e))?;
        if st.ftype != FType::File {
            return Err(self.err(name, Errno::NOTDIR));
        }
        let length = st.sig.size;
        let mut bytes = Vec::new();
        if offset < length {
            let mut buf = vec![0u8; max.min(usize::try_from(length - offset).unwrap_or(usize::MAX))];
            let mut done = 0;
            while done < buf.len() {
                match fs::seek(&fd, fs::SeekFrom::Start(offset + done as u64))
                    .and_then(|_| rustix::io::read(&fd, &mut buf[done..]))
                {
                    Ok(0) => break,
                    Ok(n) => done += n,
                    Err(Errno::INTR) => {}
                    Err(e) => return Err(self.err(name, e)),
                }
            }
            buf.truncate(done);
            bytes = buf;
        }
        Ok((bytes, length))
    }

    /// The length of a regular file. A file that does not exist has the length 0.
    pub(crate) fn file_len(&self, name: &str) -> Result<u64, FsErr> {
        match self.stat_opt(name)? {
            None => Ok(0),
            Some(st) if st.ftype == FType::File => Ok(st.sig.size),
            Some(_) => Err(self.err(name, Errno::NOTDIR)),
        }
    }

    /// Call `fsync` on the folder.
    pub(crate) fn fsync(&self) -> Result<(), FsErr> {
        self.faults.check(FaultPoint::Fsync).map_err(|e| self.err("", e))?;
        fs::fsync(&*self.fd).map_err(|e| self.err("", e))
    }

    /// Remove a file.
    pub(crate) fn unlink(&self, name: &str) -> Result<(), FsErr> {
        fs::unlinkat(&*self.fd, name, AtFlags::empty()).map_err(|e| self.err(name, e))
    }

    /// Remove a file. A file that does not exist is not an error.
    pub(crate) fn unlink_opt(&self, name: &str) -> Result<(), FsErr> {
        match self.unlink(name) {
            Err(e) if e.not_found() => Ok(()),
            other => other,
        }
    }

    /// Remove an empty folder.
    pub(crate) fn rmdir(&self, name: &str) -> Result<(), FsErr> {
        fs::unlinkat(&*self.fd, name, AtFlags::REMOVEDIR).map_err(|e| self.err(name, e))
    }

    /// Cut a file to `len` bytes, and call `fsync` on it.
    pub(crate) fn truncate(&self, name: &str, len: u64) -> Result<(), FsErr> {
        let fd = fs::openat(&*self.fd, name, OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())
            .map_err(|e| self.err(name, e))?;
        fs::ftruncate(&fd, len).map_err(|e| self.err(name, e))?;
        fs::fsync(&fd).map_err(|e| self.err(name, e))
    }

    /// Take an exclusive `flock` on a lock file, with no wait. The file is made with the mode
    /// of the folder. `Ok(None)` means that another process holds the lock.
    pub(crate) fn lock(&self, name: &str) -> Result<Option<OwnedFd>, FsErr> {
        let fd = fs::openat(
            &*self.fd,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(self.modes.file),
        )
        .map_err(|e| self.err(name, e))?;
        fs::fchmod(&fd, Mode::from_raw_mode(self.modes.file)).map_err(|e| self.err(name, e))?;
        self.faults.check(FaultPoint::Flock).map_err(|e| self.err(name, e))?;
        match fs::flock(&fd, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Some(fd)),
            Err(Errno::WOULDBLOCK) => Ok(None),
            Err(e) => Err(self.err(name, e)),
        }
    }

    fn fill(&self, name: &str, fd: &OwnedFd, bytes: &[u8]) -> Result<(), FsErr> {
        let mut off = 0;
        while off < bytes.len() {
            self.faults.check(FaultPoint::Write).map_err(|e| self.err(name, e))?;
            match rustix::io::write(fd, &bytes[off..]) {
                Ok(0) => return Err(self.err(name, Errno::IO)),
                Ok(n) => off += n,
                Err(Errno::INTR) => {}
                Err(e) => return Err(self.err(name, e)),
            }
        }
        fs::fchmod(fd, Mode::from_raw_mode(self.modes.file)).map_err(|e| self.err(name, e))?;
        self.faults.check(FaultPoint::Fsync).map_err(|e| self.err(name, e))?;
        fs::fsync(fd).map_err(|e| self.err(name, e))
    }

    /// Write a file in the steps of 2.32. A new file in `tmp` with `O_EXCL`, `fsync` on it,
    /// a rename to `name` in this folder, then `fsync` on this folder. Thus a reader never
    /// sees half a file. A file that exists is replaced. Give the attributes of the new file.
    pub(crate) fn write_atomic(&self, tmp: &Dir, name: &str, bytes: &[u8]) -> Result<FileStat, FsErr> {
        let mut last = Errno::EXIST;
        for _ in 0..8 {
            let tmp_name = temp_name().map_err(|e| self.err(name, e))?;
            let fd = match fs::openat(
                &*tmp.fd,
                tmp_name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(self.modes.file),
            ) {
                Ok(fd) => fd,
                Err(Errno::EXIST) => {
                    last = Errno::EXIST;
                    continue;
                }
                Err(e) => return Err(tmp.err(&tmp_name, e)),
            };
            let filled = self.fill(name, &fd, bytes);
            drop(fd);
            if let Err(e) = filled {
                let _ = tmp.unlink_opt(&tmp_name);
                return Err(e);
            }
            if let Err(e) = fs::renameat(&*tmp.fd, tmp_name.as_str(), &*self.fd, name) {
                let _ = tmp.unlink_opt(&tmp_name);
                return Err(self.err(name, e));
            }
            self.fsync()?;
            // A rename changes the change time, so the attributes are read after it.
            return self.stat(name);
        }
        Err(self.err(name, last))
    }

    /// Add bytes at the end of a file with `O_APPEND`, then call `fsync`. The file is made
    /// when it does not exist. Give the new length.
    pub(crate) fn append(&self, name: &str, bytes: &[u8]) -> Result<u64, FsErr> {
        let existed = self.stat_opt(name)?.is_some();
        let fd = fs::openat(
            &*self.fd,
            name,
            OFlags::WRONLY | OFlags::APPEND | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(self.modes.file),
        )
        .map_err(|e| self.err(name, e))?;
        if !existed {
            fs::fchmod(&fd, Mode::from_raw_mode(self.modes.file)).map_err(|e| self.err(name, e))?;
        }
        let mut off = 0;
        while off < bytes.len() {
            self.faults.check(FaultPoint::Write).map_err(|e| self.err(name, e))?;
            match rustix::io::write(&fd, &bytes[off..]) {
                Ok(0) => return Err(self.err(name, Errno::IO)),
                Ok(n) => off += n,
                Err(Errno::INTR) => {}
                Err(e) => return Err(self.err(name, e)),
            }
        }
        self.faults.check(FaultPoint::Fsync).map_err(|e| self.err(name, e))?;
        fs::fsync(&fd).map_err(|e| self.err(name, e))?;
        if !existed {
            self.fsync()?;
        }
        let st = fs::fstat(&fd).map_err(|e| self.err(name, e))?;
        Ok(to_u64(st.st_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &Path) -> (Dir, Dir) {
        let root = Dir::open_root(path, Faults::new(), Modes::DOCUMENTS).unwrap();
        let tmp = root.open_dir_create(".tmp").unwrap();
        (root, tmp)
    }

    #[test]
    fn write_read_and_modes() {
        let t = tempfile::tempdir().unwrap();
        let (root, tmp) = root(t.path());
        let st = root.write_atomic(&tmp, "a.json", b"hello").unwrap();
        assert_eq!(st.mode, 0o644);
        let (bytes, st2) = root.read_file("a.json", 100).unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(st2.sig, st.sig);
        assert_eq!(root.stat(".tmp").unwrap().mode, 0o755);
        assert!(tmp.list().unwrap().is_empty());
        assert!(root.read_file_opt("none", 10).unwrap().is_none());
    }

    #[test]
    fn a_read_stops_at_the_limit_plus_one() {
        let t = tempfile::tempdir().unwrap();
        let (root, tmp) = root(t.path());
        root.write_atomic(&tmp, "a", &[b'x'; 50]).unwrap();
        assert_eq!(root.read_file("a", 10).unwrap().0.len(), 11);
    }

    #[test]
    fn a_link_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir(t.path().join("real")).unwrap();
        std::fs::write(t.path().join("real/file"), b"x").unwrap();
        std::os::unix::fs::symlink(t.path().join("real"), t.path().join("d")).unwrap();
        std::os::unix::fs::symlink(t.path().join("real/file"), t.path().join("f")).unwrap();
        let (root, _) = root(t.path());
        assert!(matches!(StoreError::from(root.open_dir("d").unwrap_err()), StoreError::Corrupt(_)));
        assert!(matches!(StoreError::from(root.read_file("f", 10).unwrap_err()), StoreError::Corrupt(_)));
        assert!(root.open_path(&["d", "x"], false).is_err());
    }

    #[test]
    fn append_and_range() {
        let t = tempfile::tempdir().unwrap();
        let (root, _) = root(t.path());
        assert_eq!(root.file_len("o").unwrap(), 0);
        assert_eq!(root.append("o", b"abc").unwrap(), 3);
        assert_eq!(root.append("o", b"defg").unwrap(), 7);
        assert_eq!(root.read_range("o", 2, 3).unwrap(), (b"cde".to_vec(), 7));
        assert_eq!(root.read_range("o", 9, 3).unwrap(), (Vec::new(), 7));
        assert_eq!(root.read_range("none", 0, 3).unwrap(), (Vec::new(), 0));
        root.truncate("o", 4).unwrap();
        assert_eq!(root.file_len("o").unwrap(), 4);
    }

    #[test]
    fn a_second_lock_fails() {
        let t = tempfile::tempdir().unwrap();
        let (root, _) = root(t.path());
        let first = root.lock(".lock").unwrap();
        assert!(first.is_some());
        let other = Dir::open_root(t.path(), Faults::new(), Modes::DOCUMENTS).unwrap();
        assert!(other.lock(".lock").unwrap().is_none());
    }
}
