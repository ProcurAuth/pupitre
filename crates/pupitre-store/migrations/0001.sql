-- SPDX-License-Identifier: MIT OR Apache-2.0
-- The store schema 1 of the backend postgres (2.36). The command `pupitre migrate` runs this
-- text in one transaction, then writes the row of pupitre_meta with the key check (2.26, 2.30).
-- The account of `serve` needs SELECT, INSERT, UPDATE and DELETE on each table, and USAGE on
-- the sequence. It needs no right to change a table.

-- One row: the version of the store schema and the key check of storage.secret_key.
CREATE TABLE pupitre_meta (
    singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
    store_schema integer NOT NULL CHECK (store_schema > 0),
    key_check text NOT NULL CHECK (key_check ~ '^[0-9a-f]{64}$')
);

-- One value for each write of a document (2.37, 2.38).
-- A writer takes the advisory lock of the change barrier (2.38) in shared mode before it takes a value.
CREATE SEQUENCE pupitre_change_seq AS bigint;

-- The current documents. A delete sets bytes and etag to NULL and keeps the row (2.37).
CREATE TABLE documents (
    key text PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('environment', 'common', 'host', 'unit', 'unit_values', 'host_values')),
    bytes bytea,
    etag bytea CHECK (octet_length(etag) = 32),
    version bigint NOT NULL CHECK (version > 0),
    change_seq bigint NOT NULL,
    updated_at timestamptz NOT NULL,
    CHECK ((bytes IS NULL) = (etag IS NULL))
);
CREATE INDEX documents_change_seq ON documents (change_seq);

-- The versions of each key (2.18). A version 'delete' has no bytes and no etag.
CREATE TABLE document_versions (
    key text NOT NULL,
    version bigint NOT NULL CHECK (version > 0),
    kind text NOT NULL CHECK (kind IN ('create', 'save', 'delete', 'import', 'external')),
    bytes bytea,
    etag bytea CHECK (octet_length(etag) = 32),
    saved_by text NOT NULL,
    saved_at timestamptz NOT NULL,
    message text NOT NULL,
    PRIMARY KEY (key, version),
    CHECK ((kind = 'delete') = (bytes IS NULL)),
    CHECK ((bytes IS NULL) = (etag IS NULL))
);

-- The secrets, encrypted with ChaCha20-Poly1305 (2.39). The key is not in the database.
-- The fingerprint is part of the associated data of the cipher: a ciphertext opens only with the fingerprint that it was sealed with.
CREATE TABLE secrets (
    scope text NOT NULL,
    name text NOT NULL,
    nonce bytea NOT NULL CHECK (octet_length(nonce) = 12),
    ciphertext bytea NOT NULL,
    fingerprint text NOT NULL CHECK (fingerprint ~ '^[0-9a-f]{16}$'),
    updated_by text NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (scope, name)
);

-- The events of the secrets (2.24). No value.
CREATE TABLE secret_events (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    scope text NOT NULL,
    name text NOT NULL,
    action text NOT NULL CHECK (action IN ('set', 'delete')),
    saved_by text NOT NULL,
    saved_at timestamptz NOT NULL
);
CREATE INDEX secret_events_secret ON secret_events (scope, name, id);

-- The run records (4.38), as canonical bytes. The columns repeat the members of the filters.
CREATE TABLE runs (
    id text PRIMARY KEY,
    environment text NOT NULL,
    status text NOT NULL CHECK (status IN ('queued', 'running', 'passed', 'failed', 'canceled', 'lost')),
    record bytea NOT NULL
);
CREATE INDEX runs_status ON runs (status, id);
CREATE INDEX runs_environment ON runs (environment, id);

-- The output of each run, in chunks (2.28). byte_offset is the offset of the first byte.
CREATE TABLE run_output (
    run_id text NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq bigint NOT NULL CHECK (seq > 0),
    byte_offset bigint NOT NULL CHECK (byte_offset >= 0),
    bytes bytea NOT NULL,
    PRIMARY KEY (run_id, seq)
);

-- The event lines of each run (2.28).
CREATE TABLE run_events (
    run_id text NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq bigint NOT NULL CHECK (seq > 0),
    line bytea NOT NULL,
    PRIMARY KEY (run_id, seq)
);

-- The leases (2.29): 'runner' and 'environment:<env>'. The clock of the database decides.
CREATE TABLE leases (
    name text PRIMARY KEY,
    holder text NOT NULL,
    expires_at timestamptz NOT NULL
);
