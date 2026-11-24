// SPDX-License-Identifier: MIT OR Apache-2.0
//! The strict reader of the runner reads each line that a real Ansible wrote with the callback
//! plugin (4.20). The files come from `tests/ansible/run.sh`, which runs `ansible-playbook` in a
//! throwaway Docker container. Without the files, the test skips with a message.

use std::path::Path;

use pupitre_runner::events::{CallbackEvent, HostStatus, read_line};

fn event_files(out: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(versions) = std::fs::read_dir(out) else { return files };
    for version in versions.flatten() {
        if let Ok(entries) = std::fs::read_dir(version.path()) {
            files.extend(entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jsonl")));
        }
    }
    files.sort();
    files
}

#[test]
fn the_reader_accepts_each_line_of_a_real_ansible() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ansible/out");
    let files = event_files(&out);
    if files.is_empty() {
        eprintln!(
            "SKIPPED: no events of a real Ansible. Run crates/pupitre-runner/tests/ansible/run.sh first (it needs Docker)."
        );
        return;
    }
    let mut statuses = std::collections::BTreeSet::new();
    let mut omitted = 0;
    for file in &files {
        let bytes = std::fs::read(file).unwrap();
        assert!(bytes.ends_with(b"\n"), "{}: the file ends with a line end", file.display());
        for (n, line) in bytes[..bytes.len() - 1].split(|b| *b == b'\n').enumerate() {
            let event = read_line(line, 65_536).unwrap_or_else(|| {
                panic!("{}:{}: the strict reader refuses {}", file.display(), n + 1, String::from_utf8_lossy(line))
            });
            if let CallbackEvent::HostResult { status, omitted: o, .. } = &event {
                statuses.insert(status.name());
                omitted += usize::from(*o);
            }
        }
    }
    // Over the files, each status of 4.20 occurs, and a text above 16,384 bytes is marked.
    for status in
        [HostStatus::Ok, HostStatus::Changed, HostStatus::Failed, HostStatus::Skipped, HostStatus::Unreachable]
    {
        assert!(statuses.contains(status.name()), "no host_result with the status {}", status.name());
    }
    assert!(omitted >= 2, "a diff and a message above 16,384 bytes are marked omitted");
}
