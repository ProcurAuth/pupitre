// SPDX-License-Identifier: MIT OR Apache-2.0
//! The table of keys of Pupitre (6.7 to 6.12). The reference files, `print-config` and the
//! checks all come from this table.

use hclconf::{BlockDesc, Forms, KeyDesc, Kind, Ref, Schema, Term, TextRule};

const AUTHZEN: &[Term] = &[Term { key: "access.mode", equals: "authzen" }];
const FILES: &[Term] = &[Term { key: "storage.backend", equals: "files" }];
const POSTGRES: &[Term] = &[Term { key: "storage.backend", equals: "postgres" }];
const SSL_VERIFY_FULL: &[Term] = &[Term { key: "storage.postgres.sslmode", equals: "verify-full" }];
const TELEMETRY_ON: &[Term] = &[Term { key: "telemetry.enabled", equals: "true" }];
const PLAIN_HTTP_OFF: &[Term] = &[Term { key: "access.pdp.plain_http", equals: "false" }];
const RUNNER_ON: &[Term] = &[Term { key: "runner.enabled", equals: "true" }];
const OPEN_RUNNER_ON: &[Term] =
    &[Term { key: "access.mode", equals: "open" }, Term { key: "runner.enabled", equals: "true" }];
const EXECUTOR: &[Term] =
    &[Term { key: "runner.enabled", equals: "true" }, Term { key: "runner.execute", equals: "true" }];

const fn text(min: usize, max: usize, rule: TextRule) -> Kind {
    Kind::Text { min, max, rule }
}

const fn seconds(min: u64, max: u64) -> Kind {
    Kind::Duration { min_ms: min * 1000, max_ms: max * 1000 }
}

const OPTIONAL: &str = "never (the key is optional)";
const NEVER: &str = "never";

/// The largest integer of a JSON document (2.4): the bound of the integer parameters.
const MAX_SAFE: u64 = 9_007_199_254_740_991;

/// The blocks, in table order. A parent comes before its children.
static BLOCKS: [BlockDesc; 16] = [
    BlockDesc::new("server", "server", "The two listeners and the limits of the HTTP server (5.1, 7.10)."),
    BlockDesc::new("log", "log", "The log level (6.26)."),
    BlockDesc::new("telemetry", "telemetry", "The export of the spans with OTLP (6.29)."),
    BlockDesc::new("storage", "storage", "The store of the documents, the secrets and the runs (section 2)."),
    BlockDesc::new("storage.files", "storage.files", "The backend files: three folders and one writer process (2.31).")
        .when(FILES),
    BlockDesc::new("storage.postgres", "storage.postgres", "The backend postgres (2.36).").when(POSTGRES),
    BlockDesc::new("storage.postgres.tls", "storage.postgres.tls", "The TLS link to PostgreSQL.").when(SSL_VERIFY_FULL),
    BlockDesc::new("access", "access", "The access mode (section 3)."),
    BlockDesc::new("access.identity", "access.identity", "The identity token of the edge (3.4).").when(AUTHZEN),
    BlockDesc::new("access.pdp", "access.pdp", "The link to the policy decision point (3.5).").when(AUTHZEN),
    BlockDesc::new("runner", "runner", "The runs (section 4). The keys of the runner node need `runner.execute = true`."),
    BlockDesc::new("limits", "limits", "The limits of the store (2.42). The block and each key are optional.")
        .manual("never (the block is optional)", NEVER),
    BlockDesc::new("schema", "schema", "One schema of a unit: a file of 2.11. The label is the schema id.")
        .labelled(0, 256, "relay"),
    BlockDesc::new(
        "playbook",
        "playbook",
        "One playbook of the catalog of capabilities (4.2). The label is the playbook id. Only with `runner.enabled = true` (6.14).",
    )
    .labelled(0, 256, "deploy-relay"),
    BlockDesc::new(
        "command",
        "command",
        "One command (4.32). The label is the command id. Only with `runner.enabled = true` (6.14). The reference files hold no command block.",
    )
    .labelled(0, 256, "restart-service"),
    BlockDesc::new("command.param", "command.param", "One parameter of a command (4.33). The label is the parameter name.")
        .labelled(0, 16, "grace"),
];

/// The keys, in table order.
static KEYS: [KeyDesc; 99] = [
    // 6.8: server
    KeyDesc::new("server.listen", Kind::SocketAddr, "The main listener (5.1). The canonical form of an IP address and a port.")
        .refs(Ref::Str("127.0.0.1:8080"), Ref::Str("0.0.0.0:8080")),
    KeyDesc::new("server.ops_listen", Kind::SocketAddr, "The ops listener (5.20). Its port is not the port of `server.listen`.")
        .refs(Ref::Str("127.0.0.1:9090"), Ref::Str("0.0.0.0:9090")),
    KeyDesc::new(
        "server.base_path",
        text(1, 128, TextRule::NoControl),
        "The prefix of the routes of the main listener (5.2). `/`, or a path that starts and ends with `/`. A segment has 1 to 64 characters from `A-Z a-z 0-9 . _ ~ -` and is not `.` or `..`.",
    )
    .both(Ref::Str("/")),
    KeyDesc::new(
        "server.allowed_hosts",
        Kind::TextList { min_items: 1, max_items: 32, item_min: 1, item_max: 261, rule: TextRule::NoControlNoSpace },
        "The values of the header `Host` that the main listener serves (7.4). Each entry is a host name, an IPv4 address or an IPv6 address in brackets, with an optional port. An entry with no port matches each port. A request with another `Host` gets `421 misdirected_request`. Absent: the default is `localhost`, `127.0.0.1`, `[::1]` and the address of `server.listen`, and it applies only when `server.listen` is a loopback address.",
    )
    .manual("`server.listen` is not a loopback address", NEVER)
    .refs(Ref::Absent, Ref::StrList(&["pupitre.example.invalid"]))
    .doc_ref(Ref::StrList(&["pupitre.example.invalid"])),
    KeyDesc::new("server.max_connections", Kind::Integer { min: 16, max: 65_536 }, "The most open connections of the main listener. The default is 1024.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(1024)),
    KeyDesc::new("server.idle_timeout", seconds(1, 600), "Pupitre closes a kept connection that is idle for this time. The default is `75s`.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Str("75s")),
    KeyDesc::new("server.shutdown_grace", seconds(5, 600), "The longest graceful stop. The default is `30s`.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Str("30s")),
    KeyDesc::new("server.header_read_timeout", seconds(1, 60), "The longest read of the request line and the headers. The default is `10s`.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Str("10s")),
    KeyDesc::new("server.body_read_timeout", seconds(1, 300), "The longest read of a body. The default is `30s`.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Str("30s")),
    KeyDesc::new("server.max_request_bytes", Kind::Size { min: 65_536, max: 16_777_216 }, "The largest request body. The default is 1048576.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(1_048_576)),
    KeyDesc::new("server.max_inflight_bodies", Kind::Integer { min: 1, max: 4096 }, "The most request bodies in flight at one time. The default is 64.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(64)),
    KeyDesc::new("server.max_page_items", Kind::Integer { min: 1, max: 1000 }, "The most items of one list page (5.6). The default is 200.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(200)),
    // 6.9: log and telemetry
    KeyDesc::new("log.level", Kind::Enum(&["error", "warn", "info", "debug"]), "The lowest level that Pupitre writes (6.26).")
        .both(Ref::Str("info")),
    KeyDesc::new("telemetry.enabled", Kind::Bool, "The value `true` exports the spans with OTLP (6.29).").both(Ref::Bool(false)),
    KeyDesc::new(
        "telemetry.otlp_endpoint",
        text(1, 2048, TextRule::NoControl),
        "A URL with the scheme `http` or `https`, a host and an optional port, with no path, query, fragment or user info. Pupitre sends to `<endpoint>/v1/traces`.",
    )
    .forms(Forms::Vef)
    .when(TELEMETRY_ON)
    .doc_ref(Ref::Str("http://otel-collector.example.invalid:4318")),
    KeyDesc::new("telemetry.ca_file", Kind::Path, "A PEM file of CA certificates for an `https` endpoint.")
        .when_optional(TELEMETRY_ON, "`telemetry.enabled = true` and an `https` endpoint")
        .forbidden("`telemetry.enabled = false`, or an `http` endpoint"),
    KeyDesc::new("telemetry.service_name", text(1, 64, TextRule::PrintableAscii), "The resource attribute `service.name`.")
        .when(TELEMETRY_ON)
        .doc_ref(Ref::Str("pupitre")),
    KeyDesc::new("telemetry.export_interval", seconds(1, 300), "The period of the export.")
        .when(TELEMETRY_ON)
        .doc_ref(Ref::Str("15s")),
    KeyDesc::new("telemetry.max_queued_spans", Kind::Integer { min: 1, max: 1_000_000 }, "The most spans that wait for the export. Above it, a new span is dropped and counted. The default is 2048.")
        .when_optional(TELEMETRY_ON, "never (the key is optional)")
        .doc_ref(Ref::Int(2048)),
    // 6.10: storage
    KeyDesc::new("storage.backend", Kind::Enum(&["files", "postgres"]), "The backend of the store (section 2).")
        .refs(Ref::Str("files"), Ref::Str("postgres")),
    KeyDesc::new(
        "storage.secret_key",
        text(64, 64, TextRule::PrintableAscii),
        "The key of the store: 64 lower-case hexadecimal characters (2.26). All instances of one store have the same key.",
    )
    .secret()
    .both(Ref::File("/run/secrets/pupitre/store-key")),
    KeyDesc::new("storage.files.path", Kind::Path, "The folder of the documents (2.31). It can be a Git checkout.")
        .refs(Ref::Str("/var/lib/pupitre/data"), Ref::Absent),
    KeyDesc::new("storage.files.secrets_path", Kind::Path, "The folder of the secrets, mode 0700, outside the folder of the documents (2.31, 2.34).")
        .refs(Ref::Str("/var/lib/pupitre/secrets"), Ref::Absent),
    KeyDesc::new("storage.files.runs_path", Kind::Path, "The folder of the runs, outside the folder of the documents (2.31, 2.35).")
        .refs(Ref::Str("/var/lib/pupitre/runs"), Ref::Absent),
    KeyDesc::new("storage.files.rescan_interval", seconds(1, 3600), "The period of the scan for changes outside Pupitre (2.33). The default is `5s`.")
        .when_optional(FILES, "never (the key is optional)")
        .doc_ref(Ref::Str("5s")),
    KeyDesc::new("storage.files.history_keep", Kind::Integer { min: 1, max: 10_000 }, "The versions kept for each key, and the events kept for each secret (2.18). The default is 100.")
        .when_optional(FILES, "never (the key is optional)")
        .doc_ref(Ref::Int(100)),
    KeyDesc::new("storage.postgres.host", text(1, 253, TextRule::HostOrIp), "The host of the PostgreSQL server.")
        .refs(Ref::Absent, Ref::Str("pg.example.invalid")),
    KeyDesc::new("storage.postgres.port", Kind::Integer { min: 1, max: 65_535 }, "The port of the PostgreSQL server.")
        .refs(Ref::Absent, Ref::Int(5432)),
    KeyDesc::new("storage.postgres.database", text(1, 63, TextRule::NoControl), "The name of the database.")
        .refs(Ref::Absent, Ref::Str("pupitre")),
    KeyDesc::new("storage.postgres.user", text(1, 63, TextRule::NoControl), "The name of the account.")
        .forms(Forms::Vef)
        .refs(Ref::Absent, Ref::File("/run/secrets/pg/username")),
    KeyDesc::new("storage.postgres.password", text(1, 1024, TextRule::Any), "The password of the account.")
        .secret()
        .refs(Ref::Absent, Ref::File("/run/secrets/pg/password")),
    KeyDesc::new("storage.postgres.sslmode", Kind::Enum(&["verify-full", "disable"]), "The value `verify-full` checks the certificate and the name.")
        .refs(Ref::Absent, Ref::Str("verify-full")),
    KeyDesc::new("storage.postgres.tls.ca_file", Kind::Path, "A PEM file of CA certificates.")
        .refs(Ref::Absent, Ref::Str("/run/secrets/pg-ca/ca.crt")),
    KeyDesc::new("storage.postgres.tls.server_name", text(1, 253, TextRule::DnsName), "The name that the certificate must hold.")
        .refs(Ref::Absent, Ref::Str("pg.example.invalid")),
    KeyDesc::new("storage.postgres.max_connections", Kind::Integer { min: 2, max: 64 }, "The size of the connection pool (2.37). The default is 8.")
        .when_optional(POSTGRES, "never (the key is optional)")
        .doc_ref(Ref::Int(8)),
    KeyDesc::new("storage.postgres.connect_timeout", seconds(1, 60), "The longest wait for a connection (2.37). The default is `5s`.")
        .when_optional(POSTGRES, "never (the key is optional)")
        .doc_ref(Ref::Str("5s")),
    KeyDesc::new("storage.postgres.statement_timeout", Kind::Duration { min_ms: 100, max_ms: 60_000 }, "The longest statement (2.37). The default is `10s`.")
        .when_optional(POSTGRES, "never (the key is optional)")
        .doc_ref(Ref::Str("10s")),
    KeyDesc::new("storage.postgres.poll_interval", seconds(1, 300), "The period of the poll (2.38). The default is `5s`.")
        .when_optional(POSTGRES, "never (the key is optional)")
        .doc_ref(Ref::Str("5s")),
    KeyDesc::new("storage.postgres.history_keep", Kind::Integer { min: 1, max: 10_000 }, "The versions kept for each key, and the events kept for each secret (2.18). The default is 100.")
        .when_optional(POSTGRES, "never (the key is optional)")
        .doc_ref(Ref::Int(100)),
    // 6.11: access
    KeyDesc::new("access.mode", Kind::Enum(&["open", "authzen"]), "The access mode (3.2). The mode `open` writes a warning at each start.")
        .refs(Ref::Str("open"), Ref::Str("authzen")),
    KeyDesc::new(
        "access.identity.header",
        text(1, 64, TextRule::NoControl),
        "The name of the header that holds the token: 1 to 64 token characters (RFC 9110). It is not `Authorization`, `Proxy-Authorization`, `Cookie`, `Host`, `Content-Type` or `Content-Length`, in any case.",
    )
    .refs(Ref::Absent, Ref::Str("X-Identity-Token")),
    KeyDesc::new("access.identity.issuer", text(1, 1024, TextRule::NoControl), "The value that the claim `iss` must have.")
        .refs(Ref::Absent, Ref::Str("https://edge.example.invalid")),
    KeyDesc::new("access.identity.audience", text(1, 256, TextRule::NoControl), "The service id (3.6): 1 to 256 bytes of UTF-8.")
        .refs(Ref::Absent, Ref::Str("pupitre")),
    KeyDesc::new("access.identity.public_keys", text(1, 65_536, TextRule::Any), "The public keys of the edge: PEM text or a JWK Set with 1 to 16 keys.")
        .forms(Forms::Vef)
        .refs(Ref::Absent, Ref::File("/run/secrets/edge/jwks.json")),
    KeyDesc::new("access.identity.max_age", seconds(1, 86_400), "The longest life of a token. A whole number of seconds.")
        .refs(Ref::Absent, Ref::Str("5m")),
    KeyDesc::new("access.identity.clock_skew", seconds(0, 300), "The skew that applies to `iat`, `exp` and `nbf`. A whole number of seconds.")
        .refs(Ref::Absent, Ref::Str("1m")),
    KeyDesc::new(
        "access.pdp.url",
        text(1, 2048, TextRule::NoControl),
        "A URL with the scheme `http` or `https`, a host, an optional port and an optional path, with no user info, query or fragment.",
    )
    .forms(Forms::Vef)
    .refs(Ref::Absent, Ref::Str("https://pdp.example.invalid:4182")),
    KeyDesc::new("access.pdp.token", text(1, 4096, TextRule::NoControlNoSpace), "The credential of Pupitre at the PDP, sent in `Authorization: Bearer`.")
        .secret()
        .refs(Ref::Absent, Ref::File("/run/secrets/pdp/token")),
    KeyDesc::new("access.pdp.ca_file", Kind::Path, "A PEM file of CA certificates.")
        .when(PLAIN_HTTP_OFF)
        .necessary("`access.pdp.plain_http = false`")
        .forbidden("`access.pdp.plain_http = true`, or `access.mode = \"open\"`")
        .refs(Ref::Absent, Ref::Str("/run/secrets/pdp/ca.pem")),
    KeyDesc::new("access.pdp.plain_http", Kind::Bool, "The value `true` accepts an `http` URL, with a warning at start. It must agree with the scheme of `access.pdp.url`.")
        .refs(Ref::Absent, Ref::Bool(false)),
    KeyDesc::new("access.pdp.timeout", Kind::Duration { min_ms: 10, max_ms: 60_000 }, "The longest wait for the PDP.")
        .refs(Ref::Absent, Ref::Str("500ms")),
    KeyDesc::new("access.pdp.cache_ttl", seconds(0, 3600), "The longest life of a kept answer (3.17).")
        .refs(Ref::Absent, Ref::Str("30s")),
    KeyDesc::new("access.pdp.max_connections", Kind::Integer { min: 1, max: 32 }, "The most requests to the PDP at one time.")
        .refs(Ref::Absent, Ref::Int(4)),
    KeyDesc::new("access.pdp.cache_entries", Kind::Integer { min: 1, max: 1_000_000 }, "The most kept answers (3.17). The default is 16384.")
        .when_optional(AUTHZEN, "never (the key is optional)")
        .doc_ref(Ref::Int(16_384)),
    KeyDesc::new("access.options_max_questions", Kind::Integer { min: 1, max: 10_000 }, "The most questions of one list request or options request (3.15, 3.16). The default is 200.")
        .when_optional(AUTHZEN, "never (the key is optional)")
        .doc_ref(Ref::Int(200)),
    KeyDesc::new("access.options_page_items", Kind::Integer { min: 1, max: 50 }, "The most items of one page of the options route (3.16). The default is 50.")
        .when_optional(AUTHZEN, "never (the key is optional)")
        .doc_ref(Ref::Int(50)),
    // 6.12: runner
    KeyDesc::new("runner.enabled", Kind::Bool, "The value `true` serves the runs (section 4). The value `false` serves no run and publishes no run action (3.3).")
        .both(Ref::Bool(true)),
    KeyDesc::new("runner.execute", Kind::Bool, "The value `true` makes this instance the runner node (4.1). With the backend `files`, it is `true`.")
        .when(RUNNER_ON)
        .both(Ref::Bool(true)),
    KeyDesc::new("runner.allow_apply_in_open_mode", Kind::Bool, "The value `true` accepts the applies and the commands in the mode `open` (3.22).")
        .when(OPEN_RUNNER_ON)
        .refs(Ref::Bool(false), Ref::Absent),
    KeyDesc::new("runner.check_max_age", Kind::Duration { min_ms: 60_000, max_ms: 86_400_000 }, "The oldest check that an apply can use (4.24).")
        .when(RUNNER_ON)
        .both(Ref::Str("30m")),
    KeyDesc::new("runner.max_hosts", Kind::Integer { min: 1, max: 10_000 }, "The most hosts of one run (3.10). It is also the `max` of the ceiling `max_hosts`.")
        .when(RUNNER_ON)
        .both(Ref::Int(100)),
    KeyDesc::new("runner.max_queued_runs", Kind::Integer { min: 1, max: 10_000 }, "The most runs with the status `queued` (4.5). The default is 100.")
        .when_optional(RUNNER_ON, "never (the key is optional)")
        .doc_ref(Ref::Int(100)),
    KeyDesc::new("runner.work_dir", Kind::Path, "The folder of the runs on the runner node (4.10).")
        .when(EXECUTOR)
        .both(Ref::Str("/var/lib/pupitre/runner")),
    KeyDesc::new("runner.secrets_dir", Kind::Path, "The folder of the private folders of the runs, on `tmpfs` or `ramfs`, mode 0700 (4.13).")
        .when(EXECUTOR)
        .both(Ref::Str("/dev/shm/pupitre")),
    KeyDesc::new("runner.ansible_playbook", Kind::Path, "The one program that the runner starts (4.17). It MUST be ansible-core 2.19 or later (7.8).")
        .when(EXECUTOR)
        .both(Ref::Str("/usr/bin/ansible-playbook")),
    KeyDesc::new("runner.path", text(1, 4096, TextRule::NoControlNoSpace), "The value of `PATH` for the child (4.18).")
        .when(EXECUTOR)
        .both(Ref::Str("/usr/local/bin:/usr/bin:/bin")),
    KeyDesc::new("runner.ssh_user", text(1, 32, TextRule::NoControlNoSpace), "The SSH account on the hosts (4.15).")
        .when(EXECUTOR)
        .both(Ref::Str("deploy")),
    KeyDesc::new("runner.ssh_key_file", Kind::Path, "The SSH private key, a file of the user of the process with the mode 0600 or 0400 (7.8).")
        .when(EXECUTOR)
        .both(Ref::Str("/run/secrets/ssh/id_ed25519")),
    KeyDesc::new("runner.known_hosts_file", Kind::Path, "The pinned host keys: one line for each `<environment>.<host>`, and no other line (4.12, 7.11).")
        .when(EXECUTOR)
        .both(Ref::Str("/run/secrets/ssh/known_hosts")),
    KeyDesc::new("runner.releases_dir", Kind::Path, "The local folder of the release files, given to the playbooks as `pupitre_releases_dir` (4.14). The key is optional and has no default.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Str("/srv/releases")),
    KeyDesc::new("runner.run_timeout", Kind::Duration { min_ms: 60_000, max_ms: 172_800_000 }, "The longest run (4.28).")
        .when(EXECUTOR)
        .both(Ref::Str("2h")),
    KeyDesc::new("runner.forks", Kind::Integer { min: 1, max: 100 }, "The value of `forks` of `ansible.cfg` (4.15). The default is 5.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Int(5)),
    KeyDesc::new("runner.max_parallel_runs", Kind::Integer { min: 1, max: 64 }, "The most runs at one time on the runner node (4.6). The default is 1.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Int(1)),
    KeyDesc::new("runner.cancel_grace", seconds(1, 600), "The time between `SIGTERM` and `SIGKILL` (4.19). The default is `30s`.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Str("30s")),
    KeyDesc::new("runner.lease_ttl", seconds(5, 600), "The time to live of the leases (4.7, 4.8). The default is `30s`.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Str("30s")),
    KeyDesc::new("runner.max_log_bytes", Kind::Size { min: 65_536, max: 1_073_741_824 }, "The most stored output of one run (4.22). The default is 16777216.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Int(16_777_216)),
    KeyDesc::new("runner.max_events", Kind::Integer { min: 100, max: 10_000_000 }, "The most stored events of one run (4.22). The default is 100000.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Int(100_000)),
    KeyDesc::new("runner.max_event_bytes", Kind::Size { min: 1024, max: 1_048_576 }, "The longest event line (4.20). The default is 65536.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Int(65_536)),
    KeyDesc::new("runner.runs_keep", Kind::Integer { min: 10, max: 1_000_000 }, "The most final runs that the store keeps (4.31). The default is 10000.")
        .when_optional(EXECUTOR, "never (the key is optional)")
        .doc_ref(Ref::Int(10_000)),
    // 6.12: limits
    KeyDesc::new("limits.max_environments", Kind::Integer { min: 1, max: 1024 }, "The most environments (2.42). The default is 64.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(64)),
    KeyDesc::new("limits.max_units_per_environment", Kind::Integer { min: 1, max: 10_000 }, "The most units of one environment (2.42). The default is 1000.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(1000)),
    KeyDesc::new("limits.max_hosts_per_environment", Kind::Integer { min: 1, max: 10_000 }, "The most hosts of one environment, and of one unit (2.42). The default is 1000.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(1000)),
    KeyDesc::new("limits.max_document_bytes", Kind::Size { min: 4096, max: 1_048_576 }, "The largest canonical document (2.42). The default is 262144.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(262_144)),
    KeyDesc::new("limits.max_store_bytes", Kind::Size { min: 1_048_576, max: 17_179_869_184 }, "The most canonical bytes of all current documents (2.42). The default is 268435456.")
        .manual(OPTIONAL, NEVER)
        .doc_ref(Ref::Int(268_435_456)),
    // 6.12: schema, playbook, command
    KeyDesc::new("schema.path", Kind::Path, "The schema file (2.11). Pupitre reads it at start.")
        .both(Ref::Str("/srv/ansible/schemas/relay.json")),
    KeyDesc::new("playbook.kind", Kind::Enum(&["deploy", "render", "status", "command"]), "The kind of the playbook (4.2, 4.3).")
        .both(Ref::Str("deploy")),
    KeyDesc::new("playbook.path", Kind::Path, "The playbook file. The runner node reads it at start and at each run (4.16).")
        .both(Ref::Str("/srv/ansible/playbooks/deploy-relay.yml")),
    KeyDesc::new("command.label", text(1, 256, TextRule::NoControl), "The text of the page: 1 to 64 characters (4.32).")
        .doc_ref(Ref::Str("Restart the service")),
    KeyDesc::new("command.playbook", text(1, 63, TextRule::NoControl), "The id of a block `playbook` with the kind `command` (4.32).")
        .doc_ref(Ref::Str("restart-service")),
    KeyDesc::new("command.target", Kind::Enum(&["environment", "unit", "host"]), "The type of the node of a launch (4.32).")
        .doc_ref(Ref::Str("unit")),
    KeyDesc::new("command.check", Kind::Bool, "The value `true` makes an apply need a check of the same snapshot (4.24).")
        .doc_ref(Ref::Bool(false)),
    KeyDesc::new("command.param.label", text(1, 256, TextRule::NoControl), "The label of the form: 1 to 64 characters (4.33).")
        .doc_ref(Ref::Str("Grace period")),
    KeyDesc::new("command.param.type", Kind::Enum(&["text", "integer", "boolean", "duration", "choice"]), "The type of the parameter (4.33).")
        .doc_ref(Ref::Str("duration")),
    KeyDesc::new("command.param.required", Kind::Bool, "The value `true` makes the parameter necessary in a request (4.33).")
        .doc_ref(Ref::Bool(true)),
    KeyDesc::new("command.param.min", Kind::Integer { min: 0, max: MAX_SAFE }, "The lowest value of an `integer`, or the fewest characters of a `text`.")
        .manual("`type = \"text\"` or `type = \"integer\"`", "another type")
        .doc_ref(Ref::Int(1)),
    KeyDesc::new("command.param.max", Kind::Integer { min: 0, max: MAX_SAFE }, "The highest value of an `integer`, or the most characters of a `text`.")
        .manual("`type = \"text\"` or `type = \"integer\"`", "another type")
        .doc_ref(Ref::Int(64)),
    KeyDesc::new("command.param.min_duration", Kind::Duration { min_ms: 0, max_ms: 31_536_000_000 }, "The lowest value of a `duration`.")
        .manual("`type = \"duration\"`", "another type")
        .doc_ref(Ref::Str("1s")),
    KeyDesc::new("command.param.max_duration", Kind::Duration { min_ms: 0, max_ms: 31_536_000_000 }, "The highest value of a `duration`.")
        .manual("`type = \"duration\"`", "another type")
        .doc_ref(Ref::Str("5m")),
    KeyDesc::new(
        "command.param.choices",
        Kind::TextList { min_items: 1, max_items: 32, item_min: 1, item_max: 64, rule: TextRule::NoControl },
        "The closed list of a `choice`, each text one time.",
    )
    .manual("`type = \"choice\"`", "another type")
    .doc_ref(Ref::StrList(&["soft", "hard"])),
];

/// The key table of Pupitre.
pub static SCHEMA: Schema = Schema { blocks: &BLOCKS, keys: &KEYS };

/// The keys that have a default value (6.7), with the default in the form of `print-config`.
/// The effective configuration shows the default with the source `default`.
pub const DEFAULTS: [(&str, &str); 33] = [
    ("server.max_connections", "1024"),
    ("server.idle_timeout", "\"75s\""),
    ("server.shutdown_grace", "\"30s\""),
    ("server.header_read_timeout", "\"10s\""),
    ("server.body_read_timeout", "\"30s\""),
    ("server.max_request_bytes", "1048576"),
    ("server.max_inflight_bodies", "64"),
    ("server.max_page_items", "200"),
    ("telemetry.max_queued_spans", "2048"),
    ("storage.files.rescan_interval", "\"5s\""),
    ("storage.files.history_keep", "100"),
    ("storage.postgres.max_connections", "8"),
    ("storage.postgres.connect_timeout", "\"5s\""),
    ("storage.postgres.statement_timeout", "\"10s\""),
    ("storage.postgres.poll_interval", "\"5s\""),
    ("storage.postgres.history_keep", "100"),
    ("access.pdp.cache_entries", "16384"),
    ("access.options_max_questions", "200"),
    ("access.options_page_items", "50"),
    ("runner.max_queued_runs", "100"),
    ("runner.forks", "5"),
    ("runner.max_parallel_runs", "1"),
    ("runner.cancel_grace", "\"30s\""),
    ("runner.lease_ttl", "\"30s\""),
    ("runner.max_log_bytes", "16777216"),
    ("runner.max_events", "100000"),
    ("runner.max_event_bytes", "65536"),
    ("runner.runs_keep", "10000"),
    ("limits.max_environments", "64"),
    ("limits.max_units_per_environment", "1000"),
    ("limits.max_hosts_per_environment", "1000"),
    ("limits.max_document_bytes", "262144"),
    ("limits.max_store_bytes", "268435456"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_consistent() {
        assert_eq!(SCHEMA.validate(), Ok(()));
    }

    #[test]
    fn each_default_has_a_key_with_the_same_documented_value() {
        for (path, value) in DEFAULTS {
            let key = SCHEMA.key(path).unwrap_or_else(|| panic!("no key {path}"));
            let doc = match key.doc_ref {
                Ref::Int(n) => n.to_string(),
                Ref::Str(s) => format!("\"{s}\""),
                Ref::Bool(b) => b.to_string(),
                _ => String::new(),
            };
            assert_eq!(doc, value, "{path}");
            assert!(key.description.contains("The default is"), "{path}");
        }
    }

    #[test]
    fn the_secret_keys_of_7_6() {
        let secrets: Vec<&str> = SCHEMA.keys.iter().filter(|k| k.secret).map(|k| k.path).collect();
        assert_eq!(secrets, vec!["storage.secret_key", "storage.postgres.password", "access.pdp.token"]);
    }
}
