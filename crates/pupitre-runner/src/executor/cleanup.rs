// SPDX-License-Identifier: MIT OR Apache-2.0
//! The cleanup of the runner node (4.31): old runs, and the folders of the runs.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use pupitre_core::ids::RunId;
use pupitre_store::Store;
use pupitre_store::types::StoreError;

use crate::executor::log;
use crate::executor::shared::Shared;
use crate::process::ProcessControl;
use crate::snapshot::disk;

/// The run ids that name a folder of a folder.
fn run_folders(dir: &Path) -> Vec<(RunId, std::path::PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let id = RunId::parse(e.file_name().to_str()?).ok()?;
            Some((id, e.path()))
        })
        .collect()
}

async fn remove(path: std::path::PathBuf) {
    let shown = path.display().to_string();
    match tokio::task::spawn_blocking(move || disk::remove_tree(&path)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => log::store_error(&format!("remove the folder {shown}"), &e),
        Err(e) => log::store_error("remove a folder", &e),
    }
}

/// After the end of a run (4.31): `prune_runs(runner.runs_keep)`, then the folder of each run
/// that the store no longer holds. A folder of a run with the status `queued` or `running` stays.
pub(crate) async fn after_run<S: Store, P: ProcessControl>(sh: &Shared<S, P>) {
    let removed = match sh.store.prune_runs(sh.settings.runs_keep).await {
        Ok(ids) => ids,
        Err(e) => {
            log::store_error("prune_runs", &e);
            return;
        }
    };
    let active: BTreeSet<RunId> = sh.active_runs().into_iter().map(|(id, _)| id).collect();
    for id in removed.into_iter().filter(|id| !active.contains(id)) {
        remove(sh.settings.work_dir.join("runs").join(id.to_string())).await;
    }
}

/// At start (4.31), remove two sets of folders. The first set is each folder of
/// `runner.secrets_dir` that is not the folder of an adopted run. The second set is each folder
/// of a run that the store does not hold.
pub(crate) async fn at_start<S: Store, P: ProcessControl>(sh: &Shared<S, P>, adopted: &BTreeSet<RunId>) {
    for (id, path) in run_folders(&sh.settings.secrets_dir) {
        if !adopted.contains(&id) {
            remove(path).await;
        }
    }
    for (id, path) in run_folders(&sh.settings.work_dir.join("runs")) {
        if adopted.contains(&id) {
            continue;
        }
        match sh.store.get_run(&id).await {
            // The store does not hold the run: its folder is not needed.
            Err(StoreError::NotFound) => remove(path).await,
            Ok(_) => {}
            Err(e) => log::store_error("get_run", &e),
        }
    }
}
