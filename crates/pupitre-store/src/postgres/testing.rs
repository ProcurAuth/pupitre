// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hooks and helpers for the tests of the backend `postgres`. This module exists in the test
//! profile and behind the feature `conformance`. The production code does not use it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use super::PgStore;
use crate::types::StoreError;

/// Faults that a test sets on a store. Each fault fires one time.
#[derive(Debug, Default)]
pub struct Faults {
    after_version: AtomicBool,
    hold: Mutex<Option<Hold>>,
}

/// A write that a test holds after it took its change number and before its commit.
#[derive(Debug, Clone, Default)]
pub struct Hold {
    reached: Arc<Notify>,
    release: Arc<Notify>,
}

impl Hold {
    /// Wait until the held write has its change number and waits for `release`.
    pub async fn reached(&self) {
        self.reached.notified().await;
    }

    /// Let the held write commit.
    pub fn release(&self) {
        self.release.notify_one();
    }
}

impl Faults {
    /// The next write of a document stops after it wrote the version and before it writes the
    /// document (2.19). The transaction then ends with an error and the database keeps nothing.
    pub fn crash_after_version(&self) {
        self.after_version.store(true, Ordering::SeqCst);
    }

    /// The next write of a document waits after it took its change number and before its
    /// commit (2.38), until `Hold::release`. A test uses it to force a commit out of order.
    pub fn hold_before_commit(&self) -> Hold {
        let hold = Hold::default();
        if let Ok(mut slot) = self.hold.lock() {
            *slot = Some(hold.clone());
        }
        hold
    }

    pub(crate) async fn before_commit(&self) {
        let hold = self.hold.lock().ok().and_then(|mut slot| slot.take());
        if let Some(hold) = hold {
            hold.reached.notify_one();
            hold.release.notified().await;
        }
    }

    pub(crate) fn after_version(&self) -> Result<(), StoreError> {
        if self.after_version.swap(false, Ordering::SeqCst) {
            return Err(StoreError::Unavailable("fault: the process stopped after the version".to_string()));
        }
        Ok(())
    }
}

impl PgStore {
    /// The faults of this store.
    pub fn faults(&self) -> &Faults {
        &self.faults
    }
}

#[cfg(feature = "conformance")]
mod harness;
#[cfg(feature = "conformance")]
pub use harness::PgHarness;

use std::time::Duration;

use tokio_postgres::NoTls;

use super::{PgOptions, PgPassword, PgTls, SslMode};
use crate::secret::SecretKey;

/// The server of the tests: a PostgreSQL that `tests/ci/store-pg.sh` starts. The variables
/// `PUPITRE_TEST_PG_*` name it. A test that needs it is `#[ignore]`, and the script runs it.
#[derive(Debug, Clone)]
pub struct PgTestEnv {
    /// The host of the server.
    pub host: String,
    /// The port of the server.
    pub port: u16,
    admin: (String, String),
    owner: (String, String),
    app: (String, String),
    /// The CA that signed the certificate of the server.
    pub ca_pem: Vec<u8>,
    /// A CA that signed nothing.
    pub wrong_ca_pem: Vec<u8>,
    /// The name in the certificate of the server.
    pub server_name: String,
    /// The major version of the server.
    pub version: String,
}

fn var(name: &str) -> String {
    std::env::var(format!("PUPITRE_TEST_PG_{name}")).unwrap_or_else(|_| {
        panic!("the variable PUPITRE_TEST_PG_{name} is not set: run these tests with tests/ci/store-pg.sh")
    })
}

fn file(name: &str) -> Vec<u8> {
    let path = var(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read the file of PUPITRE_TEST_PG_{name}: {e}"))
}

impl PgTestEnv {
    /// Read the variables `PUPITRE_TEST_PG_*`. It panics with the name of a missing variable.
    pub fn require() -> PgTestEnv {
        PgTestEnv {
            host: var("HOST"),
            port: var("PORT").parse().expect("PUPITRE_TEST_PG_PORT is a port"),
            admin: (var("ADMIN_USER"), var("ADMIN_PASSWORD")),
            owner: (var("OWNER_USER"), var("OWNER_PASSWORD")),
            app: (var("APP_USER"), var("APP_PASSWORD")),
            ca_pem: file("CA_FILE"),
            wrong_ca_pem: file("WRONG_CA_FILE"),
            server_name: var("SERVER_NAME"),
            version: var("VERSION"),
        }
    }

    /// The options of a connection as one account, with `verify-full` and the CA of the test.
    pub fn options(&self, database: &str, user: &str, password: &str, key: &SecretKey) -> PgOptions {
        PgOptions {
            host: self.host.clone(),
            port: self.port,
            database: database.to_string(),
            user: user.to_string(),
            password: PgPassword(password.to_string()),
            sslmode: SslMode::VerifyFull,
            tls: Some(PgTls { ca_pem: self.ca_pem.clone(), server_name: self.server_name.clone() }),
            max_connections: 8,
            connect_timeout: Duration::from_secs(5),
            statement_timeout: Duration::from_secs(10),
            poll_interval: Duration::from_secs(5),
            history_keep: 100,
            secret_key: key.clone(),
            max_document_bytes: 262_144,
        }
    }

    /// A session as the administrator of the server, to make a database.
    async fn session(&self, database: &str, user: &str, password: &str) -> tokio_postgres::Client {
        let mut config = tokio_postgres::Config::new();
        config.host(&self.host).port(self.port).dbname(database).user(user).password(password);
        let (client, connection) = config.connect(NoTls).await.expect("a session on the test server");
        tokio::spawn(async move { drop(connection.await) });
        client
    }

    /// Make a new database that the owner account owns, and that no table fills yet.
    pub async fn new_database(&self) -> TestDb {
        let mut random = [0u8; 6];
        getrandom::fill(&mut random).expect("random bytes");
        let name = format!("pt_{}", pupitre_core::etag::to_hex(&random));
        let admin = self.session("postgres", &self.admin.0, &self.admin.1).await;
        admin
            .batch_execute(&format!("CREATE DATABASE {name} OWNER {}", self.owner.0))
            .await
            .expect("create the database");
        TestDb { env: self.clone(), name }
    }
}

/// One database of a test.
#[derive(Debug, Clone)]
pub struct TestDb {
    /// The server.
    pub env: PgTestEnv,
    /// The name of the database.
    pub name: String,
}

impl TestDb {
    /// The options of the owner account: the one of `migrate`.
    pub fn owner_options(&self, key: &SecretKey) -> PgOptions {
        self.env.options(&self.name, &self.env.owner.0, &self.env.owner.1, key)
    }

    /// The options of the account of `serve`: it has the rights of 2.36 and no more.
    pub fn app_options(&self, key: &SecretKey) -> PgOptions {
        self.env.options(&self.name, &self.env.app.0, &self.env.app.1, key)
    }

    /// A session in this database as the owner.
    pub async fn owner_session(&self) -> tokio_postgres::Client {
        self.env.session(&self.name, &self.env.owner.0, &self.env.owner.1).await
    }

    /// A session in this database as the administrator.
    pub async fn admin_session(&self) -> tokio_postgres::Client {
        self.env.session(&self.name, &self.env.admin.0, &self.env.admin.1).await
    }

    /// A session in this database as the account of `serve`.
    pub async fn app_session(&self) -> tokio_postgres::Client {
        self.env.session(&self.name, &self.env.app.0, &self.env.app.1).await
    }

    /// Give the account of `serve` the rights of 2.36: `SELECT`, `INSERT`, `UPDATE` and
    /// `DELETE` on each table, and `USAGE` on each sequence. It gets no other right.
    pub async fn grant_app(&self) {
        let owner = self.owner_session().await;
        owner
            .batch_execute(&format!(
                "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO {app}; \
                 GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO {app}",
                app = self.env.app.0
            ))
            .await
            .expect("grant the rights of 2.36");
    }

    /// Run `migrate` as the owner, give the rights, and open the store as the account of `serve`.
    pub async fn ready(&self, key: &SecretKey) -> PgStore {
        super::migrate(&self.owner_options(key)).await.expect("migrate");
        self.grant_app().await;
        PgStore::open(self.app_options(key)).await.expect("open as the account of serve")
    }
}
