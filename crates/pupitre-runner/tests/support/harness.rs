// SPDX-License-Identifier: MIT OR Apache-2.0
//! A runner node on a fake process control and an in-memory store.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use pupitre_core::ids::RunId;
use pupitre_core::node::SecretScope;
use pupitre_core::run::{RunKind, RunMode, RunRecord, RunStatus};
use pupitre_runner::child::RunFolders;
use pupitre_runner::executor::{Options, Timing, start_with};
use pupitre_runner::{RunnerError, RunnerHandle, RunnerSettings, Wake};

use super::fake_process::{FakeProcess, Script};
use super::fixture::{self, Node1};
use super::mem_store::MemStore;
use super::wait_for;
use pupitre_runner::process::Exit;

/// The code that the fake end of the process got (4.8). `-1` means that nobody called it.
pub static EXIT_CODE: AtomicI32 = AtomicI32::new(-1);

fn fake_exit(code: i32) {
    EXIT_CODE.store(code, Ordering::SeqCst);
}

pub struct H {
    pub dir: tempfile::TempDir,
    pub node: Node1,
    pub store: Arc<MemStore>,
    pub process: FakeProcess,
    pub handle: Option<RunnerHandle>,
    pub wake: Wake,
    pub timing: Timing,
}

/// A script that ends with the code 0 after 20 ms.
pub fn quick() -> Script {
    Script { exit: Some((Duration::from_millis(20), Exit::Code(0))), ..Script::default() }
}

/// A script that never ends by itself.
pub fn forever() -> Script {
    Script::default()
}

pub fn stats_line(hosts: &[(&str, u64, u64)]) -> String {
    let items: Vec<String> = hosts
        .iter()
        .map(|(h, failed, unreachable)| {
            format!(
                r#""{h}":{{"changed":1,"failed":{failed},"ignored":0,"ok":2,"rescued":0,"skipped":0,"unreachable":{unreachable}}}"#
            )
        })
        .collect();
    format!(r#"{{"hosts":{{{}}},"kind":"stats","v":1}}"#, items.join(","))
}

impl H {
    pub fn new() -> H {
        let dir = tempfile::tempdir().unwrap();
        let node = fixture::node(dir.path());
        let store = Arc::new(MemStore::new());
        for doc in fixture::documents() {
            store.add_doc(doc);
        }
        for (scope, name, value, fp) in fixture::secrets() {
            store.put_secret_value(scope, name.as_str(), &value, &fp);
        }
        H {
            dir,
            node,
            store,
            process: FakeProcess::new(),
            handle: None,
            wake: Wake::default(),
            timing: Timing { pump: Duration::from_millis(50), probe: Duration::from_millis(50) },
        }
    }

    pub fn settings(&mut self) -> &mut RunnerSettings {
        &mut self.node.settings
    }

    pub async fn try_start(&mut self) -> Result<(), RunnerError> {
        let handle = start_with(
            self.node.settings.clone(),
            Arc::new(self.node.caps.clone()),
            self.store.clone(),
            self.wake.clone(),
            self.process.clone(),
            Options { timing: self.timing, exit: fake_exit },
        )
        .await?;
        self.handle = Some(handle);
        Ok(())
    }

    pub async fn start(&mut self) {
        self.try_start().await.expect("the runner starts");
    }

    pub async fn stop(&mut self) {
        if let Some(h) = self.handle.take() {
            h.stop(Duration::from_secs(2)).await;
        }
    }

    pub fn node_id(&self) -> String {
        std::fs::read_to_string(self.node.settings.work_dir.join("node-id")).unwrap().trim().to_string()
    }

    pub fn folders(&self, id: &RunId) -> RunFolders {
        RunFolders::of(&self.node.settings, &id.to_string())
    }

    pub fn queue(&self, run: RunRecord) {
        self.store.insert_run(run);
        self.wake.wake();
    }

    pub fn check_run(&self, n: u8) -> RunRecord {
        fixture::run_record(
            fixture::run_id(n),
            RunKind::Deploy,
            RunMode::Check,
            "environment:production/unit:relay",
            &["host-a", "host-b"],
            "deploy-relay",
        )
    }

    pub async fn wait_status(&self, id: &RunId, status: RunStatus) -> RunRecord {
        let store = self.store.clone();
        let id = *id;
        wait_for(&format!("the run {id} to be {}", status.name()), 20, move || {
            store.run(&id).is_some_and(|r| r.status == status)
        })
        .await;
        self.store.run(&id).unwrap()
    }

    pub fn secrets_dir_of(&self, id: &RunId) -> PathBuf {
        self.folders(id).secrets
    }

    pub fn secret_scope() -> SecretScope {
        SecretScope::Environment(fixture::env_id())
    }
}
