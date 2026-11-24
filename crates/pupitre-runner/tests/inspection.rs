// SPDX-License-Identifier: MIT OR Apache-2.0
//! 7.8: a child of the runner node cannot read the environment of the process of Pupitre. The
//! test is alone in its binary, because the flag stays on the whole test process.

use pupitre_runner::process::forbid_inspection;

fn child_reads_my_environment() -> bool {
    let path = format!("/proc/{}/environ", std::process::id());
    let out = std::process::Command::new("/bin/cat").arg(path).output().expect("start /bin/cat");
    out.status.success() && !out.stdout.is_empty()
}

#[test]
fn a_child_cannot_read_the_environment_after_the_flag() {
    assert!(child_reads_my_environment(), "before the flag, a child of the same user reads the environment");
    forbid_inspection().unwrap();
    assert_eq!(rustix::process::dumpable_behavior().unwrap(), rustix::process::DumpableBehavior::NotDumpable);
    assert!(!child_reads_my_environment(), "after the flag, a child cannot read the environment");
    // The file `stat` stays readable by all: the probe of 4.29 still works.
    assert!(pupitre_runner::process::read_stat(std::process::id()).is_some());
    let mut child = std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap();
    assert!(pupitre_runner::process::read_stat(child.id()).is_some());
    child.kill().unwrap();
    child.wait().unwrap();
}
