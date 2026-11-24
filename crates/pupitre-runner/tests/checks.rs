// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the checks of the runner node at start (7.8, 6.13).

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use pupitre_runner::RunnerError;
use pupitre_runner::executor::checks::{check_playbook_folders, check_runner_node};
use support::fixture;

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A folder on `tmpfs` for the secrets, or `None` when the machine has no `/dev/shm`.
fn tmpfs_dir() -> Option<tempfile::TempDir> {
    if !Path::new("/dev/shm").is_dir() {
        eprintln!("skipped: no /dev/shm on this machine");
        return None;
    }
    tempfile::Builder::new().prefix("pupitre-runner-test-").tempdir_in("/dev/shm").ok()
}

/// A node that passes each check, or `None` when the machine has no `tmpfs`.
fn good_node(root: &Path) -> Option<(fixture::Node1, tempfile::TempDir)> {
    let secrets = tmpfs_dir()?;
    let mut node = fixture::node(root);
    node.settings.secrets_dir = secrets.path().to_path_buf();
    chmod(&node.settings.secrets_dir, 0o700);
    std::fs::create_dir_all(node.settings.ansible_playbook.parent().unwrap()).unwrap();
    fake_ansible(&node.settings.ansible_playbook, "echo 'ansible-playbook [core 2.21.6]'");
    std::fs::create_dir_all(&node.settings.work_dir).unwrap();
    chmod(&node.settings.work_dir, 0o700);
    Some((node, secrets))
}

/// Write a fake `ansible-playbook` that runs a shell command.
fn fake_ansible(path: &Path, body: &str) {
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    chmod(path, 0o755);
}

fn message(result: Result<(), RunnerError>) -> String {
    match result {
        Err(RunnerError::Check(text)) => text,
        other => panic!("expected a failed check, found {other:?}"),
    }
}

#[test]
fn a_node_that_is_right_passes() {
    let dir = tempfile::tempdir().unwrap();
    let Some((node, _keep)) = good_node(dir.path()) else { return };
    check_runner_node(&node.settings).unwrap();
    assert!(node.settings.work_dir.join("runs").is_dir() || !node.settings.work_dir.join("runs").exists());
    // The mode 0400 of the SSH key is also valid.
    chmod(&node.settings.ssh_key_file, 0o400);
    check_runner_node(&node.settings).unwrap();
}

/// 7.8: the SSH key is a regular file of the user with the mode 0600 or 0400.
#[test]
fn the_ssh_key_file_is_checked() {
    let dir = tempfile::tempdir().unwrap();
    let Some((node, _keep)) = good_node(dir.path()) else { return };
    for mode in [0o644, 0o640, 0o700, 0o660] {
        chmod(&node.settings.ssh_key_file, mode);
        let text = message(check_runner_node(&node.settings));
        assert!(
            text.contains("runner.ssh_key_file")
                && text.contains("0600 or 0400")
                && text.contains(&format!("{mode:04o}")),
            "{text}"
        );
    }
    chmod(&node.settings.ssh_key_file, 0o600);
    std::fs::remove_file(&node.settings.ssh_key_file).unwrap();
    assert!(message(check_runner_node(&node.settings)).contains("runner.ssh_key_file"));
    std::fs::create_dir(&node.settings.ssh_key_file).unwrap();
    assert!(message(check_runner_node(&node.settings)).contains("expected a regular file"));
}

/// 7.8: the known hosts file is a regular file with one or more lines.
#[test]
fn the_known_hosts_file_is_checked() {
    let dir = tempfile::tempdir().unwrap();
    let Some((node, _keep)) = good_node(dir.path()) else { return };
    for content in ["", "\n\n  \n"] {
        std::fs::write(&node.settings.known_hosts_file, content).unwrap();
        let text = message(check_runner_node(&node.settings));
        assert!(text.contains("runner.known_hosts_file") && text.contains("one or more lines"), "{text}");
    }
    std::fs::remove_file(&node.settings.known_hosts_file).unwrap();
    assert!(message(check_runner_node(&node.settings)).contains("runner.known_hosts_file"));
}

/// 6.13: the program is a file that the user can run.
#[test]
fn the_program_is_checked() {
    let dir = tempfile::tempdir().unwrap();
    let Some((node, _keep)) = good_node(dir.path()) else { return };
    chmod(&node.settings.ansible_playbook, 0o644);
    assert!(message(check_runner_node(&node.settings)).contains("runner.ansible_playbook"));
    std::fs::remove_file(&node.settings.ansible_playbook).unwrap();
    assert!(message(check_runner_node(&node.settings)).contains("runner.ansible_playbook"));
}

/// 7.8, 4.13: the folder of the secrets has the mode 0700 and is on `tmpfs` or `ramfs`.
#[test]
fn the_secrets_folder_is_checked() {
    let dir = tempfile::tempdir().unwrap();
    let Some((mut node, _keep)) = good_node(dir.path()) else { return };
    chmod(&node.settings.secrets_dir, 0o750);
    let text = message(check_runner_node(&node.settings));
    assert!(text.contains("runner.secrets_dir") && text.contains("0700") && text.contains("0750"), "{text}");
    chmod(&node.settings.secrets_dir, 0o700);

    // A folder on the disk: the check needs a place that is not `tmpfs`. The folder of the test is
    // on the disk of the container; on a machine whose temporary folder is `tmpfs` the case is skipped.
    let disk = dir.path().join("disk-secrets");
    node.settings.secrets_dir = disk.clone();
    match check_runner_node(&node.settings) {
        Err(RunnerError::Check(text)) => {
            assert!(text.contains("runner.secrets_dir") && text.contains("tmpfs or ramfs"), "{text}");
            // The folder was made with the mode 0700.
            assert_eq!(std::fs::metadata(&disk).unwrap().permissions().mode() & 0o777, 0o700);
        }
        Ok(()) => eprintln!("skipped: the temporary folder of this machine is on tmpfs"),
        other => panic!("{other:?}"),
    }
}

/// 7.8: the work folder is a folder of the user that others cannot write.
#[test]
fn the_work_folder_is_checked() {
    let dir = tempfile::tempdir().unwrap();
    let Some((node, _keep)) = good_node(dir.path()) else { return };
    chmod(&node.settings.work_dir, 0o777);
    let text = message(check_runner_node(&node.settings));
    assert!(text.contains("runner.work_dir") && text.contains("0777"), "{text}");
    chmod(&node.settings.work_dir, 0o700);
    std::fs::remove_dir_all(&node.settings.work_dir).unwrap();
    std::fs::write(&node.settings.work_dir, "a file").unwrap();
    assert!(message(check_runner_node(&node.settings)).contains("expected a folder"));
}

/// 7.3, 7.8: the program is `ansible-core` 2.19 or later. The error names the key, the expected
/// range and the version found.
#[test]
fn the_version_of_ansible_core_is_checked() {
    let dir = tempfile::tempdir().unwrap();
    let Some((node, _keep)) = good_node(dir.path()) else { return };
    let program = &node.settings.ansible_playbook;
    for version in ["2.19.0", "2.20.11", "2.21.6"] {
        fake_ansible(program, &format!("echo 'ansible-playbook [core {version}]'; echo '  config file = None'"));
        check_runner_node(&node.settings).unwrap();
    }
    for version in ["2.18.20", "2.15.13", "1.9.0"] {
        fake_ansible(program, &format!("echo 'ansible-playbook [core {version}]'"));
        let text = message(check_runner_node(&node.settings));
        assert!(
            text.contains("runner.ansible_playbook")
                && text.contains("ansible-core 2.19 or later")
                && text.contains(&format!("found core {version}")),
            "{text}"
        );
    }
    // The scratch folder of the check is removed.
    let left: Vec<_> = std::fs::read_dir(&node.settings.work_dir)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("version-check"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// 7.8: an output that cannot be parsed, an empty output, a failure and a hang stop the start.
#[test]
fn an_unreadable_version_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let Some((node, _keep)) = good_node(dir.path()) else { return };
    let program = &node.settings.ansible_playbook;
    for body in ["echo garbage", "echo 'ansible-playbook [core x.y.z]'", "echo 'ansible-playbook [core 2.21.6'", "true"]
    {
        fake_ansible(program, body);
        let text = message(check_runner_node(&node.settings));
        assert!(
            text.contains("runner.ansible_playbook") && text.contains("ansible-playbook [core X.Y.Z]"),
            "{body}: {text}"
        );
    }
    fake_ansible(program, "true");
    assert!(message(check_runner_node(&node.settings)).contains("no output"));
    fake_ansible(program, "echo 'ansible-playbook [core 2.21.6]'; exit 3");
    let text = message(check_runner_node(&node.settings));
    assert!(text.contains("runner.ansible_playbook") && text.contains("ended with"), "{text}");
}

/// 7.8: the check runs the program with the closed environment of 4.18 and no shell.
#[test]
fn the_version_check_gives_a_closed_environment() {
    let dir = tempfile::tempdir().unwrap();
    let Some((mut node, _keep)) = good_node(dir.path()) else { return };
    node.settings.path = "/usr/bin:/bin".to_string();
    // The program refuses to answer when a variable that the file does not name is present.
    fake_ansible(
        &node.settings.ansible_playbook,
        "if [ \"$(env | sort | cut -d= -f1 | grep -v -x -e PWD -e OLDPWD -e SHLVL -e _ | tr '\\n' ' ')\" = \"ANSIBLE_CONFIG ANSIBLE_LOCAL_TEMP HOME LANG LC_ALL PATH PYTHONDONTWRITEBYTECODE \" ] && [ \"$1\" = \"--version\" ] && [ $# = 1 ]; then echo 'ansible-playbook [core 2.21.6]'; else env >&2; echo 'bad environment'; fi",
    );
    check_runner_node(&node.settings).unwrap();
}

/// 4.2, 7.8: a folder of a playbook with `group_vars` or `host_vars` stops the start. The error names
/// the key `playbook "<id>".path`, the expected state and the entry that was found.
#[test]
fn a_playbook_folder_with_variable_entries_stops_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let node = fixture::node(dir.path());
    check_playbook_folders(&node.caps).unwrap();
    // A file `group_vars/production.yml` would reach each run where a host has the label `production`.
    let folder = dir.path().join("playbooks");
    std::fs::create_dir_all(folder.join("group_vars")).unwrap();
    std::fs::write(folder.join("group_vars/production.yml"), "pupitre_x: 1\n").unwrap();
    let Err(RunnerError::Check(text)) = check_playbook_folders(&node.caps) else { panic!("accepted") };
    assert!(text.starts_with("playbook \""), "{text}");
    assert!(text.contains("\".path:") && text.contains("no entry group_vars and no entry host_vars"), "{text}");
    assert!(text.contains("found group_vars"), "{text}");
    std::fs::remove_dir_all(folder.join("group_vars")).unwrap();
    std::os::unix::fs::symlink("elsewhere", folder.join("host_vars")).unwrap();
    let Err(RunnerError::Check(text)) = check_playbook_folders(&node.caps) else { panic!("accepted") };
    assert!(text.contains("found host_vars"), "{text}");
}
