// SPDX-License-Identifier: MIT OR Apache-2.0
//! The checks of the runner node at start (7.8, 6.13) and its node id (4.8).

use std::fs;
use std::io::Read;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::executor::RunnerError;
use crate::settings::RunnerSettings;

/// The magic numbers of `statfs` for `tmpfs` and `ramfs` (4.13).
const TMPFS_MAGIC: i128 = 0x0102_1994;
const RAMFS_MAGIC: i128 = 0x8584_58f6;

/// The oldest `ansible-core` that Pupitre supports (7.3, 7.8). Before 2.19 the tag `!unsafe` is lost
/// when a playbook reads a value through `hostvars`, so a filter can make the value active.
const MIN_CORE: (u64, u64) = (2, 19);

/// The time that `ansible-playbook --version` has to end.
const VERSION_TIMEOUT: Duration = Duration::from_secs(60);

/// The most bytes that the check reads from each output of `--version`.
const VERSION_OUTPUT_MAX: u64 = 64 * 1024;

fn fail<T>(key: &str, path: &Path, cause: impl std::fmt::Display) -> Result<T, RunnerError> {
    Err(RunnerError::Check(format!("{key}: {cause} (path {})", path.display())))
}

fn mode_of(meta: &fs::Metadata) -> u32 {
    meta.permissions().mode() & 0o777
}

/// A regular file of the user of the process (7.8).
fn own_regular_file(key: &str, path: &Path) -> Result<fs::Metadata, RunnerError> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return fail(key, path, format!("expected a regular file that the process can read, found: {e}")),
    };
    if !meta.is_file() {
        return fail(key, path, "expected a regular file");
    }
    Ok(meta)
}

/// Make a folder with the mode 0700 when it is absent. Then check that it is a folder of the
/// user of the process.
fn private_folder(key: &str, path: &Path) -> Result<fs::Metadata, RunnerError> {
    if !path.exists()
        && let Err(e) = fs::DirBuilder::new().mode(0o700).create(path)
    {
        return fail(key, path, format!("expected a folder, and it cannot be made: {e}"));
    }
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) => return fail(key, path, format!("expected a folder, found: {e}")),
    };
    if !meta.is_dir() {
        return fail(key, path, "expected a folder");
    }
    if meta.uid() != rustix::process::geteuid().as_raw() {
        return fail(key, path, format!("expected a folder of the user of the process, found the user {}", meta.uid()));
    }
    Ok(meta)
}

/// Check the runner node (7.8, 6.13). The checks cover the SSH key, the known hosts file, the
/// work folder, the folder of the secrets on `tmpfs`, and the program `ansible-playbook`. The
/// first failure gives the key and the cause.
pub fn check_runner_node(settings: &RunnerSettings) -> Result<(), RunnerError> {
    // The SSH key: a regular file of the user, mode 0600 or 0400.
    let key = &settings.ssh_key_file;
    let meta = own_regular_file("runner.ssh_key_file", key)?;
    if meta.uid() != rustix::process::geteuid().as_raw() {
        return fail(
            "runner.ssh_key_file",
            key,
            format!("expected a file of the user of the process, found the user {}", meta.uid()),
        );
    }
    if !matches!(mode_of(&meta), 0o600 | 0o400) {
        return fail(
            "runner.ssh_key_file",
            key,
            format!("expected the mode 0600 or 0400, found {:04o}", mode_of(&meta)),
        );
    }

    // The known hosts file: a regular file with one or more lines.
    let known = &settings.known_hosts_file;
    own_regular_file("runner.known_hosts_file", known)?;
    match fs::read(known) {
        Ok(bytes) if bytes.split(|b| *b == b'\n').any(|l| !l.iter().all(u8::is_ascii_whitespace)) => {}
        Ok(_) => return fail("runner.known_hosts_file", known, "expected one or more lines, found none"),
        Err(e) => {
            return fail(
                "runner.known_hosts_file",
                known,
                format!("expected a file that the process can read, found: {e}"),
            );
        }
    }

    // The program: a regular file that the user can run.
    let program = &settings.ansible_playbook;
    let meta = own_regular_file("runner.ansible_playbook", program)?;
    if mode_of(&meta) & 0o111 == 0 {
        return fail(
            "runner.ansible_playbook",
            program,
            format!("expected an executable file, found the mode {:04o}", mode_of(&meta)),
        );
    }

    // The work folder.
    let work = &settings.work_dir;
    let meta = private_folder("runner.work_dir", work)?;
    if mode_of(&meta) & 0o022 != 0 {
        return fail(
            "runner.work_dir",
            work,
            format!("expected no write right for the group and the others, found {:04o}", mode_of(&meta)),
        );
    }
    if let Err(e) = fs::create_dir_all(work.join("runs")) {
        return fail(
            "runner.work_dir",
            work,
            format!("expected a folder where the process can make the folder runs, found: {e}"),
        );
    }

    // The folder of the secrets: mode 0700, on `tmpfs` or `ramfs`.
    let secrets = &settings.secrets_dir;
    let meta = private_folder("runner.secrets_dir", secrets)?;
    if mode_of(&meta) != 0o700 {
        return fail("runner.secrets_dir", secrets, format!("expected the mode 0700, found {:04o}", mode_of(&meta)));
    }
    match rustix::fs::statfs(secrets) {
        Ok(stat) => {
            let kind = i128::from(stat.f_type);
            if kind != TMPFS_MAGIC && kind != RAMFS_MAGIC {
                return fail("runner.secrets_dir", secrets, "expected a folder on tmpfs or ramfs");
            }
        }
        Err(e) => {
            return fail(
                "runner.secrets_dir",
                secrets,
                format!("expected a folder whose file system is known, found: {e}"),
            );
        }
    }

    // The version of ansible-core, after the folders: the program runs with a private folder.
    check_ansible_version(settings)?;
    Ok(())
}

/// The names that Ansible reads as variable folders next to a playbook (4.2, 7.8).
const VARIABLE_ENTRIES: [&str; 2] = ["group_vars", "host_vars"];

/// Refuse a playbook whose folder holds an entry `group_vars` or `host_vars` (4.2, 7.8). The entry
/// can be a file, a folder or a symbolic link, and the check does not follow a link. Ansible reads
/// these entries next to the playbook for each run. They win over the inventory of Pupitre, and a
/// label that an editor gives a host becomes a group, so a file `group_vars/<label>.yml` would reach
/// the run of another environment. The error text names the key and the entry that was found.
pub fn check_playbook_folder(spec: &pupitre_core::capability::PlaybookSpec) -> Result<(), RunnerError> {
    let key = format!("playbook \"{}\".path", spec.id.as_str());
    let Some(folder) = spec.path.parent() else {
        return fail(&key, &spec.path, "expected a path with a folder");
    };
    for name in VARIABLE_ENTRIES {
        let entry = folder.join(name);
        match fs::symlink_metadata(&entry) {
            Ok(_) => {
                return fail(
                    &key,
                    &entry,
                    format!("expected a folder with no entry group_vars and no entry host_vars, found {name}"),
                );
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return fail(&key, &entry, format!("expected a folder that the process can inspect, found: {e}"));
            }
        }
    }
    Ok(())
}

/// Check the folder of each playbook of the catalog (4.2, 7.8). The first failure stops the start.
pub fn check_playbook_folders(capabilities: &pupitre_core::capability::Capabilities) -> Result<(), RunnerError> {
    capabilities.playbooks.values().try_for_each(check_playbook_folder)
}

/// Parse the line `ansible-playbook [core X.Y.Z]` of the output of `--version`. The result is
/// the major and the minor number and the full version text. `None` means that no line has the
/// form, or that a number is absent.
pub fn parse_core_version(output: &str) -> Option<(u64, u64, String)> {
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("ansible-playbook [core ") else { continue };
        let Some(version) = rest.split(']').next().filter(|_| rest.contains(']')) else { continue };
        let mut parts = version.split('.');
        let number = |part: Option<&str>| -> Option<u64> {
            let digits: String = part?.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        };
        let (Some(major), Some(minor)) = (number(parts.next()), number(parts.next())) else { continue };
        return Some((major, minor, version.to_string()));
    }
    None
}

/// Run the program with `--version` and check that it is `ansible-core` 2.19 or later (7.3,
/// 7.8). The program starts with no shell and with an explicit environment, as a run does
/// (4.17, 4.18), and a private folder for its home and its temporary files.
fn check_ansible_version(settings: &RunnerSettings) -> Result<(), RunnerError> {
    let key = "runner.ansible_playbook";
    let program = &settings.ansible_playbook;
    let scratch = settings.work_dir.join(format!("version-check-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    for folder in [scratch.clone(), scratch.join("home"), scratch.join("tmp")] {
        if let Err(e) = fs::DirBuilder::new().mode(0o700).create(&folder) {
            return fail(key, &folder, format!("expected a folder that the process can make, found: {e}"));
        }
    }
    let config = scratch.join("ansible.cfg");
    if let Err(e) = fs::write(&config, "") {
        return fail(key, &config, format!("expected a file that the process can write, found: {e}"));
    }
    let result = run_version(settings, &scratch, &config);
    let _ = fs::remove_dir_all(&scratch);
    let output = match result {
        Ok(text) => text,
        Err(cause) => {
            return fail(
                key,
                program,
                format!("expected a program that prints its version with --version, found: {cause}"),
            );
        }
    };
    match parse_core_version(&output) {
        Some((major, minor, _)) if (major, minor) >= MIN_CORE => Ok(()),
        Some((_, _, version)) => fail(
            key,
            program,
            format!("expected ansible-core {}.{} or later, found core {version}", MIN_CORE.0, MIN_CORE.1),
        ),
        None => fail(
            key,
            program,
            format!(
                "expected a line \"ansible-playbook [core X.Y.Z]\" (ansible-core {}.{} or later) in the output of --version, found {}",
                MIN_CORE.0,
                MIN_CORE.1,
                describe(&output)
            ),
        ),
    }
}

/// The first line of an output, with the control characters escaped and at most 80 characters.
fn describe(output: &str) -> String {
    let line = output.lines().next().unwrap_or("");
    if line.is_empty() {
        return "no output".to_string();
    }
    let shown: String = line.chars().take(80).flat_map(char::escape_default).collect();
    format!("\"{shown}\"")
}

/// Start `<program> --version` and return its standard output. The environment is the one of 4.18,
/// without the variables of the events, and `ANSIBLE_CONFIG` names an empty file.
fn run_version(settings: &RunnerSettings, scratch: &Path, config: &Path) -> Result<String, String> {
    let p = |path: &Path| path.to_string_lossy().into_owned();
    let mut busy = 0;
    let mut child = loop {
        let spawned = Command::new(&settings.ansible_playbook)
            .arg("--version")
            .env_clear()
            .env("PATH", &settings.path)
            .env("HOME", p(&scratch.join("home")))
            .env("LANG", "C.UTF-8")
            .env("LC_ALL", "C.UTF-8")
            .env("ANSIBLE_CONFIG", p(config))
            .env("ANSIBLE_LOCAL_TEMP", p(&scratch.join("tmp")))
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .current_dir(scratch)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        match spawned {
            Ok(child) => break child,
            // The file was written a moment ago by a package tool: wait a little.
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && busy < 10 => {
                busy += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("the program cannot start: {e}")),
        }
    };
    let read = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            if let Some(stream) = stream {
                let _ = stream.take(VERSION_OUTPUT_MAX).read_to_end(&mut text);
            }
            String::from_utf8_lossy(&text).into_owned()
        })
    };
    let out = read(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let err = read(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
    let deadline = Instant::now() + VERSION_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("the program did not end in {} seconds", VERSION_TIMEOUT.as_secs()));
            }
            Err(e) => return Err(format!("the program cannot be waited for: {e}")),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let _ = err.join();
    if !status.success() {
        return Err(format!("the program ended with {status}"));
    }
    Ok(stdout)
}

/// Read the node id of `<runner.work_dir>/node-id`, or write a new one with the mode 0600
/// (4.8). A file that is not a UUID stops the start.
pub fn node_id(settings: &RunnerSettings) -> Result<String, RunnerError> {
    let path = settings.work_dir.join("node-id");
    match fs::read_to_string(&path) {
        Ok(text) => {
            let id = text.trim_end_matches('\n');
            if is_uuid(id) {
                Ok(id.to_string())
            } else {
                fail("runner.work_dir", &path, "expected a UUID in the file node-id")
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let id = new_uuid().map_err(|e| RunnerError::Check(format!("runner.work_dir: no random bytes: {e}")))?;
            let mut file = match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
                Ok(f) => f,
                Err(e) => {
                    return fail(
                        "runner.work_dir",
                        &path,
                        format!("expected a file node-id that the process can write, found: {e}"),
                    );
                }
            };
            file.write_all(id.as_bytes()).and_then(|()| file.write_all(b"\n")).and_then(|()| file.sync_all()).map_err(
                |e| {
                    RunnerError::Check(format!(
                        "runner.work_dir: cannot write the file node-id: {e} (path {})",
                        path.display()
                    ))
                },
            )?;
            Ok(id)
        }
        Err(e) => {
            fail("runner.work_dir", &path, format!("expected a file node-id that the process can read, found: {e}"))
        }
    }
}

/// True for the text form of a UUID: 8-4-4-4-12 lower-case hexadecimal characters.
pub fn is_uuid(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_digit() || (b'a'..=b'f').contains(c) }
        })
}

/// A random UUID, version 4.
fn new_uuid() -> Result<String, getrandom::Error> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b)?;
    b[6] = 0x40 | (b[6] & 0x0f);
    b[8] = 0x80 | (b[8] & 0x3f);
    let hex: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    let h = hex.concat();
    Ok(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playbook_folder_with_variable_entries_is_refused() {
        use pupitre_core::capability::{PlaybookKind, PlaybookSpec};
        use pupitre_core::ids::PlaybookId;
        let dir = tempfile::tempdir().unwrap();
        let spec = PlaybookSpec {
            id: PlaybookId::parse("deploy-relay").unwrap(),
            kind: PlaybookKind::Deploy,
            path: dir.path().join("deploy.yml"),
        };
        fs::write(&spec.path, "- hosts: all\n").unwrap();
        check_playbook_folder(&spec).unwrap();
        // Other names stay allowed.
        fs::create_dir(dir.path().join("roles")).unwrap();
        fs::write(dir.path().join("group_vars.yml"), "").unwrap();
        check_playbook_folder(&spec).unwrap();
        for name in ["group_vars", "host_vars"] {
            for kind in ["folder", "file", "link", "dangling-link"] {
                let entry = dir.path().join(name);
                match kind {
                    "folder" => fs::create_dir(&entry).unwrap(),
                    "file" => fs::write(&entry, "").unwrap(),
                    "link" => std::os::unix::fs::symlink("roles", &entry).unwrap(),
                    _ => std::os::unix::fs::symlink("nowhere", &entry).unwrap(),
                }
                let Err(RunnerError::Check(text)) = check_playbook_folder(&spec) else {
                    panic!("{name} {kind} was accepted")
                };
                assert!(
                    text.contains("playbook \"deploy-relay\".path") && text.contains("found ") && text.contains(name),
                    "{text}"
                );
                match kind {
                    "folder" => fs::remove_dir(&entry).unwrap(),
                    _ => fs::remove_file(&entry).unwrap(),
                }
            }
        }
        check_playbook_folder(&spec).unwrap();
    }

    #[test]
    fn the_version_line_is_parsed() {
        let ok = |text: &str| parse_core_version(text).map(|(a, b, _)| (a, b));
        assert_eq!(ok("ansible-playbook [core 2.19.0]\n  config file = None\n"), Some((2, 19)));
        assert_eq!(ok("warning\nansible-playbook [core 2.21.6]\n"), Some((2, 21)));
        assert_eq!(ok("ansible-playbook [core 2.20.0rc1]"), Some((2, 20)));
        assert_eq!(ok("ansible-playbook [core 2.18.20]"), Some((2, 18)));
        for bad in [
            "",
            "garbage",
            "ansible-playbook 2.19.0",
            "ansible-playbook [core 2.19.0",
            "ansible-playbook [core x.y]",
            "ansible-playbook [core 2]",
        ] {
            assert_eq!(ok(bad), None, "{bad}");
        }
    }

    #[test]
    fn uuids_have_the_text_form() {
        let id = new_uuid().unwrap();
        assert!(is_uuid(&id), "{id}");
        assert_eq!(&id[14..15], "4");
        assert!(!is_uuid("not-a-uuid"));
        assert!(!is_uuid("018BCFE5-6801-7101-8101-010101010101"));
    }
}
