// SPDX-License-Identifier: MIT OR Apache-2.0
//! The child of a run: its command line (4.17), its environment (4.18) and the file
//! `ansible.cfg` (4.15). These functions are pure: they build texts and lists, and they start
//! nothing.

use std::path::{Path, PathBuf};

use crate::settings::RunnerSettings;

/// The phase of a run (4.25).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The render phase of a `deploy` check.
    Render,
    /// The main phase.
    Main,
}

impl Phase {
    /// The name of the phase in the events and in `PUPITRE_EVENTS_PHASE`.
    pub fn name(self) -> &'static str {
        match self {
            Phase::Render => "render",
            Phase::Main => "main",
        }
    }
}

/// The folders of one run (4.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFolders {
    /// `<runner.work_dir>/runs/<run id>`
    pub run: PathBuf,
    /// `<runner.secrets_dir>/<run id>`
    pub secrets: PathBuf,
}

impl RunFolders {
    /// The folders of a run id.
    pub fn of(settings: &RunnerSettings, run_id: &str) -> RunFolders {
        RunFolders { run: settings.work_dir.join("runs").join(run_id), secrets: settings.secrets_dir.join(run_id) }
    }

    /// `<run>/snapshot`
    pub fn snapshot(&self) -> PathBuf {
        self.run.join("snapshot")
    }

    /// `<runner.secrets_dir>/<run id>/render`: the folder of the render phase (4.13, 4.25). It is
    /// in the private folder, so the rendered files stay on `tmpfs` and go at the end of the run.
    pub fn render(&self) -> PathBuf {
        self.secrets.join("render")
    }
}

/// The inventory group of the hosts of a run (4.11). The limit of the command line names it
/// (4.17). No label and no group of a schema can have this name (2.1), and no host id has a `_`.
pub const RUN_GROUP: &str = "pupitre_run";

/// The arguments of `runner.ansible_playbook` (4.17), as separate items. They hold paths, flags
/// and the fixed name of the group of the run only.
pub fn command_args(folders: &RunFolders, playbook: &Path, check: bool, has_params: bool) -> Vec<String> {
    let snap = folders.snapshot();
    let p = |path: PathBuf| path.to_string_lossy().into_owned();
    let mut args = vec![
        "--inventory".to_string(),
        p(snap.join("inventory/hosts.yml")),
        "--limit".to_string(),
        RUN_GROUP.to_string(),
        "--extra-vars".to_string(),
        format!("@{}", p(snap.join("vars/run.json"))),
    ];
    if has_params {
        args.push("--extra-vars".to_string());
        args.push(format!("@{}", p(snap.join(crate::snapshot::PARAMS_FILE))));
    }
    args.push("--extra-vars".to_string());
    args.push(format!("@{}", p(folders.run.join("run-vars.json"))));
    if check {
        args.push("--check".to_string());
        args.push("--diff".to_string());
    }
    args.push(p(playbook.to_path_buf()));
    args
}

/// The environment of the child (4.18): exactly these variables, and no other.
pub fn child_environment(settings: &RunnerSettings, folders: &RunFolders, phase: Phase) -> Vec<(String, String)> {
    let p = |path: PathBuf| path.to_string_lossy().into_owned();
    vec![
        ("PATH".to_string(), settings.path.clone()),
        ("HOME".to_string(), p(folders.run.join("home"))),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("LC_ALL".to_string(), "C.UTF-8".to_string()),
        ("ANSIBLE_CONFIG".to_string(), p(folders.run.join("ansible.cfg"))),
        ("ANSIBLE_LOCAL_TEMP".to_string(), p(folders.run.join("tmp"))),
        ("PYTHONDONTWRITEBYTECODE".to_string(), "1".to_string()),
        ("PUPITRE_EVENTS_FILE".to_string(), p(folders.run.join("events.jsonl"))),
        ("PUPITRE_EVENTS_PHASE".to_string(), phase.name().to_string()),
    ]
}

/// The text of `<run>/ansible.cfg` (4.15).
pub fn ansible_cfg(settings: &RunnerSettings, folders: &RunFolders) -> String {
    let snap = folders.snapshot();
    format!(
        "[defaults]\n\
         inventory = {inventory}\n\
         callback_plugins = {plugins}\n\
         callbacks_enabled = pupitre_events\n\
         retry_files_enabled = False\n\
         host_key_checking = True\n\
         remote_user = {user}\n\
         private_key_file = {key}\n\
         forks = {forks}\n\
         local_tmp = {tmp}\n\
         \n\
         [ssh_connection]\n\
         ssh_args = -o UserKnownHostsFile={known} -o GlobalKnownHostsFile=/dev/null -o StrictHostKeyChecking=yes -o CheckHostIP=no -o IdentitiesOnly=yes -o ForwardAgent=no -o PasswordAuthentication=no\n",
        inventory = snap.join("inventory/hosts.yml").display(),
        plugins = snap.join("callback_plugins").display(),
        user = settings.ssh_user,
        key = settings.ssh_key_file.display(),
        forks = settings.forks,
        tmp = folders.run.join("tmp").display(),
        known = settings.known_hosts_file.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn settings() -> RunnerSettings {
        RunnerSettings {
            work_dir: "/var/lib/pupitre/runner".into(),
            secrets_dir: "/dev/shm/pupitre".into(),
            ansible_playbook: "/usr/bin/ansible-playbook".into(),
            path: "/usr/local/bin:/usr/bin:/bin".into(),
            ssh_user: "deploy".into(),
            ssh_key_file: "/run/secrets/ssh/id_ed25519".into(),
            known_hosts_file: "/run/secrets/ssh/known_hosts".into(),
            releases_dir: None,
            forks: 5,
            max_parallel_runs: 1,
            check_max_age: Duration::from_secs(1800),
            run_timeout: Duration::from_secs(7200),
            cancel_grace: Duration::from_secs(30),
            lease_ttl: Duration::from_secs(30),
            max_log_bytes: 16 << 20,
            max_events: 100_000,
            max_event_bytes: 65_536,
            runs_keep: 10_000,
            poll_interval: Duration::from_secs(5),
        }
    }

    #[test]
    fn the_command_line_of_4_17_holds_no_value() {
        let f = RunFolders::of(&settings(), "r1");
        let args = command_args(&f, Path::new("/srv/ansible/deploy.yml"), true, true);
        assert_eq!(
            args,
            vec![
                "--inventory",
                "/var/lib/pupitre/runner/runs/r1/snapshot/inventory/hosts.yml",
                "--limit",
                "pupitre_run",
                "--extra-vars",
                "@/var/lib/pupitre/runner/runs/r1/snapshot/vars/run.json",
                "--extra-vars",
                "@/var/lib/pupitre/runner/runs/r1/snapshot/vars/params.yml",
                "--extra-vars",
                "@/var/lib/pupitre/runner/runs/r1/run-vars.json",
                "--check",
                "--diff",
                "/srv/ansible/deploy.yml",
            ]
        );
        let apply = command_args(&f, Path::new("/srv/ansible/deploy.yml"), false, false);
        assert!(!apply.iter().any(|a| a == "--check" || a.contains("params.yml")));
    }

    #[test]
    fn the_environment_of_4_18_is_closed() {
        let f = RunFolders::of(&settings(), "r1");
        let env = child_environment(&settings(), &f, Phase::Render);
        let names: Vec<&str> = env.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "PATH",
                "HOME",
                "LANG",
                "LC_ALL",
                "ANSIBLE_CONFIG",
                "ANSIBLE_LOCAL_TEMP",
                "PYTHONDONTWRITEBYTECODE",
                "PUPITRE_EVENTS_FILE",
                "PUPITRE_EVENTS_PHASE"
            ]
        );
        assert!(!names.contains(&"SSH_AUTH_SOCK"));
        assert_eq!(env.last().map(|(_, v)| v.as_str()), Some("render"));
    }

    #[test]
    fn ansible_cfg_pins_the_host_keys() {
        let f = RunFolders::of(&settings(), "r1");
        let cfg = ansible_cfg(&settings(), &f);
        assert!(cfg.contains("StrictHostKeyChecking=yes"));
        // The host keys are found by the alias of the host id only (4.12, 7.8).
        assert!(cfg.contains("-o CheckHostIP=no"));
        assert!(cfg.contains("ForwardAgent=no"));
        assert!(cfg.contains("UserKnownHostsFile=/run/secrets/ssh/known_hosts"));
        assert!(cfg.contains("host_key_checking = True"));
    }
}
