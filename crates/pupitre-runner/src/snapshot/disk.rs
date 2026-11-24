// SPDX-License-Identifier: MIT OR Apache-2.0
//! The file steps of the snapshot builder (4.10, 4.13). They write a file with a mode, make the
//! snapshot read only, and remove a folder that the builder made read only.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Make a folder and its parents with the mode 0700. A folder that exists stays as it is.
pub fn make_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
}

/// Write a file with the given mode. The file is new, or it is replaced.
pub fn write_file(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        make_dir(parent)?;
    }
    let mut file: File = OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(path)?;
    // The mode of `open` is cut by the umask: set it again.
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The paths of a folder tree, folders after their content (the deepest first).
fn walk(root: &Path, files: &mut Vec<std::path::PathBuf>, dirs: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        // A symbolic link is never followed: the builder writes none.
        if entry.file_type()?.is_dir() {
            walk(&path, files, dirs)?;
            dirs.push(path);
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// Remove the write bits: 0500 for the folders and 0400 for the files (4.10). The root folder
/// is included.
pub fn make_read_only(root: &Path) -> io::Result<()> {
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    walk(root, &mut files, &mut dirs)?;
    for file in &files {
        fs::set_permissions(file, fs::Permissions::from_mode(0o400))?;
    }
    for dir in dirs {
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500))?;
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o500))
}

/// Remove a folder tree, also when the builder made it read only. A path that does not exist
/// is not an error. A symbolic link is removed, never followed.
pub fn remove_tree(root: &Path) -> io::Result<()> {
    let meta = match fs::symlink_metadata(root) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !meta.is_dir() {
        return fs::remove_file(root);
    }
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_tree(&entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    fs::remove_dir(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_only_tree_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("snapshot");
        write_file(&root.join("a/b/c.txt"), b"x", 0o600).unwrap();
        write_file(&root.join("d.txt"), b"y", 0o600).unwrap();
        make_read_only(&root).unwrap();
        assert_eq!(fs::metadata(root.join("a")).unwrap().permissions().mode() & 0o777, 0o500);
        assert_eq!(fs::metadata(root.join("d.txt")).unwrap().permissions().mode() & 0o777, 0o400);
        assert!(write_file(&root.join("new.txt"), b"z", 0o600).is_err() || nix_root());
        remove_tree(&root).unwrap();
        assert!(!root.exists());
        remove_tree(&root).unwrap();
    }

    /// A test as root can write in a read only folder.
    fn nix_root() -> bool {
        rustix::process::geteuid().is_root()
    }
}
