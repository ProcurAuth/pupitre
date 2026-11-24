// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read a regular file with a size limit (6.6, 6.25). This module only reads.

use std::fmt;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Why a read failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The last part of the path is a symbolic link.
    Symlink,
    /// The path is not a regular file.
    NotRegular,
    /// The file has more bytes than the limit. The field is the size that the system gave.
    TooLarge(u64),
    /// The system gave an error. The field is its text.
    Io(String),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Symlink => f.write_str("the path is a symbolic link"),
            ReadError::NotRegular => f.write_str("the path is not a regular file"),
            ReadError::TooLarge(n) => write!(f, "the file has {n} bytes, more than the limit"),
            ReadError::Io(text) => f.write_str(text),
        }
    }
}

/// Text of an I/O error without the Rust suffix ` (os error N)`.
fn io_text(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(i) => text[..i].to_string(),
        None => text,
    }
}

/// Read a regular file of `max_bytes` at most. Refuse a symbolic link as the last part of the
/// path (6.6). The function checks the link state of the path, then opens the file. It
/// compares the device and the inode of both, so that it finds a swap in between.
pub fn read_regular_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, ReadError> {
    let before = std::fs::symlink_metadata(path).map_err(|e| ReadError::Io(io_text(&e)))?;
    if before.file_type().is_symlink() {
        return Err(ReadError::Symlink);
    }
    if !before.is_file() {
        return Err(ReadError::NotRegular);
    }
    let file = std::fs::File::open(path).map_err(|e| ReadError::Io(io_text(&e)))?;
    let after = file.metadata().map_err(|e| ReadError::Io(io_text(&e)))?;
    if !after.is_file() || after.dev() != before.dev() || after.ino() != before.ino() {
        return Err(ReadError::Io("the file changed while it was opened".to_string()));
    }
    if after.len() > max_bytes {
        return Err(ReadError::TooLarge(after.len()));
    }
    let mut data = Vec::new();
    file.take(max_bytes.saturating_add(1)).read_to_end(&mut data).map_err(|e| ReadError::Io(io_text(&e)))?;
    if data.len() as u64 > max_bytes {
        return Err(ReadError::TooLarge(data.len() as u64));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(path: &Path, content: &[u8]) -> std::io::Result<()> {
        let mut file = std::fs::File::options().write(true).create(true).truncate(true).open(path)?;
        file.write_all(content)
    }

    #[test]
    fn reads_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        write_file(&p, b"abc").unwrap();
        assert_eq!(read_regular_file(&p, 10).unwrap(), b"abc");
    }

    #[test]
    fn refuses_a_symbolic_link() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        write_file(&p, b"abc").unwrap();
        let l = dir.path().join("l");
        std::os::unix::fs::symlink(&p, &l).unwrap();
        assert_eq!(read_regular_file(&l, 10), Err(ReadError::Symlink));
    }

    #[test]
    fn refuses_a_directory_and_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_regular_file(dir.path(), 10), Err(ReadError::NotRegular));
        let err = read_regular_file(&dir.path().join("none"), 10).unwrap_err();
        assert_eq!(err, ReadError::Io("No such file or directory".to_string()));
    }

    #[test]
    fn refuses_a_large_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        write_file(&p, b"abcdef").unwrap();
        assert_eq!(read_regular_file(&p, 5), Err(ReadError::TooLarge(6)));
        assert!(read_regular_file(&p, 6).is_ok());
    }
}
