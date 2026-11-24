# Pupitre specification, part 3: access

This part uses ASD-STE100 Simplified Technical English. Part 1 gives the rules of the document.

## 3. Access

### 3.1 Principle

Pupitre is a PEP of the ProcurAuth protocol, version 1.1, in the mode `authzen`. It uses the crates `procurauth-pep` and `procurauth-core` of the repository `ProcurAuth/procurauth`, at one full commit (8.9). Where the library does not cover a rule of the protocol, the code of Pupitre does it, and this part gives the rule.

ProcurAuth is optional. The mode `open` asks no question. The block `access` is mandatory, and its key `mode` has no default (6.11). So no instance gets the mode `open` by error.

### 3.2 The two modes

| | `open` | `authzen` |
|---|---|---|
| Token | None. The subject is `-`. | An identity token on each request, in the header `access.identity.header` (3.4). |
| Who guards the page | The reverse proxy | The edge and the PDP |
| Questions | None | One or more for each operation (3.12) |
| `run.apply` and `command.run` | Refused with `403 apply_disabled`, unless `runner.allow_apply_in_open_mode = true` (3.22) | Decided by the PDP |
| Action document and options route | `404 not_found` | Served (3.11, 3.16) |
| Start | The log line `access.mode_open` at each start, at the level `warn` | The PDP link needs TLS, or `access.pdp.plain_http = true` with a warning at start |

### 3.3 What each mode serves

The mode never changes the routes of section 5, except the two ProcurAuth routes. A feature that the configuration turns off is absent: its routes give `404 not_found`, and the action document does not publish its actions (P 5.2a).

| Configuration | Published actions | Effect on the routes |
|---|---|---|
| `runner.enabled = true` | The eight actions of 3.9 | All routes |
| `runner.enabled = false` | `config.read`, `config.write`, `secret.write`, `inventory.write` | The routes under `/api/runs` and the run pages give `404 not_found` with no question. `GET /api/catalog` has no `commands` and no `playbooks` (5.15). |

### 3.4 The identity token

The block `access.identity` gives the values of P 9.4 (6.11). They are the token header, the issuer, the service id, the public keys, the maximum age and the clock skew. The checks are the checks of P 9.4:

- The algorithm is `Ed25519` or `ES256`. The header `typ` is `JWT`.
- `iss` equals `access.identity.issuer`. `aud` is `access.identity.audience`, or an array that holds it.
- `iat`, `exp` and `nbf` are JSON integers with digits only (P 9.3). `exp` minus `iat` is not above `access.identity.max_age`.
- The skew `access.identity.clock_skew` applies to `iat`, `exp` and `nbf`.
- With one public key, the token can have no `kid`. With more than one public key, the token MUST have a `kid` of the set.
- A JWK Set with more than one key MUST give a different `kid` to each key. Else the start stops (6.13).
- Pupitre gives each key of a PEM text the RFC 7638 thumbprint as its `kid`. A token for a PEM key names it in `kid`, or has no `kid` when the text holds one key.
- The `sub` of the token MUST NOT be `-`, `external` or `import`. These are the subjects of the history (2.18). Such a token gives `401 unauthenticated`.
- The service reads the token only from the configured header, never from `Authorization`, a cookie or a query parameter. Two copies of the header give `401 unauthenticated`.

Pupitre sends only `sub` to the PDP (P 9.6). It never uses `owner` in a decision (P 9.5). It writes `owner` in the log line `access.decision` when the token has it.

### 3.5 The link to the PDP

The block `access.pdp` gives the URL, the credential, the CA file, `plain_http`, the timeout, the cache time and the pool size (6.11).

- The link uses TLS with the CA file of `access.pdp.ca_file`. A URL with the scheme `http` needs `access.pdp.plain_http = true`. Pupitre then writes the warning `access.pdp_plain_http` at start.
- The credential goes in `Authorization: Bearer` (P 10.0, step 4). It is a secret of the configuration.
- At most `access.pdp.max_connections` requests go to the PDP at one time. A request that waits longer than `access.pdp.timeout` for a place fails with `503 pdp_unavailable`.
- Pupitre reads the metadata of the PDP (`/.well-known/authzen-configuration`) at the first question. It checks the bounds that the PDP publishes as local bounds (P 10.7a).

### 3.6 The service id

The service id is `access.identity.audience`. It is the member `server` of the action document (P 5.2), and the id of the resource of type `service`. It follows P 4.1: 1 to 256 bytes of UTF-8 with no control character.

### 3.7 The resource tree

```text
service                      access.identity.audience
└── environment              the id of an environment          depth 1
    └── unit                 the id of a unit                  depth 2
        └── host             the id of a host of the unit      depth 3
```

The tree uses 3 of the 4 levels that P 4.1 permits. A host node is always below a unit: the same host in two units is two nodes. A grant on a node covers each node below it, also a node that a person adds later (P 4.3). So a grant of `run.apply` on `environment:production` covers each unit and each host of `production`, now and later.

The document `host` (2.8) has no node of its own. Its routes use the node of its environment (3.12).

### 3.8 The chain and the node path

Pupitre sends the node and the chain of its ancestors in `resource.properties.ancestors` (P 4.1). The chain starts at the service and ends at the parent of the node. Example, the host `host-a` of the unit `relay` of `production`, for the service `pupitre`:

```json
{ "type": "host", "id": "host-a",
  "properties": { "ancestors": [ { "type": "service", "id": "pupitre" },
                                 { "type": "environment", "id": "production" },
                                 { "type": "unit", "id": "relay" } ] } }
```

The node path is the path form of P 5.2, without the service: `environment:production/unit:relay/host:host-a`. The ids of 2.1 need no percent escapes. The API, the run records and the logs use this text.

The chain comes from the catalog (2.41), never from a value of the request (P 4.5). A node path in a request body names a node. The server reads it, finds the node in the catalog, and builds the chain from the catalog.

### 3.9 The actions

| Action | `resource_types` | Ceilings | What it permits |
|---|---|---|---|
| `config.read` | `service`, `environment`, `unit`, `host` | none | Read the documents, the effective values, the provenance, the history and the metadata of the secrets. Never a secret value. |
| `config.write` | `service`, `environment`, `unit`, `host` | none | Write the values of a layer: `common` on an environment, `unit_values` on a unit, `host_values` on a host. |
| `secret.write` | `service`, `environment`, `unit` | none | Set and delete a secret of the scope. No action reads a secret value. |
| `inventory.write` | `service`, `environment` | none | Create, change and delete environments, hosts and units. |
| `run.read` | `service`, `environment`, `unit`, `host` | none | Read the runs, their events and their output. |
| `run.check` | `service`, `environment`, `unit`, `host` | `max_hosts` | Start a check run or a status run of a unit. Cancel such a run. |
| `run.apply` | `service`, `environment`, `unit`, `host` | `max_hosts` | Start an apply run of a unit. Cancel such a run. |
| `command.run` | `service`, `environment`, `unit`, `host` | `max_hosts` | Start a command of the catalog, in check or apply. Cancel such a run. |

`run.read`, `run.check`, `run.apply` and `command.run` are published only with `runner.enabled = true` (3.3).

### 3.10 The ceiling `max_hosts`

`max_hosts` is the most hosts of one run. The action document declares it with `min` 1 and `max` equal to `runner.max_hosts` (6.12).

- The effective bound of a run is the lowest of two values (P 7.3, P 8.1). The first is `runner.max_hosts`. The second is `context.limits.max_hosts` of the answer to the use, when the answer has it.
- The count is the length of `hosts` of the run request (4.4).
- A count above the effective bound gives `403 ceiling_exceeded`, with the members `ceiling` and `limit` (5.4). Pupitre never runs a part of the hosts.

### 3.11 The action document

`GET` and `HEAD /.well-known/procurauth/actions` give the action document (P 5.1). The route needs no token and asks nothing. Pupitre builds the document one time at start with the builder of the library. It serves the same bytes at each read, with `Content-Type: application/json` and `Cache-Control: no-store`. In the mode `open`, the route gives `404 not_found`.

The document for `audience = "pupitre"`, `runner.enabled = true` and `runner.max_hosts = 100`:

```json
{
  "server": "pupitre",
  "resource_types": [
    { "name": "environment", "parent": "service",
      "description": "One environment: a set of hosts and units." },
    { "name": "unit", "parent": "environment",
      "description": "One deployed service of an environment." },
    { "name": "host", "parent": "unit",
      "description": "One host of a unit." }
  ],
  "actions": [
    { "name": "config.read", "resource_types": ["service", "environment", "unit", "host"],
      "description": "Read the documents, the effective values and the history. Never a secret value." },
    { "name": "config.write", "resource_types": ["service", "environment", "unit", "host"],
      "description": "Write the values of the layer of the node." },
    { "name": "secret.write", "resource_types": ["service", "environment", "unit"],
      "description": "Set and delete the secrets of the node. No action reads a secret." },
    { "name": "inventory.write", "resource_types": ["service", "environment"],
      "description": "Create, change and delete environments, hosts and units." },
    { "name": "run.read", "resource_types": ["service", "environment", "unit", "host"],
      "description": "Read the runs, their events and their output." },
    { "name": "run.check", "resource_types": ["service", "environment", "unit", "host"],
      "description": "Start a check run or a status run.", "limits": ["max_hosts"] },
    { "name": "run.apply", "resource_types": ["service", "environment", "unit", "host"],
      "description": "Start an apply run after a check run that passed.", "limits": ["max_hosts"] },
    { "name": "command.run", "resource_types": ["service", "environment", "unit", "host"],
      "description": "Start a command of the configuration file.", "limits": ["max_hosts"] }
  ],
  "limits": [
    { "name": "max_hosts", "label": "Most hosts of one run",
      "description": "The most hosts that one run or one command can reach.",
      "unit": "hosts", "kind": "ceiling", "min": 1, "max": 100 }
  ]
}
```

Rules:

- `server` is `access.identity.audience`.
- The three types never change. The document has no member `resources`: the environments, the units and the hosts change while Pupitre runs, so the options route gives them (3.16).
- With `runner.enabled = false`, the document has the first four actions and no member `limits` (P 5.2a).
- The document has no member `approval` (P 5.2b).
- The document is less than 8 KiB, with 8 actions, 3 types and 1 limit, within P 5.3c.

### 3.12 The routes and their questions

Notation: S is the service. E(e) is the node `environment:e`. U(e,u) is the node `environment:e/unit:u`. H(e,u,h) is the node `environment:e/unit:u/host:h`. N(r) is the node of the run r (4.38).

"Use" is the permission of the operation (P 5.5a). "Display" means the questions of the display filter of 3.15. Section 5 gives the routes.

| Route | Use |
|---|---|
| `GET /api/session`, `GET /api/catalog`, pages, static files | `access` on S (3.20) |
| `GET /api/environments` | Display, on each E(e) |
| `POST /api/environments` | `inventory.write` on S |
| `GET /api/environments/{e}` | `config.read` on E(e) |
| `PUT`, `DELETE /api/environments/{e}` | `inventory.write` on E(e) |
| `GET /api/environments/{e}/hosts`, `GET .../hosts/{h}` | `config.read` on E(e) |
| `PUT`, `DELETE /api/environments/{e}/hosts/{h}` | `inventory.write` on E(e) |
| `GET /api/environments/{e}/units` | Display, on each U(e,u) |
| `GET /api/environments/{e}/units/{u}` | `config.read` on U(e,u) |
| `PUT`, `DELETE /api/environments/{e}/units/{u}` | `inventory.write` on E(e) |
| `GET /api/environments/{e}/common` and its history | `config.read` on E(e) |
| `PUT`, `DELETE /api/environments/{e}/common` | `config.write` on E(e) |
| `GET /api/environments/{e}/units/{u}/values` and its history | `config.read` on U(e,u) |
| `PUT`, `DELETE /api/environments/{e}/units/{u}/values` | `config.write` on U(e,u) |
| `GET /api/environments/{e}/units/{u}/hosts/{h}/values` and its history | `config.read` on H(e,u,h) |
| `PUT`, `DELETE /api/environments/{e}/units/{u}/hosts/{h}/values` | `config.write` on H(e,u,h) |
| `GET /api/environments/{e}/units/{u}/effective` | `config.read` on U(e,u) |
| `GET /api/environments/{e}/units/{u}/effective?host={h}` | `config.read` on H(e,u,h) |
| History of the documents `environment`, `host` and `unit` | `config.read` on E(e), E(e) and U(e,u) |
| `GET /api/environments/{e}/secrets` | `config.read` on E(e) |
| `PUT`, `DELETE /api/environments/{e}/secrets/{name}` | `secret.write` on E(e) |
| `GET /api/environments/{e}/units/{u}/secrets` | `config.read` on U(e,u) |
| `PUT`, `DELETE /api/environments/{e}/units/{u}/secrets/{name}` | `secret.write` on U(e,u) |
| `GET /api/runs` | Display: `run.read` with the match `node` on N(r), for each run |
| `GET /api/runs/{r}`, `.../events`, `.../output` | `run.read` on N(r) |
| `POST /api/runs` | The action of the kind and the mode of the request (4.3), on the node of the request |
| `POST /api/runs/{r}/cancel` | The action of the kind and the mode of r (4.3), on N(r) |
| `GET`, `HEAD /.well-known/procurauth/actions` | None, and no token (3.11) |
| `GET`, `HEAD /.well-known/procurauth/resources` | The questions of 3.16 |

Rules:

- No route has other permissions (P 5.5a). Each operation asks one use.
- No action declares a budget. So no question has `context.usage` (P 6.4).
- `HEAD` of a `GET` route asks the questions of that `GET` route.
- A route of section 5 that this table does not list gives `403 forbidden`, asks nothing, and writes the log line `access.no_rule`. A test of the server fails when a route of the router has no line in this table.

### 3.13 The order of the checks

Each request on the main listener passes these steps in this order. A step that refuses stops the request.

1. Limits of the request line, the headers and the declared body size (7.10), then the check of the header `Host` (7.4). Refusal: 400, 404, 405, 413, 414, 421 or 431.
2. The write guard on each `POST`, `PUT` and `DELETE` (7.4). Refusal: `403 cross_site_request` or `415 unsupported_media_type`.
3. The token check, in the mode `authzen`, on each route except the action document (3.4). Refusal: `401 unauthenticated`.
4. A feature that is off (3.3). Refusal: `404 not_found`, with no question.
5. The body read, within its limit (7.10). Refusal: 408 or 413.
6. The shape of the request: the parameters, the JSON syntax of 2.5 and the closed members of the body. Refusal: `400 bad_request`.
7. The catalog lookup of the node. An unknown node follows 3.14.
8. The decision of the use. Refusal: 403 or 503.
9. The process ceilings: `403 apply_disabled` (3.22) and `403 ceiling_exceeded` (3.10).
10. The validation of the document, of the values or of the run request, and the rules across documents (2.16, 4.4). Refusal: 409 or 422.
11. The effect: the store write or the run record.

A request runs in its own task. A client that closes the connection does not stop the request. The write, the catalog update and the log line stay complete (7.10).

So a subject without the right never reaches the validation of a document, and never learns a rule of a value.

### 3.14 Unknown and forbidden give the same answer

A node, a document or a run that does not exist MUST give no information to a subject without the right to see it. In the mode `authzen`:

1. A route names a node or a run that the catalog or the store does not hold. Pupitre asks the use action of the route on S, with the match `node`. A `false` answer gives `403 forbidden`. A `true` answer gives `404 not_found`. A failure gives `503 pdp_unavailable`.
2. Pupitre asks this question at each request. It never uses a kept answer for it, and it keeps no answer to it (3.17).
3. Before the decision, Pupitre does the same work for an item that exists and for one that does not exist. This work is the body read and the shape check. It reads no store before the decision, except the record of a run of the route (`/api/runs/{r}`), which it reads in both cases.
4. The `403` of rule 1 is the `403` of each other refusal of the route. It has the same status, the same headers and the same body. Its member `action` is the use action of the route.
5. A document key whose node exists, but whose document does not exist, gives `404 not_found` after a `true` answer. Example: a unit with no document `unit_values`.

In the mode `open`, an unknown item gives `404 not_found`.

### 3.15 The display filter

A list route shows only the items that the subject can see. For each item, Pupitre asks the published actions whose `resource_types` hold the type of the item. It asks with the match `subtree` (P 4.4) and stops at the first `true` answer. A `true` answer for `subtree` is for the display only: it never lets an operation run (P 4.4).

- `GET /api/environments` and `GET /api/environments/{e}/units` use this filter.
- `GET /api/runs` asks `run.read` with the match `node` on the node of each run.
- One request asks at most `access.options_max_questions` questions (6.11). When it reaches this bound, the answer holds the items that it found and a `next` cursor (5.6).
- One item can need one question for each action that lists its type: 8 questions with `runner.enabled = true`, and 4 with `runner.enabled = false`. The key `access.options_max_questions` MUST NOT be below this number (C11 in 6.14).
- A list route with the display filter makes one call for a window of `limit` items from the cursor. The page can thus have fewer than `limit` items. It can have no item and a `next` value.
- A failure of the PDP gives `503 pdp_unavailable`, never a list that the failure made shorter.

### 3.16 The options route

`GET /.well-known/procurauth/resources` lists the children of a node, page by page (P 5.6 to P 5.8). It needs a valid token. It has no action of its own. In the mode `open`, it gives `404 not_found`.

| `type` | `parent` | Children | `label` | `has_children` |
|---|---|---|---|---|
| `environment` | absent | The environments of the catalog | The `label` of the environment | `true` |
| `unit` | `environment:<e>` | The units of e | The `label` of the unit | `true` |
| `host` | `environment:<e>/unit:<u>` | The hosts of u | The host id | `false` |

- For each child, Pupitre asks the published actions whose `resource_types` hold the type of the child, with the match `subtree`, in the order of 3.9. It stops at the first `true` answer. A child with a `true` answer is in the list (P 5.6).
- The search `q` keeps the visible children whose id or label holds the text, with no case for ASCII letters. `q` has 256 bytes at most. Pupitre asks the display questions for the children first and applies `q` to the visible ones. So the questions never depend on `q`, and a search cannot show that a hidden name holds the text.
- `limit` is an integer from 1 to `access.options_page_items`. Absent, it is `access.options_page_items`. A value above the bound gives `400 invalid_options_request`. Pupitre never changes the value.
- The budget has the minimum of 3.15: a smaller budget could never end one item (C11 in 6.14).
- The items are in the byte order of the id. The route walks the children and fills the page up to `limit` visible items that the search keeps. A search that keeps few items can give a page with no item and a `next` value. One request asks at most `access.options_max_questions` questions. When the bound stops the walk, the answer holds the items that it found and a `next` value. The answer can have no item and a `next` value.
- The parameter `after` holds the cursor of the previous answer (P 5.7). The cursor is the base64url text, with no padding, of `<type>` U+000A `<parent>` U+000A `<last shown id>` U+000A `<count of skipped children after it>`. The skipped children are the children that the walk examined and did not show. It has 2,048 bytes at most. It never holds the id of a child that the subject cannot see.
- A cursor with no id and a count is the cursor of a page with no shown item. A cursor that this route did not make for the same `type` and `parent` gives `400 invalid_options_request`.
- The query string follows P 5.7: Pupitre decodes each `%XX` one time. A raw `+`, a lower-case percent escape such as `%3a`, and an escape that is not valid give `400 invalid_options_request`. A parameter two times, a request with no `type`, or a `type` that is not the type of the children of `parent` gives `400 invalid_options_request`. An unknown parameter has no effect.
- Pupitre gives each label as it is in the document, with up to 64 characters (2.7, 2.9). P 5.8 asks a caller to accept 60 characters, so a caller MAY treat a longer label as a failed call. So keep a label to 60 characters or less.
- A parent that does not exist gives an answer with no item (P 5.7). A PDP failure gives `503 pdp_unavailable`, never an empty list.

### 3.17 The answer cache

The library keeps the answers of the PDP for `access.pdp.cache_ttl` at most (P 10.1).

- The key is the subject, the action, the resource with its chain, and the match.
- The cache holds `access.pdp.cache_entries` entries at most. When it is full, it keeps no new answer (P 10.1b).
- A failure is never kept. `cache_ttl = "0s"` keeps no answer.
- Pupitre keeps the answer to the question `access` (3.20) itself, for the same time and with the same bound. The key is the subject. Only a `true` answer is kept: Pupitre never keeps a `false` answer.
- The questions of a `POST`, `PUT` or `DELETE` route never use a kept answer, and the cache keeps no answer to them. So a revoked grant stops each write and each run at once.
- The question of 3.14 never uses a kept answer.
- Each instance has its own cache. A revoked grant can stay active on a read route for `cache_ttl` at most.

### 3.18 The PDP down, slow or wrong

Pupitre fails closed (P 10.3). It never sends `context.reason` to the client.

| Case | Answer | Log line |
|---|---|---|
| `true` with no ceiling, or with `max_hosts` on a run action | The operation continues (3.10). | `access.decision` |
| `false` with no reason, or a reason other than those below | `403 forbidden` | `access.decision` |
| `false` with a reason of P 17.1 | `403 approval_cannot_wait` (3.19) | `access.decision` |
| `false` with `budget_exceeded` | `403 forbidden`. Pupitre declares no budget, so a correct PDP never gives it. | `access.decision` |
| `true` with a ceiling that the action does not list, or a ceiling value that is not an integer from `min` | `403 forbidden` (P 7.5) | `access.ceiling_unenforceable` |
| No connection, a timeout, a status other than 200, or a body that is not a valid answer | `503 pdp_unavailable` | `access.pdp_failed` or `access.pdp_invalid_answer` |

A PDP outage never changes `/readyz` (5.20). The action document, `/livez` and `/readyz` still answer during an outage.

### 3.19 Approvals

Each action follows section 17 of the protocol (P 5.2b). A grant with `confirm` makes the PDP answer a reason of P 17.1 to a question of a key.

No route of Pupitre can wait (P 17.7a). The API has no result that tells the caller that an action waits. It has no control argument of P 17.5. Thus:

- When the answer to the use has a reason of P 17.1, Pupitre refuses the call with HTTP 403 and the body `{"error":"approval_cannot_wait"}`. It runs nothing, computes no effect and submits no request.
- A person never gets such a reason (P 17.8). A person thus never sees this refusal.
- A key with a `confirm` grant on `run.apply` thus cannot start an apply through the HTTP API. Section 7.11 lists this limit.

### 3.20 Routes with no action

A route that runs no action of the action document follows P 10.1c:

- The pages, the static files, `GET /api/session` and `GET /api/catalog` need a valid token. For each request, Pupitre asks the action `access` on S, with no `context.usage`. The route runs only after a `true` answer. The answer can be a kept answer.
- The routes of the ops listener (`/livez`, `/readyz`, `/metrics`) need no token and ask nothing. They are on a second listener that the edge MUST NOT reach (7.2).

### 3.21 The errors of the access layer

Section 5.4 owns the closed list of error codes. The access layer uses these codes:

| Status | Code | Cause |
|---|---|---|
| 401 | `unauthenticated` | No token, a token that is not valid, or two copies of the token header |
| 403 | `forbidden` | A `false` answer, an answer that Pupitre treats as `false`, or a route with no rule. The body has `action`. |
| 403 | `approval_cannot_wait` | A reason of P 17.1 (3.19). The body is exactly `{"error":"approval_cannot_wait"}`. |
| 403 | `ceiling_exceeded` | More hosts than the effective `max_hosts` (3.10) |
| 403 | `apply_disabled` | An apply or a command in the mode `open` without `runner.allow_apply_in_open_mode = true` (3.22) |
| 404 | `not_found` | An unknown item after 3.14, a feature that is off, or a ProcurAuth route in the mode `open` |
| 400 | `invalid_options_request` | A query string of the options route that is not valid |
| 503 | `pdp_unavailable` | The PDP gave no valid answer in time |

The member `action` of `forbidden` is the use action of the route. The question that the PDP refused does not change it. So a `403` does not show whether a node exists.

### 3.22 The mode `open`

The mode `open` sends no question. The subject of each request is `-`. These process rules still apply:

- With `runner.allow_apply_in_open_mode = false`, a run request of the mode `apply` and each command request give `403 apply_disabled`. A check run and a status run still run.
- With `runner.allow_apply_in_open_mode = true`, each run request runs.
- `runner.max_hosts` applies to each run (3.10).
- The key `runner.allow_apply_in_open_mode` is forbidden in the mode `authzen` (6.12).

### 3.23 The interface in the code

The module `crates/pupitre/src/access/` holds the trait `Access` and the enum `AnyAccess` with the variants `Open` and `Authzen`. The server calls only this interface. The code of `crates/pupitre/src/access/mod.rs` gives the exact signatures:

| Function | Role |
|---|---|
| `authenticate(values)` | Steps 1 and 2 of P 10.0, with the values of the token header. It gives the subject, or `Unauthenticated`. |
| `decide_access(subject)` | The question `access` of 3.20. |
| `decide(subject, question, policy)` | One use. It gives the ceilings, or a refusal. `policy` tells if a kept answer can serve (3.17). |
| `decide_unknown(subject, action)` | The question of 3.14. It gives `NotFound` or `Forbidden`. |
| `visible(subject, items, budget)` | The display filter of 3.15. It gives the visible items and the place where the budget stopped. |
| `action_document()` | The bytes of 3.11, or none in the mode `open`. |

### 3.24 Conformance

The test run of the access layer runs the series U of the ProcurAuth conformance suite against Pupitre, at the commit of 8.9 (8.12). A required case that fails stops the release. The suite never skips a required case.
