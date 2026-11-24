// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contract suite of 2.43 on the backend `postgres`. Each test needs the server of
//! `tests/ci/store-pg.sh`, so it is ignored by default. The script runs it on PostgreSQL 15 and 18.
#![cfg(feature = "conformance")]

use pupitre_store::conformance as suite;
use pupitre_store::postgres::testing::PgHarness;

macro_rules! cases {
    ($($name:ident => $case:path),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            #[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
            async fn $name() {
                $case(&PgHarness::from_env()).await;
            }
        )*
    };
}

cases! {
    s01_create_and_get => suite::s01_create_and_get,
    s02_save_and_conflict => suite::s02_save_and_conflict,
    s03_two_writers_one_winner => suite::s03_two_writers_one_winner,
    s04_create_exists_and_after_delete => suite::s04_create_exists_and_after_delete,
    s05_delete_and_poll => suite::s05_delete_and_poll,
    s06_history_and_version => suite::s06_history_and_version,
    s07_history_keep => suite::s07_history_keep,
    s08_crash_between_version_and_document => suite::s08_crash_between_version_and_document,
    s09_poll => suite::s09_poll,
    s10_read_environment_is_consistent => suite::s10_read_environment_is_consistent,
    s11_secrets => suite::s11_secrets,
    s12_fingerprint_vector => suite::s12_fingerprint_vector,
    s13_wrong_key_stops_the_open => suite::s13_wrong_key_stops_the_open,
    s13_same_key_opens => suite::s13_same_key_opens,
    s14_queue_and_check_use => suite::s14_queue_and_check_use,
    s15_update_run_transitions => suite::s15_update_run_transitions,
    s16_output_and_events => suite::s16_output_and_events,
    s17_leases => suite::s17_leases,
    s18_prune_runs => suite::s18_prune_runs,
    s20_old_schema_and_migrate => suite::s20_old_schema_and_migrate,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
async fn s19_transfer_postgres_to_postgres() {
    let harness = PgHarness::from_env();
    suite::s19_transfer(&harness, &harness).await;
}

/// Each case of the table `CASES` has a test in this file, so a new case cannot be left out.
#[test]
fn each_case_of_2_43_has_a_test() {
    let source = include_str!("contract_postgres.rs");
    for (id, _) in suite::CASES {
        let prefix = id.to_lowercase();
        assert!(
            source.lines().any(|l| {
                let l = l.trim_start();
                l.starts_with(&format!("{prefix}_")) || l.starts_with(&format!("async fn {prefix}_"))
            }),
            "no test for the case {id}"
        );
    }
}
