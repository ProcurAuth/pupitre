// SPDX-License-Identifier: MIT OR Apache-2.0
//! The store commands of 6.24 on PostgreSQL, through the command line: `migrate`, `export` and
//! `copy-store`, with configuration files. The test needs the server of `tests/ci/store-pg.sh`
//! and the two databases that the script makes, so it is ignored by default.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hclconf::Profile;
use hclconf::env::CapturedEnvironment;
use pupitre::cli;
use pupitre::config::{ReferenceFormat, render_reference};
use pupitre_core::doc::{CanonicalDocument, DocKey};
use pupitre_core::ids::{EnvId, KeyPath, Subject, UnitId};
use pupitre_core::json::{self, Json};
use pupitre_core::node::SecretScope;
use pupitre_core::time::UtcMillis;
use pupitre_store::postgres::{PgOptions, PgPassword, PgStore, PgTls, SslMode};
use pupitre_store::secret::{SecretKey, SecretValue};
use pupitre_store::{Message, Precondition, Store, StoreError, WriteMeta};

const KEY_ONE: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const KEY_TWO: &str = "abababababababababababababababababababababababababababababababab";
const SCHEMA: &str = r#"{"format": 1, "group": "relay_hosts", "variable": "relay_config", "keys": [
  {"description": "The password.", "kind": "text", "max": 64, "min": 8, "path": "storage.password", "rule": "any", "secret": true}
]}"#;

fn var(name: &str) -> String {
    std::env::var(format!("PUPITRE_TEST_{name}")).unwrap_or_else(|_| {
        panic!("the variable PUPITRE_TEST_{name} is not set: run this test with tests/ci/store-pg.sh")
    })
}

struct Account {
    user: String,
    password: String,
}

fn owner() -> Account {
    Account { user: var("PG_OWNER_USER"), password: var("PG_OWNER_PASSWORD") }
}

fn app() -> Account {
    Account { user: var("PG_APP_USER"), password: var("PG_APP_PASSWORD") }
}

/// Write a configuration file for one database, one account and one key. The password and the
/// key are files, as in 6.5.
fn config(dir: &Path, name: &str, database: &str, who: &Account, key: &str) -> PathBuf {
    let write = |file: &str, text: &str| -> String {
        let path = dir.join(file);
        std::fs::write(&path, text).expect("a file");
        path.to_string_lossy().into_owned()
    };
    let schema = write("relay.json", SCHEMA);
    let key_file = write(&format!("{name}.key"), &format!("{key}\n"));
    let password_file = write(&format!("{name}.password"), &who.password);
    let text = render_reference(ReferenceFormat::Hcl, Profile::Local);
    let start = text.find("\nstorage {").expect("the block storage") + 1;
    let end = start + text[start..].find("\n}\n").expect("the end of the block") + 3;
    let storage = format!(
        "storage {{\n  backend         = \"postgres\"\n  secret_key_file = \"{key_file}\"\n\n  postgres {{\n    host          = \"127.0.0.1\"\n    port          = {}\n    database      = \"{database}\"\n    user          = \"{}\"\n    password_file = \"{password_file}\"\n    sslmode       = \"verify-full\"\n\n    tls {{\n      ca_file     = \"{}\"\n      server_name = \"{}\"\n    }}\n  }}\n}}\n",
        var("PG_PORT"),
        who.user,
        var("PG_CA_FILE"),
        var("PG_SERVER_NAME"),
    );
    let text =
        format!("{}{}{}", &text[..start], storage, &text[end..]).replace("/srv/ansible/schemas/relay.json", &schema);
    let path = dir.join(format!("{name}.hcl"));
    std::fs::write(&path, text).expect("the configuration file");
    path
}

fn run(args: &[&str]) -> u8 {
    let env = CapturedEnvironment::from_pairs(Vec::<(String, String)>::new());
    cli::run(args.iter().map(OsString::from), &env)
}

fn path(p: &Path) -> &str {
    p.to_str().expect("a path of text")
}

fn options(database: &str, key: &str) -> PgOptions {
    let who = app();
    PgOptions {
        host: "127.0.0.1".to_string(),
        port: var("PG_PORT").parse().expect("a port"),
        database: database.to_string(),
        user: who.user,
        password: PgPassword(who.password),
        sslmode: SslMode::VerifyFull,
        tls: Some(PgTls {
            ca_pem: std::fs::read(var("PG_CA_FILE")).expect("the CA file"),
            server_name: var("PG_SERVER_NAME"),
        }),
        max_connections: 4,
        connect_timeout: Duration::from_secs(5),
        statement_timeout: Duration::from_secs(10),
        poll_interval: Duration::from_secs(5),
        history_keep: 100,
        secret_key: SecretKey::parse(key).expect("a key"),
        max_document_bytes: 262_144,
    }
}

fn meta(by: &str, message: &str) -> WriteMeta {
    WriteMeta {
        saved_by: Subject::parse(by).expect("subject"),
        saved_at: UtcMillis::now(),
        message: Message::parse(message).expect("message"),
    }
}

fn doc(label: &str) -> CanonicalDocument {
    CanonicalDocument::from_bytes(json::to_canonical(&Json::object([
        ("format", Json::Int(1)),
        ("label", Json::str(label)),
    ])))
}

fn scope() -> SecretScope {
    SecretScope::Unit(EnvId::parse("production").expect("id"), UnitId::parse("relay").expect("id"))
}

#[test]
#[ignore = "needs PostgreSQL: run tests/ci/store-pg.sh"]
fn migrate_copy_store_and_export_work_from_the_command_line() {
    let dir = tempfile::tempdir().expect("a folder");
    let (db_a, db_b) = (var("CLI_DB_A"), var("CLI_DB_B"));
    let a_owner = config(dir.path(), "a-owner", &db_a, &owner(), KEY_ONE);
    let b_owner = config(dir.path(), "b-owner", &db_b, &owner(), KEY_TWO);
    let a_app = config(dir.path(), "a-app", &db_a, &app(), KEY_ONE);
    let b_app = config(dir.path(), "b-app", &db_b, &app(), KEY_TWO);
    let b_wrong_key = config(dir.path(), "b-wrong", &db_b, &app(), KEY_ONE);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a runtime");

    // `migrate` makes the schema with the account of the owner, and a second run changes nothing.
    assert_eq!(run(&["migrate", "--config", path(&a_owner)]), 0);
    assert_eq!(run(&["migrate", "--config", path(&a_owner)]), 0);
    assert_eq!(run(&["migrate", "--config", path(&b_owner)]), 0);
    // A wrong key stops `migrate` (2.26). The account of `serve` cannot migrate (2.36).
    let a_wrong = config(dir.path(), "a-wrong", &db_a, &owner(), KEY_TWO);
    assert_eq!(run(&["migrate", "--config", path(&a_wrong)]), 1);
    assert_eq!(run(&["migrate", "--config", path(&a_app)]), 0, "the schema is current: nothing to change");
    // A configuration that does not load stops the command with the code 1.
    assert_eq!(run(&["migrate", "--config", path(&dir.path().join("missing.hcl"))]), 1);

    // Fill the store A: two versions, a deleted key, a secret with two events.
    runtime.block_on(async {
        let store = PgStore::open(options(&db_a, KEY_ONE)).await.expect("open A");
        let key = DocKey::Environment(EnvId::parse("production").expect("id"));
        let (v1, v2) = (doc("One"), doc("Two"));
        store.put(&key, &v1, &Precondition::Create, &meta("alice", "first")).await.expect("create");
        store.put(&key, &v2, &Precondition::Match(v1.etag), &meta("bob", "second")).await.expect("save");
        let gone = DocKey::Common(EnvId::parse("production").expect("id"));
        let c = doc("Common");
        store.put(&gone, &c, &Precondition::Create, &meta("alice", "")).await.expect("create");
        store.delete(&gone, &c.etag, &meta("alice", "removed")).await.expect("delete");
        let name = KeyPath::parse("storage.password").expect("path");
        let (one, two) =
            (SecretValue::new(b"first-secret-value".to_vec()), SecretValue::new(b"second-secret-value".to_vec()));
        store.put_secret(&scope(), &name, &one, &meta("alice", "")).await.expect("secret");
        store.put_secret(&scope(), &name, &two, &meta("bob", "")).await.expect("secret");
    });

    // `copy-store` into the empty store B. A wrong key of the target stops it. A second copy is refused.
    assert_eq!(run(&["copy-store", "--from", path(&a_app), "--to", path(&b_wrong_key)]), 1);
    assert_eq!(run(&["copy-store", "--from", path(&a_app), "--to", path(&b_app)]), 0);
    assert_eq!(run(&["copy-store", "--from", path(&a_app), "--to", path(&b_app)]), 1, "the target is not empty");
    runtime.block_on(async {
        let a = PgStore::open(options(&db_a, KEY_ONE)).await.expect("open A");
        let b = PgStore::open(options(&db_b, KEY_TWO)).await.expect("open B with its own key");
        let (la, lb) = (a.load().await.expect("load A"), b.load().await.expect("load B"));
        assert_eq!(la.docs, lb.docs, "the same documents, etags and versions");
        let key = DocKey::Environment(EnvId::parse("production").expect("id"));
        let (ha, hb) =
            (a.history(&key, None, 10).await.expect("history"), b.history(&key, None, 10).await.expect("history"));
        assert_eq!((ha.len(), ha), (2, hb));
        let sa = a.read_secrets(std::slice::from_ref(&scope())).await.expect("read");
        let sb = b.read_secrets(&[scope()]).await.expect("read");
        assert_eq!(sa.len(), 1);
        assert_eq!(sa[0].2.expose(), sb[0].2.expose());
        // B has its own key: the key of A does not open B.
        let wrong = PgStore::open(options(&db_b, KEY_ONE)).await;
        assert!(matches!(wrong, Err(StoreError::KeyMismatch)), "{wrong:?}");
    });

    // `export` writes the documents and the versions into an empty folder, and no secret.
    let out = dir.path().join("export");
    assert_eq!(run(&["export", "--config", path(&a_app), "--to", path(&out)]), 0);
    assert!(out.join("pupitre-store.json").is_file());
    assert!(out.join("environments/production/environment.json").is_file());
    assert!(out.join(".history/environments/production/0000000002.json").is_file());
    assert!(!out.join("environments/production/common.json").exists(), "a deleted key has no file");
    assert_eq!(run(&["export", "--config", path(&a_app), "--to", path(&out)]), 1, "the folder is not empty");
}
