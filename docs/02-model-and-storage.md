# Pupitre specification, part 2: model and storage

This part uses ASD-STE100 Simplified Technical English. Part 1 gives the rules of the document.

## 2. Model and storage

### 2.1 Ids

Each id is a text of ASCII characters. An id that breaks its rule is an error with the id, the rule and the value. Pupitre never changes an id to make it fit.

| Id | Rule | Example |
|---|---|---|
| Environment, unit | `^[a-z0-9][a-z0-9-]{0,62}$` | `production`, `relay` |
| Host | `^[a-z0-9][a-z0-9-]{0,62}$`. It is not `all`, `ungrouped` or `localhost`. | `host-a` |
| Schema, playbook, command (labels of the configuration blocks) | `^[a-z0-9][a-z0-9-]{0,62}$` | `relay`, `deploy-relay` |
| Parameter name of a command | `^[a-z][a-z0-9_]{0,31}$` | `grace` |
| Host label | `^[a-z][a-z0-9_]{0,62}$`. It is not `all`, `ungrouped` or `localhost`. It does not start with `ansible_` or `pupitre_`. | `region_eu`, `canary` |
| Key path | 1 to 8 segments joined by `.`. A segment matches `^[a-z][a-z0-9_]{0,62}$`. The path has 255 bytes at most. | `limits.max_clients` |
| Ansible variable and group of a schema (2.11) | `^[a-z][a-z0-9_]{0,62}$`. The variable does not start with `ansible_` or `pupitre_`, and it is not a name that Ansible reserves, for example `hostvars`, `groups`, `inventory_hostname`, `omit`, `vars`, `environment`, `group_names` or `play_hosts`. A group does not start with `ansible_` or `pupitre_`, and it is not `all` or `ungrouped`. | `relay_config`, `relay_hosts` |
| Run id | A UUID version 7 in the form `8-4-4-4-12`, lower-case hexadecimal | `0193a1f2-7c4e-7d10-8a3b-5c2e9f0d1a42` |

These ids hold no character that P 5.2 encodes. A node path of 3.8 thus holds the ids as they are. Ansible reserves the names `all`, `ungrouped` and `localhost` in an inventory, and the group `pupitre_run` is the group of a run (4.11). So no host and no label has these names.

A label (2.7, 2.9), a note (2.10), a subject (2.18) and the message of a run request (4.4) are free texts. A free text never holds a control character, except U+000A where its rule permits it. It also never holds a format character: a character of the Unicode category Cf. Examples are the bidirectional controls U+202A to U+202E, U+2066 to U+2069, U+200E, U+200F and U+061C, and U+200B. Such a character can change the order or the look of a text on the page, so that a person reads another text.

### 2.2 The documents

The store keeps six kinds of document. Each document is one JSON object in the canonical form of 2.4.

| Kind | Content | Key (2.3) |
|---|---|---|
| `environment` | The label of an environment | `environments/<env>` |
| `common` | The values of the layer `environment` | `environments/<env>/common` |
| `host` | The address, the port and the labels of a host | `environments/<env>/hosts/<host>` |
| `unit` | The label, the schema, the playbooks and the hosts of a unit | `environments/<env>/units/<unit>` |
| `unit_values` | The values of the layer `unit` | `environments/<env>/units/<unit>/values` |
| `host_values` | The values of the layer `host` for one unit on one host | `environments/<env>/units/<unit>/hosts/<host>/values` |

The store also keeps secrets (2.24), runs (2.27), their output and events (2.28), and leases (2.29). These are not documents.

### 2.3 Document keys

A document key is the text of the last column of the table of 2.2, with the ids of 2.1. The key gives the kind. A key with another form is not a key, and the store ignores such an entry with the event `store.entry_ignored` (6.27).

The key is the identity of a document. A document holds no id: its key gives the ids. The parent of a document is the document of the key before the last segment pair. Example: the parent of `environments/production/units/relay/values` is `environments/production/units/relay`.

### 2.4 The canonical form

Each document, each run record and each manifest of a snapshot has exactly one byte form. The writer of `pupitre-core` makes it. These are the rules:

1. The text is UTF-8 with no byte order mark. It ends with one U+000A.
2. A value is an object, an array, a string, an integer, `true` or `false`. A document has no `null` and no number with a fraction or an exponent.
3. The members of an object are in the byte order of their names. A name occurs one time.
4. An object with members writes `{`, a line end, each member on its own line, then `}`. An array with items writes `[`, a line end, each item on its own line, then `]`. An empty object is `{}`. An empty array is `[]`.
5. Each line of a member or an item starts with two spaces for each level of depth. A member writes the name, `: `, and the value. A comma ends each member and each item except the last.
6. An integer is from 0 to 9007199254740991 (2^53-1). It has decimal digits, no sign and no zero at the start, except the integer `0`.
7. A string writes `"`, the characters, `"`. The escapes are `\"`, `\\`, `\b`, `\f`, `\n`, `\r` and `\t`. Each other character below U+0020 is `\u00xx` with lower-case digits. Each other character stays as it is, also `/` and U+007F.

Example, a document `common`:

```json
{
  "format": 1,
  "notes": {
    "log.level": "Debug for the incident of 2026-11-02."
  },
  "values": {
    "limits.max_clients": 200,
    "log.level": "debug"
  }
}
```

The canonical form is also valid YAML. So a file of the backend `files` is easy to read in a review of Git.

### 2.5 The strict reader

The strict reader of `pupitre-core` reads each document, each request body of the API and each schema file. It refuses:

- a text that is not UTF-8, a byte order mark, and a text after the value;
- `null`, a number with a sign, a fraction, an exponent or a zero at the start, and an integer above 2^53-1;
- a name that occurs two times in one object;
- an escape that is not valid, and a `\u` escape of a lone surrogate;
- a depth above 16 levels of objects and arrays;
- a text above the byte bound that the caller gives.

A refusal gives the byte offset and the reason. The reader never repairs a text.

A stored document MUST be in the canonical form. The catalog reads the bytes, parses them, writes them again in the canonical form, and compares. Different bytes make the document invalid, with the error `not_canonical` and the offset of the first different byte. So a file that a person edits by hand must keep the canonical form.

### 2.6 The etag

The etag of a document is the SHA-256 of its canonical bytes. Its text form has 64 lower-case hexadecimal characters. In HTTP, it is a strong entity tag: `ETag: "<64 hex>"`.

Two documents with the same bytes have the same etag. An etag compares two versions of one key only.

### 2.7 The document `environment`

```json
{
  "format": 1,
  "label": "Production"
}
```

| Member | Rule |
|---|---|
| `format` | The integer 1. |
| `label` | A text of 1 to 64 characters with no control character and no format character (2.1). |

No other member is valid.

### 2.8 The document `host`

```json
{
  "address": "203.0.113.10",
  "format": 1,
  "labels": [
    "canary",
    "region_eu"
  ],
  "port": 22
}
```

| Member | Rule |
|---|---|
| `format` | The integer 1. |
| `address` | A DNS name of 1 to 253 bytes whose last label is not all digits, or an IPv4 or IPv6 address in canonical form. It becomes `ansible_host` (4.12). |
| `port` | Optional. An integer from 1 to 65535. It becomes `ansible_port`. Without it, the SSH client uses its default port. |
| `labels` | An array of 0 to 16 host labels of 2.1, in byte order, each one time. Each label is an inventory group (4.11). |

A label MUST NOT be the `group` of a schema of the configuration (2.11). The server refuses such a write with `422 invalid_document`.

### 2.9 The document `unit`

```json
{
  "format": 1,
  "hosts": [
    "host-a",
    "host-b"
  ],
  "label": "Relay",
  "playbooks": {
    "deploy": "deploy-relay",
    "render": "render-relay",
    "status": "status-relay"
  },
  "schema": "relay"
}
```

| Member | Rule |
|---|---|
| `format` | The integer 1. |
| `label` | A text of 1 to 64 characters with no control character and no format character (2.1). |
| `schema` | The id of a block `schema` of the configuration (6.12). |
| `playbooks` | An object. `deploy` is necessary. `render` and `status` are optional. Each value is the id of a block `playbook` of the configuration with the same `kind` as the member name. |
| `hosts` | An array of 0 to `limits.max_hosts_per_environment` host ids, in byte order, each one time. Each host MUST have a document `host` in the same environment. |

A host MUST NOT be in two units of one environment that name the same schema. The server refuses such a write with `422 invalid_document`. The reason: the two units would write the same Ansible variable on the host (4.12).

A unit names playbooks of the catalog of capabilities. It never names a path. When the configuration has no block `playbook` (`runner.enabled = false`), the member `playbooks` still names ids, and the server does not check them. The catalog then shows no run action for the unit.

### 2.10 The documents of values

The kinds `common`, `unit_values` and `host_values` have one form:

| Member | Rule |
|---|---|
| `format` | The integer 1. |
| `values` | An object. Each name is a key path (2.1). Each value has the JSON type of its value kind (2.12). 0 to 1,024 members. |
| `notes` | Optional. An object. Each name is a key path of `values`. Each value is a text of 1 to 500 characters with no control character except U+000A and no format character (2.1). |

The checks of the values:

- `unit_values` and `host_values`: each key path MUST be a key of the schema of the unit, and not a secret key. Each value MUST pass the check of its kind (2.12).
- `host_values`: the host MUST be in `hosts` of the unit.
- `common`: each key path MUST be a key of at least one schema of the configuration, and not a secret key of any schema. The value MUST pass the check of each schema that declares the key path (2.15).

A value that fails gives `422 invalid_document` with one error for each key: the key path, the expected type and range, and the value. Pupitre never changes a value to make it fit.

### 2.11 The schema of a unit

A block `schema "<id>"` of the configuration names a schema file (6.12). Pupitre reads each file at start, with the strict reader of 2.5, and stops at start when a file is not valid. A file has 1 MiB at most. The schemas are not in the store. A person who edits values cannot add a key.

```json
{
  "format": 1,
  "group": "relay_hosts",
  "keys": [
    {
      "default": 100,
      "description": "The most clients at one time.",
      "kind": "integer",
      "max": 100000,
      "min": 1,
      "path": "limits.max_clients"
    },
    {
      "default": "info",
      "description": "The lowest level of the log.",
      "kind": "enum",
      "path": "log.level",
      "values": [
        "error",
        "warn",
        "info",
        "debug"
      ]
    },
    {
      "description": "The password of the database account.",
      "kind": "text",
      "max": 1024,
      "min": 8,
      "path": "storage.password",
      "rule": "any",
      "secret": true
    }
  ],
  "variable": "relay_config"
}
```

| Member | Rule |
|---|---|
| `format` | The integer 1. |
| `variable` | The Ansible variable that holds the effective values of a unit on a host (4.12). It follows 2.1. |
| `group` | The inventory group of the hosts of the units of this schema (4.11). It follows 2.1. |
| `keys` | An array of 1 to 1,024 keys, in the byte order of `path`, each path one time. |

The members of a key:

| Member | Rule |
|---|---|
| `path` | A key path of 2.1. No path is the start of another path at a segment boundary: `a.b` and `a.b.c` cannot be in one schema. |
| `kind` | One kind of 2.12. |
| Range members | The members of the kind, from the table of 2.12. Each one is necessary. No other member is valid. |
| `description` | A text of 1 to 300 characters. |
| `secret` | Optional. `true` makes the key a secret key (2.24). Absent means `false`. A secret key has the kind `text`. A secret key named `units` can have a secret in the scope of a unit only (2.24). |
| `default` | Optional. A value that passes the check of the kind. It is the layer `default` (2.13). A secret key has no `default`. |

The text of a schema file is the same JSON that a service of the family prints from its table of keys. A labelled block of the service becomes one path for each instance, for example `clickhouse.prod.url`.

These checks run at start, across all schemas (6.13):

- Two schemas with the same key path MUST give it the same `kind` and the same `secret`. The range can differ.
- Two schemas MUST NOT have the same `variable`. Two schemas MUST NOT have the same `group`.

### 2.12 The value kinds

The kinds are the kinds of the configuration engine (6.4). A value has the JSON type of its kind.

| Kind | Range members | JSON type | Check |
|---|---|---|---|
| `text` | `min`, `max` (bytes), `rule` | string | The length in bytes is in the range. The rule holds. |
| `text_list` | `min_items`, `max_items`, `item_min`, `item_max`, `rule` | array of strings | The count and each item are in their ranges. The rule holds for each item. |
| `enum` | `values`: 1 to 64 texts of 1 to 64 bytes, each one time | string | The value is one of `values`. |
| `integer` | `min`, `max` | integer | The value is in the range. |
| `size` | `min`, `max` (bytes) | integer | The value is in the range. |
| `duration` | `min`, `max` (texts of durations) | string | The text is a duration of 6.4 in canonical form, and it is in the range. |
| `bool` | none | `true` or `false` | None. |
| `path` | none | string | An absolute path of 1 to 4,096 bytes with no NUL byte. |
| `socket_addr` | none | string | An IP address and a port in canonical form. |

The rules of a text are `any`, `no_control`, `printable_ascii`, `no_control_no_space`, `dns_name` and `host_or_ip`. The engine gives their rules (6.4). A string of a value never holds U+0000.

A string of a value that is not a secret never holds a template marker of Ansible: `{{`, `{%` or `{#`. The rule applies to each kind with a string, to each item of a list, and to the `default` of a schema. Ansible evaluates such a marker when a playbook reads the value (7.3). The error gives the key path, the expected kind with the words `no template marker`, and the value. A secret value can hold a marker: it reaches Ansible as the content of a file, never as a variable (4.13).

The engine writes a duration in its canonical form. It has one whole number and the largest unit of `ms`, `s`, `m` and `h` that gives a whole number. Example: `90s` and `2m` are canonical. `120s` is not canonical, and the check gives the expected text `2m`.

### 2.13 The layers and the resolution

A unit U has the schema S and is in the environment E. On a host H of U, each key K of S gets its value from four layers. The last layer that gives a value wins.

| Order | Layer | Source |
|---|---|---|
| 0 | `default` | The member `default` of K in S |
| 1 | `environment` | `values[K]` of the document `common` of E |
| 2 | `unit` | `values[K]` of the document `unit_values` of U |
| 3 | `host` | `values[K]` of the document `host_values` of U on H |

- An absent document gives no value. An absent member gives no value.
- A key with no value in any layer has no effective value. The variable of 4.12 does not hold it. The render phase of 4.25 then tells if the service needs it.
- The resolution of a unit with no host uses the layers 0 to 2. The page shows it as the view of the unit.
- A key path of `common` that S does not declare has no effect on U.
- The resolution checks each layer value against K in S again. A value that fails gives an error for K, with the layer, and K has no effective value. This happens after a change of a schema file. A run then stops before the snapshot (4.9). Pupitre never changes the value.

A secret key has two layers only: `environment` and `unit` (2.24). The unit layer wins. A secret has no default and no host layer.

### 2.14 The provenance

The resolution gives, for each key of S, in the order of S:

| Member | Content |
|---|---|
| `path` | The key path |
| `kind` | The kind |
| `secret` | `true` for a secret key |
| `layer` | The layer that gives the effective value, or absent when no layer gives one |
| `value` | The effective value. Absent for a secret key or when no layer gives one. |
| `layers` | An object with one member for each layer that gives a value: `default`, `environment`, `unit`, `host`. For a secret key, each member holds `{"fingerprint": "<16 hex>"}` (2.26). |
| `error` | Present when a stored value fails its check (2.13): `{"layer": "<layer>", "message": "<text>"}` |

The route of 5.12 gives this list.

### 2.15 The common values and several schemas

A key path of `common` can be in several schemas. The value MUST pass the check of each of them at write. Example: `log.level` is an `enum` in the schemas `relay` and `feed`. The value `trace` fails when one of the two lists does not hold it.

A later change of a schema file can make a stored value fail. Rule 2.13 then gives an error for the units of that schema only.

### 2.16 Rules across documents

The server checks these rules at each write, with the documents of the catalog (2.41). A refusal gives `409 in_use`, `409 not_empty` or `422 invalid_document`.

| Write | Rule | Code |
|---|---|---|
| Create a document other than `environment` | The document `environment` of its environment exists. | `422 invalid_document` |
| Create or change `unit` | Each host of `hosts` has a document `host`. | `422 invalid_document` |
| Change `unit` | A host that leaves `hosts` has no document `host_values` for this unit. | `409 in_use` |
| Create or change `host_values` | The unit exists and its `hosts` holds the host. | `422 invalid_document` |
| Delete `host` | No unit of the environment holds the host. | `409 in_use` |
| Delete `unit` | The unit has no document `unit_values`, no document `host_values` and no secret. | `409 not_empty` |
| Delete `environment` | The environment has no other document and no secret. | `409 not_empty` |
| Each write | The counts stay within the limits of 2.42. | `422 limit_exceeded` |

Two instances of the backend `postgres` read the catalog at different times. Two writes at the same time can thus break a rule. The catalog shows each broken reference as an error of the document, and a run of that unit stops before the snapshot (4.9). Section 7.11 lists this limit.

### 2.17 The `Store` trait

The crate `pupitre-store` defines the trait `Store`. The backends `files` and `postgres` implement it, and the enum `AnyStore` dispatches the calls. Each method is async, and its future is `Send`. The store keeps bytes and metadata. It does not parse a document: the catalog does it (2.41).

| Group | Methods |
|---|---|
| Documents | `load`, `poll`, `get`, `put`, `put_import`, `delete`, `history`, `version`, `read_environment` |
| Secrets | `list_secrets`, `put_secret`, `delete_secret`, `read_secrets` |
| Runs | `create_run`, `get_run`, `list_runs`, `list_runs_page`, `next_queued`, `update_run`, `prune_runs` |
| Output and events | `append_output`, `read_output`, `append_events`, `read_events` |
| Leases | `acquire_lease`, `renew_lease`, `release_lease` |
| Transfer | `export_begin`, `import_begin` |

The code of `crates/pupitre-store/src/store.rs` gives the exact signatures. This part gives their rules.

### 2.18 Write metadata and versions

Each write of a document carries:

| Member | Rule |
|---|---|
| `saved_by` | The subject: 1 to 256 characters with no control character and no format character (2.1). `-` in the mode `open`. `import` for the command `import`. `external` for a change outside Pupitre (2.33). |
| `saved_at` | The time of the instance, in UTC, to the millisecond. |
| `message` | A text of 0 to 500 characters with no control character except U+000A. |

Each key has its own list of versions, from 1, with no gap. A write makes the next version. A version keeps the kind of the write (`create`, `save`, `delete`, `import`, `external`), the metadata, the etag and the bytes. A version of the kind `delete` has no bytes and no etag.

The call `put_import` makes the version kind `import`. It has the rules of `put` with the precondition `Create`. Only the command `import` calls it (6.24a). `history` of a key that never had a version gives an empty list, on both backends.

The store keeps the newest `storage.<backend>.history_keep` versions of each key (6.10). It removes the oldest versions after a write. The numbers of the versions never change.

### 2.19 The order of a write

A write of a document follows this order:

1. Check the precondition (2.20).
2. Write the history version, with the bytes and the metadata.
3. Write the current document, or remove it for a delete.

A crash after step 2 leaves a history version that is newer than the current document. At open and at each poll, the store finds such a version. It writes the current document of that version, then it continues. So the history is the audit, and the current document never has a change that the history does not hold.

### 2.20 Preconditions and conflicts

`put` takes one precondition:

| Precondition | HTTP form (5.5) | Rule |
|---|---|---|
| `Create` | `If-None-Match: *` | The key has no current document. A key with only a version `delete` is free. Else `AlreadyExists`. |
| `Match(etag)` | `If-Match: "<etag>"` | The current document has this etag. Else `Conflict` with the current etag, or `NotFound` when the key has no current document. |

`delete` takes `Match(etag)` only. The check and the write are one atomic step: two writers with the same base etag cannot both succeed. The second gets `Conflict`, and no data is lost.

### 2.21 Deletes and tombstones

A delete writes a version of the kind `delete`, then removes the current document. The key keeps its versions. A later create of the same key gets the next version number. A poll reports each removed key (2.22).

### 2.22 Load, poll and marks

`load` gives each current document with its key, bytes, etag and version, and the marks of this state. `poll(marks)` gives the documents that changed since the marks, the keys that are gone, and the new marks.

- The marks are opaque for the caller. The backend `files` uses the signature of each file (2.33). The backend `postgres` uses the last change number (2.38).
- A poll never misses a write that ended before the poll started.
- A poll can report a document that did not change. The catalog then compares the etags.
- `load` and `poll` return the bytes as stored. The catalog checks them (2.41).

### 2.23 The read of one environment

`read_environment(env)` gives each current document of the environment in one consistent state: no write is half visible. The runner uses it to build a snapshot (4.9). The backend `files` reads under its process lock. The backend `postgres` reads in one transaction at the level `REPEATABLE READ`.

### 2.24 Secrets

A secret is a value of a secret key of a schema. It has a scope:

| Scope | Text form | Who writes it |
|---|---|---|
| Environment | `environment:<env>` | A subject with `secret.write` on the environment |
| Unit | `environment:<env>/unit:<unit>` | A subject with `secret.write` on the unit |

The name of a secret is a key path. For the scope of an environment, the name MUST be a secret key of at least one schema of the configuration. For the scope of a unit, it MUST be a secret key of the schema of the unit.

- The name `units` is reserved in the scope of an environment. The path `values/environments/<env>/units` is the folder of the unit scopes (2.31). The server answers `422 invalid_secret`, and the backend `files` refuses the name too. A unit scope can use it.
- No route, no command and no log gives the value of a secret. Section 5 gives the metadata only: the name, the fingerprint, the time and the subject of the last write.
- `read_secrets` gives the values. Only the runner calls it, to build the private folder of a run (4.13).
- Each write and each delete of a secret writes one event: the scope, the name, `set` or `delete`, the subject and the time. The events have no value. The store keeps the newest `storage.<backend>.history_keep` events of each secret.
- A secret has no versions. A new value replaces the old one. A secret does not move with a document, so a delete of its environment or unit needs a delete of the secret first (2.16).

### 2.25 The value of a secret

A value is a UTF-8 text of 8 to 65,536 bytes with no U+0000. It MUST also pass the check of the kind of its key in each schema that declares the key (2.12). A shorter value gives `422 invalid_secret`. The bound of 8 bytes keeps the scrubber of 4.21 useful.

### 2.26 The fingerprint and the secret key of the store

The key `storage.secret_key` gives 32 bytes as 64 lower-case hexadecimal characters (6.10). It is a secret: the file accepts it only with `_env` or `_file`. All instances of one store MUST have the same key.

Pupitre derives two keys from it with HKDF-SHA256 (RFC 5869), with the salt `pupitre` and an output of 32 bytes:

| Derived key | `info` | Use |
|---|---|---|
| Encryption key | `pupitre secret encryption v1` | The cipher of the backend `postgres` (2.39) |
| Fingerprint key | `pupitre secret fingerprint v1` | The fingerprint and the key check |

The fingerprint of a secret is the start of an HMAC-SHA256 with the fingerprint key. The input is the UTF-8 bytes of `<scope>` U+000A `<name>` U+000A `<value>`. The fingerprint is the first 16 lower-case hexadecimal characters of the result. Two equal values in two scopes have two different fingerprints. A person who sees the fingerprint cannot test a guess without the key.

The key check is the 64 lower-case hexadecimal characters of HMAC-SHA256 with the fingerprint key over the bytes `pupitre key check v1`. The command `migrate` writes it in the store (2.30). At open, the store compares it with the key of the configuration. A different value stops the start with the message `storage.secret_key does not match the key of the store`.

### 2.27 Runs in the store

The store keeps each run record (4.38) in the canonical form. These are the rules of the methods:

- `create_run(record, max_queued, consumes)`: the record has the status `queued`. The store refuses the record with `QueueFull` when `max_queued` runs have the status `queued`. With `consumes = Some(check)`, the store sets `applied_by` of the check run to the new id in the same atomic step. It refuses with `CheckUsed` when `applied_by` of the check is present.
- `get_run(id)` and `list_runs(filter, before, limit)`: the list is in the order of the id, newest first. The filter has an optional environment and an optional status.
- `list_runs_page(filter, before, skip, limit)`: the same list, with the first `skip` runs left out. The backend copies only the runs of the page.
- `next_queued(busy, limit)`: the oldest queued runs whose environment is not in `busy`, at most `limit` runs.
- `update_run(expected, record)`: the store replaces the record when its stored status is `expected`. The new status is `expected` or a status after it in 4.39. Another stored status gives `Conflict`, and another new status gives `BadTransition`.
- `prune_runs(keep)`: remove the oldest finished runs above `keep`, with their output and events.

### 2.28 The output and the events of a run

- `append_output(id, offset, bytes)`: the store adds the bytes at `offset`. `offset` MUST be the current length of the output. Another value gives `Conflict` with `current` empty, because `StoreError::Conflict` carries only an etag. The caller then reads the current length with `read_output`. So a second call after a crash adds nothing two times.
- `read_output(id, offset, max)`: the bytes from `offset`, at most `max` bytes, and the current length.
- `append_events(id, first_seq, lines)`: the store adds the event lines with the numbers from `first_seq`. `first_seq` MUST be the next number. Another value gives `Conflict` with `current` empty.
- `read_events(id, after, limit)`: the lines with a number above `after`, at most `limit` lines.

The four methods give `NotFound` for a run that does not exist. The runner scrubs the output and the events before it calls these methods (4.21). The store never holds a secret value of a run.

### 2.29 Leases

A lease is a name, a holder and an end time. The names are `runner` (4.8) and `environment:<env>` (4.7).

- `acquire_lease(name, holder, ttl)`: the store gives the lease when it has no holder, when its end time is past, or when the holder is the same. The same holder gets a new end time. Else the store gives the holder and the end time.
- `renew_lease(name, holder, ttl)`: the store moves the end time when the holder is the same and the end time is in the future. Else it gives `Lost`.
- `release_lease(name, holder)`: the store removes the lease when the holder is the same.

The backend `postgres` uses the clock of the database. The backend `files` keeps the leases in the memory of its one process.

### 2.30 The version of the store schema

Each store has a schema version. This version of Pupitre uses the store schema 1.

- At open, the store reads the version. A store with no version, or with an older version, stops the start with the message `run pupitre migrate`. A newer version stops the start with the message `the store is newer than this binary`.
- The command `migrate` (6.24) makes the store schema, or changes it to the version of the binary. It also writes the key check of 2.26. It never runs at start.
- `migrate` on a store that has the version of the binary changes nothing and exits with 0.
- For the backend `files`, `migrate` makes a folder that does not exist when its parent exists. It writes the two small files `pupitre-store.json` and `pupitre-secrets.json`. It takes the locks of 2.32, so it stops when a server runs.

### 2.31 The backend `files`: layout

The backend uses three folders that the configuration names (6.10). The three paths are absolute. No path is inside another one.

```text
<storage.files.path>/                       the documents; it can be a Git checkout
  pupitre-store.json                         {"format": 1, "store_schema": 1}
  .lock
  environments/<env>/environment.json
  environments/<env>/common.json
  environments/<env>/hosts/<host>.json
  environments/<env>/units/<unit>/unit.json
  environments/<env>/units/<unit>/values.json
  environments/<env>/units/<unit>/hosts/<host>/values.json
  .history/<key>/<version>.json             the bytes of the version (absent for a delete)
  .history/<key>/<version>.meta.json        the metadata of the version
  .tmp/                                      temporary files of the writes
<storage.files.secrets_path>/               mode 0700, outside Git
  pupitre-secrets.json                       {"format": 1, "key_check": "<64 hex>"}
  .lock
  values/<scope folder>/<name>               mode 0600, the bytes of the value
  meta/<scope folder>/<name>.json            the time and the subject of the last write
  events/<scope folder>/<name>.jsonl         the events of 2.24
<storage.files.runs_path>/                  the runs
  .lock
  <run id>/run.json
  <run id>/output.log
  <run id>/events.jsonl
```

- `<key>` is the document key of 2.3. `<version>` has 10 digits with zeros at the start, for example `0000000012`.
- `<scope folder>` is `environments/<env>` or `environments/<env>/units/<unit>`.
- The metadata of a version is a canonical object with the members `etag` (absent for a delete), `kind`, `message`, `saved_at` and `saved_by`. `saved_at` has the form `2026-11-24T10:00:00.000Z`.
- An entry under `environments/` that is not a file of this layout is ignored, with the event `store.entry_ignored`.
- In this backend, a secret name has 249 bytes at most. A file name has 255 bytes at most, and the longest suffix is `.jsonl`. A longer name gives an error at the write.

### 2.32 The backend `files`: lock and writes

- At open, the store takes an exclusive `flock` on the file `.lock` of each of the three folders, with no wait. A lock that it cannot take stops the start with the message `the folder <path> is locked by another process`. A file system with no `flock` also stops the start. The store keeps the locks until the process ends.
- A write of a file writes a new file in `.tmp/` with `O_EXCL` and calls `fsync` on it. Then it renames the file to its place and calls `fsync` on the folder. So a reader never sees half a file.
- The store opens each path below its folders with `O_NOFOLLOW` on each segment. A symbolic link is an error. Each path comes from ids of 2.1, so no path holds `..`.
- The store writes the files with the mode 0644 and the folders with the mode 0755 in the documents folder. It writes 0600 and 0700 in the secrets folder. It writes 0640 and 0750 in the runs folder.

### 2.33 The backend `files`: changes outside Pupitre

A person can change the documents folder outside Pupitre, for example with `git pull`. The store finds the changes at each poll, at the period `storage.files.rescan_interval`.

- The signature of a file is its device, its inode, its size and its two change times. A file with a new signature is read again.
- A changed document gets a version of the kind `external` when it is valid. It is valid when it passes the strict reader (2.5) and the rules of its kind (2.7 to 2.10). It must also be canonical and not exceed the size limit (2.42). The version has `saved_by` equal to `external` and an empty message. A removed document gets a version `delete` with `saved_by` equal to `external`.
- The store does not check the schemas. A document that fails only a schema check also gets a version `external`, and the catalog shows its error.
- A changed document that is not valid stays out of the catalog. The catalog shows the error (2.41), and the store writes no version.
- The store writes no file of `environments/` for a change outside Pupitre.
- A metadata file of `.history` that is not valid does not stop the store. The store ignores the entry, with the event `store.entry_ignored`. It uses the newest version that it can read. The store trusts the folder `.history` as much as the documents folder (7.11).

### 2.34 The backend `files`: secrets

The value of a secret is the content of its file, with no line end added. The store reads a value only in `read_secrets`. It writes a value with the steps of 2.32, with the mode 0600.

A value file above 1 MiB gives the error `Corrupt`. The store never gives a cut value. Section 7.11 tells that the values are in clear text on the disk of this backend.

### 2.35 The backend `files`: runs

The store writes `run.json` with the steps of 2.32 at each change. It adds to `output.log` and `events.jsonl` with `O_APPEND`, then it calls `fsync`. At open, it reads each `run.json`. A crash during an append can leave a partial last line in `events.jsonl`. The store cuts this line and counts the lines that remain. It makes no other check of the output files: a caller adds at the length that it reads (2.28).

A file of events above 1 GiB gives the error `Corrupt` at open and at read. The store never cuts it.

### 2.36 The backend `postgres`: tables

The file `crates/pupitre-store/migrations/0001.sql` creates the tables of the store schema 1. It is part of this specification. The tables are `pupitre_meta`, `documents`, `document_versions`, `secrets`, `secret_events`, `runs`, `run_output`, `run_events` and `leases`, with the sequence `pupitre_change_seq`.

The account of `serve` needs `SELECT`, `INSERT`, `UPDATE` and `DELETE` on these tables, and `USAGE` on the sequence. It needs no right to change a table. The account of `migrate` owns the tables.

### 2.37 The backend `postgres`: writes

- A write of a document is one transaction. It takes the barrier of 2.38 in shared mode. It locks the row of the key with `SELECT ... FOR UPDATE` and checks the precondition. Then it inserts the version and writes the row of `documents` with a new value of `pupitre_change_seq`.
- A delete writes `bytes` and `etag` to `NULL` in `documents`. The row stays, so that a poll sees the delete.
- `create_run` with `consumes` is one transaction that locks the row of the check run.
- A statement has the timeout `storage.postgres.statement_timeout`. A pool of `storage.postgres.max_connections` connections serves all calls. A call that waits longer than `storage.postgres.connect_timeout` for a connection fails with `Unavailable`.

### 2.38 The backend `postgres`: poll

The marks are the highest `change_seq` that the load or the poll saw. A poll reads the rows with a `change_seq` above the marks.

A transaction takes a value of the sequence before its commit. A slow transaction can thus commit a lower value after a faster one. A poll that ran between the two commits would skip the slow row for good. The backend prevents this with a barrier. The barrier is one advisory lock of the database with the key `0x5075706974726332`.

- Each writer takes the lock in shared mode before it takes a value of the sequence. This includes the import of 2.40. The lock ends with the transaction of the writer.
- The load and the poll take the lock in exclusive mode, then read the rows with one statement, then end their transaction.
- The exclusive lock waits for each writer that began before it. When the poll reads, no writer holds a value that is not committed. A writer that begins later waits for the end of the read, so its value is above each value that the poll saw.
- Writers do not wait for each other. A poll waits as long as the slowest writer that began before it. A poll that waits longer than `storage.postgres.statement_timeout` fails with `Unavailable`, and the catalog keeps its state and tries again.

The catalog compares the etags.

### 2.39 The backend `postgres`: the cipher of the secrets

The backend encrypts each value with ChaCha20-Poly1305 (RFC 8439) and the encryption key of 2.26.

- The nonce has 12 random bytes. Each write makes a new nonce.
- The associated data is the UTF-8 bytes of `<scope>` U+000A `<name>` U+000A `<fingerprint>`. The fingerprint is the one of 2.26, in 16 lower-case hexadecimal characters. A value thus cannot move to another row. An old nonce and ciphertext cannot stay under the fingerprint of a new value.
- The row keeps the nonce, the ciphertext with its tag, and the fingerprint.
- When the store reads a value for a run, it decrypts with the fingerprint of the row. Then it computes the fingerprint of the value again. A difference gives `SecretUnreadable`.
- A person who writes the database can still put back a whole old row: the nonce, the ciphertext and the fingerprint together. This rollback gives the old value with its old fingerprint. Section 7.11 lists it.
- The schema 1 has this format from the first release. The store has no migration for it.
- A value that does not decrypt stops the run that needs it, with the reason `secret_unreadable` (4.23). The value is never sent in a log.

The key is not in the database. A copy of the database without the key file gives no value.

### 2.40 Export, import and copy

The transfer moves all documents with their versions, all secrets with their events, and all runs with their output and events. It also moves the events of deleted secrets, with no value. It uses `export_begin` on the source and `import_begin` on the target.

- The target MUST be empty: no document, no secret and no run. Else the command stops before it writes.
- `copy-store` (6.24) copies a store to another store, of any backend. The secrets go through the memory of the process. The source decrypts or reads them. The target encrypts or writes them with its own key.
- `export` (6.24) writes the documents and their versions into an empty folder, in the layout of 2.31 for `storage.files.path`. It writes no secret and no run. The result is a folder for a review in Git.
- The transfer keeps the version numbers, the metadata and the etags. After the transfer, the command compares the etag of each current document of the two stores. A difference stops the command with exit code 1.

### 2.41 The catalog

The catalog keeps the current documents in memory. The server reads it for each request. The runner does not read it: it uses `read_environment` (2.23).

- At start, the catalog calls `load`. Then it calls `poll` at the period of the backend. After a write of the instance, it puts the saved document in at once.
- For each document, it keeps the key, the etag, the version, and the parsed document or the list of its errors. A document is valid when it passes 2.5 and the rules of its kind (2.7 to 2.10). An entry with errors is visible to the page, with its errors, and never used for a run.
- It builds the tree of 3.7: the environments, their hosts and their units, and the hosts of each unit. A unit whose `hosts` names a host with no document has an error for that host.
- It checks the limits of 2.42. A load or a poll above a limit keeps the old state and writes the event `catalog.limit_exceeded`. The probe `/readyz` then fails (5.20).
- At start, the old state is an empty catalog. A load above a limit does not stop the start: the server starts, it shows no document, and it loads again at each poll period. The server stops only when the store does not answer.
- While the catalog has no load within the limits, a `POST` or a `PUT` gives `503 store_over_limit`, with the member `limit`. The member `retryable` is `false`, because a try again cannot help. So no write can go above a limit on a state that is empty or old.
- Two kinds of write stay possible: the cancel of a run, and a `DELETE` of a document or of a secret. They cannot add a document. The catalog must have a load. A catalog that never had a load has no document to delete, so it refuses each write.
- The old state does not show a document that arrived above the limit. An example is a file that a Git pull added to the folder of the backend `files`. A `DELETE` of a visible document reduces the count. The next poll then loads the store again, and the catalog is ready.
- When the extra documents are not visible, an operator removes them in the store, outside the API. For the backend `files`, delete the files. For the backend `postgres`, delete the rows. The next poll recovers the catalog.

### 2.42 The limits of the store

The configuration block `limits` gives the limits (6.12). The server checks them at each write. The catalog checks them at each load and each poll.

| Key | Default | What it bounds |
|---|---|---|
| `limits.max_environments` | 64 | Environments of the store |
| `limits.max_units_per_environment` | 1000 | Units of one environment |
| `limits.max_hosts_per_environment` | 1000 | Hosts of one environment, and hosts of one unit |
| `limits.max_document_bytes` | 262144 | Canonical bytes of one document |
| `limits.max_store_bytes` | 268435456 | Canonical bytes of all current documents |

The format of 2.10 bounds the values of one document to 1,024 keys. The format of 2.11 bounds a schema to 1,024 keys. These two bounds are part of the formats, not keys.

### 2.43 The contract suite

The module `conformance` of `pupitre-store` (feature `conformance`) holds one suite. It runs on each backend. These are its cases:

| Case | Rule |
|---|---|
| S01 | A create then a `get` give the same bytes, etag and version 1. |
| S02 | A save with the current etag makes version 2. A save with an old etag gives `Conflict` with the current etag, and changes nothing. |
| S03 | Two saves at the same time with one base etag: exactly one succeeds. |
| S04 | A create on a key with a current document gives `AlreadyExists`. A create after a delete gives the next version. |
| S05 | A delete writes a version `delete`. `get` gives `NotFound`. A poll reports the key as gone. |
| S06 | `history` gives the versions newest first, with pages. `version` gives the bytes of each version. |
| S07 | `history_keep` removes the oldest versions and keeps the numbers. |
| S08 | A crash between the version and the document (a fault hook of the backend) gives the document of the version at the next open (2.19). A backend with atomic writes, such as `postgres`, leaves nothing. The suite accepts both results and checks that the document is the newest version. |
| S09 | A poll after a write of another handle reports the document. A poll with no change reports nothing new. |
| S10 | `read_environment` never shows half of a write. |
| S11 | A secret write, a list, a read for a run, a delete. The list never holds the value. The events have no value. |
| S12 | The fingerprint of a known key, scope, name and value equals the vector of the suite. |
| S13 | A wrong `storage.secret_key` stops the open. |
| S14 | `create_run` above `max_queued` gives `QueueFull`. Two applies that consume one check: exactly one succeeds. |
| S15 | `update_run` with a wrong expected status gives `Conflict`. A transition outside 4.39 is refused. |
| S16 | `append_output` and `append_events` with a wrong offset or number give `Conflict` with `current` empty, and add nothing. |
| S17 | A lease: acquire, a second acquire gives the holder, renew, the end time passes, a second holder acquires, the first renew gives `Lost`. |
| S18 | `prune_runs` removes the oldest finished runs with their output. It never removes a run with the status `queued` or `running`. |
| S19 | An export from one backend and an import into the other give the same etags and versions. |
| S20 | An old store schema stops the open with `run pupitre migrate`. `migrate` then makes it current. |
