// SPDX-License-Identifier: MIT OR Apache-2.0
//! The modes of 2.32 hold under a strict umask. This test has its own file: its own process
//! has the umask, so no other test sees it.

mod files_support;

use files_support::*;
use pupitre_core::run::RunStatus;
use pupitre_store::{Precondition, Store};
use rustix::fs::Mode;

#[tokio::test]
async fn the_modes_hold_under_a_strict_umask() {
    let fx = Fixture::new();
    let old = rustix::process::umask(Mode::from_raw_mode(0o277));
    let result = async {
        let store = fx.open();
        store.put(&key("environments/p"), &env_doc("P"), &Precondition::Create, &meta("a", "")).await.unwrap();
        store.put_secret(&unit_scope("p", "r"), &name("a.b"), &value("12345678"), &meta("a", "")).await.unwrap();
        let id = run_id(0, 1);
        store.create_run(&run_record(id, "p", RunStatus::Queued), 10, None).await.unwrap();
        store.append_output(&id, 0, b"x").await.unwrap();
        store
    }
    .await;
    rustix::process::umask(old);
    drop(result);
    for (root, file, dir) in [(fx.docs(), 0o644, 0o755), (fx.secrets(), 0o600, 0o700), (fx.runs(), 0o640, 0o750)] {
        for p in entries_below(&root) {
            let is_dir = std::fs::symlink_metadata(&p).unwrap().is_dir();
            assert_eq!(mode_of(&p), if is_dir { dir } else { file }, "{}", p.display());
        }
    }
}
