# Pupitre specification, part 5: API and page

This part uses ASD-STE100 Simplified Technical English. Part 1 gives the rules of the document.

## 5. API and page

### 5.1 The listeners

Pupitre opens two listeners. Both use plain HTTP/1.1. Pupitre has no TLS on its listeners: the edge or the proxy does TLS.

| Listener | Key | Routes |
|---|---|---|
| Main | `server.listen` | The pages, the static files, the API and the two ProcurAuth routes |
| Ops | `server.ops_listen` | `/livez`, `/readyz` and `/metrics` (5.20) |

The two listeners have different ports (6.14). The edge MUST NOT reach the ops listener (7.2).

### 5.2 The base path

Each route of the main listener is below `server.base_path` (6.8). With `base_path = "/console/"`, the route `/api/session` is `/console/api/session`. A path outside the base path gives `404 not_found`. The routes of the ops listener have no base path.

Each request of the main listener also needs a header `Host` that `server.allowed_hosts` lists (7.4). Else the answer is `421 misdirected_request`.

### 5.3 Common rules

- A request body is JSON. It needs `Content-Type: application/json`, with an optional parameter `charset=utf-8` (7.4). The strict reader of 2.5 reads it.
- An answer of `/api/` has `Content-Type: application/json; charset=utf-8`, except the output of a run (5.17). It has `Cache-Control: no-store`.
- An answer body is compact JSON, with no line end and no space. The bytes of a document (5.5) and of a version (5.13) are the exception: they use the canonical form of 2.4. Only the options route writes `null`, as P 5.8 says.
- A known path with another method gives `405 method_not_allowed` and an `Allow` header. `OPTIONS` gives `405` with no CORS header.
- `HEAD` of a `GET` route gives the headers of the `GET` and no body.
- Each answer has `X-Request-Id`: the id that Pupitre gives to the request. The log lines of the request have the same id.
- A query parameter that the route does not name gives `400 bad_request`. A parameter two times gives `400 bad_request`. The options route follows its own rule (3.16). A page ignores its query string (5.21).
- Times are texts in the form `2026-11-24T10:00:00.000Z`.

### 5.4 Errors

An error answer has this body:

```json
{"error":"conflict","message":"The document changed after your read.","retryable":false,"etag":"<64 hex>"}
```

- `error` is a code of the table below. `message` is a fixed English text of the code. It never holds a value of the request, a detail of the PDP, a path of the server or a secret. `retryable` is `true` only for the codes of 503. The code `store_over_limit` is the one code of 503 with `retryable` false.
- Some codes add members. The table gives them.
- The code `internal_error` is for a defect that the code finds and reports, for example a store error that cannot happen. The release build uses `panic = "abort"`, so a panic in a handler stops the process. It gives no `500` and no other answer, and a supervisor MUST restart Pupitre (7.11).
- The one exception is `approval_cannot_wait`: its body is exactly `{"error":"approval_cannot_wait"}` (P 17.7a).

| Status | Code | Members | Cause |
|---|---|---|---|
| 400 | `bad_request` | | A request that does not have the shape of its route |
| 400 | `invalid_options_request` | | 3.16 |
| 401 | `unauthenticated` | | 3.4 |
| 403 | `forbidden` | `action` | 3.21 |
| 403 | `approval_cannot_wait` | | 3.19 |
| 403 | `ceiling_exceeded` | `ceiling`, `limit` | 3.10 |
| 403 | `apply_disabled` | | 3.22 |
| 403 | `cross_site_request` | | 7.4 |
| 404 | `not_found` | | An unknown route, item or feature |
| 421 | `misdirected_request` | | 7.4 |
| 405 | `method_not_allowed` | | 5.3 |
| 408 | `request_timeout` | | 7.10 |
| 409 | `conflict` | `etag` (absent when no current document exists) | 2.20 |
| 409 | `already_exists` | | 2.20 |
| 409 | `in_use` | `errors` | 2.16 |
| 409 | `not_empty` | `errors` | 2.16 |
| 409 | `check_required` | | 4.24 |
| 409 | `check_used` | | 4.24 |
| 409 | `check_too_old` | | 4.24 |
| 409 | `run_finished` | | 4.27 |
| 413 | `payload_too_large` | | 7.10 |
| 414 | `uri_too_long` | | 7.10 |
| 415 | `unsupported_media_type` | | 7.4 |
| 422 | `invalid_document` | `errors` | 2.7 to 2.16 |
| 422 | `invalid_secret` | `errors` | 2.24, 2.25 |
| 422 | `invalid_run_request` | `errors` | 4.4 |
| 422 | `limit_exceeded` | `limit` | 2.42 |
| 428 | `precondition_required` | | 5.5 |
| 431 | `headers_too_large` | | 7.10 |
| 500 | `internal_error` | | A defect. The log has the details. |
| 503 | `server_busy` | | 7.10, with `Retry-After: 1` |
| 503 | `pdp_unavailable` | | 3.18 |
| 503 | `store_unavailable` | | The store did not answer (2.41) |
| 503 | `store_over_limit` | `limit` | The store holds more than a limit, so the catalog is not ready. A `PUT` or a `POST` is refused. Not retryable: reduce the count (2.41) |
| 503 | `run_queue_full` | | 4.5, with `Retry-After: 5` |

An item of `errors` has `path` (a key path, a member or a parameter name), `message`, and optional `layer`. A list has 100 items at most, then one item with the path `""` and the message `and <n> more errors`.

### 5.5 Etags and preconditions

- `GET` of a document gives the canonical document as the body (2.4), with `ETag: "<etag>"`.
- `PUT` of a document needs exactly one precondition header: `If-None-Match: *` for a create, or `If-Match: "<etag>"` for a save. No header gives `428 precondition_required`. Two headers, or a value of another form, give `400 bad_request`.
- `DELETE` of a document needs `If-Match: "<etag>"`. It has no body. `DELETE` of an environment or a unit that still has a secret gives `409 not_empty` (2.16).
- The body of `PUT` is `{"document": {...}, "message": "<text>"}`. `message` is optional, and absent means an empty message (2.18).
- The answer of a save is `200`, and of a create `201`. The body is `{"etag": "<etag>", "version": <n>}`, with the header `ETag`. The answer of a delete is `200` with `{"version": <n>}`.
- A precondition that fails gives `409 conflict` with the current etag, `409 already_exists`, or `404 not_found` (2.20).

### 5.6 Lists

A list route takes `limit` and `after`. `limit` is an integer from 1 to `server.max_page_items`. Absent, it is `server.max_page_items`. A value above the bound gives `400 bad_request`. `after` is the `next` value of the previous answer.

The answer is `{"items": [...], "next": "<cursor>"}`. The member `next` is absent on the last page. A cursor is base64url text with no padding, of 1,024 bytes at most. A cursor that the route did not make gives `400 bad_request`.

The display filter of 3.15 can stop a page before `limit` items. The answer then has fewer items and a `next` value. The answer can have no item and a `next` value.

The cursor of a list with the display filter holds the id of the last visible item and the count of hidden items after it. It never holds the id of a hidden item. A count above 10,000 gives `400 bad_request`. The cursor of `GET /api/runs` has the same form, and the other lists have a count of 0.

### 5.7 The session

`GET /api/session` gives the facts that the page needs:

```json
{
  "limits": {"max_document_bytes": 262144, "max_hosts": 100, "max_page_items": 200},
  "mode": "authzen",
  "runner": {"allow_apply": true, "enabled": true},
  "subject": "alice",
  "version": "0.1.0"
}
```

`runner.allow_apply` is `true` in the mode `authzen`, and is the value of `runner.allow_apply_in_open_mode` in the mode `open`. With `runner.enabled = false`, `runner` is `{"allow_apply": false, "enabled": false}`. `limits.max_hosts` is `runner.max_hosts`, and it is 0 with `runner.enabled = false`.

### 5.8 Environments

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/environments` | | List (5.6). Each item: `id`, `label`, `etag`, `version`, optional `errors` |
| `POST /api/environments` | `{"id": "<env>", "document": {...}, "message": "..."}` | `201 {"etag", "version"}` |
| `GET /api/environments/{e}` | | The document `environment` |
| `PUT /api/environments/{e}` | 5.5 | 5.5 |
| `DELETE /api/environments/{e}` | | 5.5 |
| `GET /api/environments/{e}/history` | `limit`, `before` | 5.13 |

`POST` creates the environment. It gives `409 already_exists` when the id exists, and `422 limit_exceeded` above `limits.max_environments`. `PUT` cannot create an environment: an environment that does not exist is not a node, so `PUT` gives the answer of 3.14.

`GET` of a document that fails 2.5 or 2.7 to 2.10 gives `422 invalid_document` with its errors. The list item of such a document has `etag`, `version` and `errors`, and no content. A save with the etag of the list repairs the document.

### 5.9 Hosts

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/environments/{e}/hosts` | | List. Each item: `id`, `document`, `etag`, `version`, optional `errors` |
| `GET /api/environments/{e}/hosts/{h}` | | The document `host` |
| `PUT`, `DELETE /api/environments/{e}/hosts/{h}` | 5.5 | 5.5 |
| `GET /api/environments/{e}/hosts/{h}/history` | `limit`, `before` | 5.13 |

### 5.10 Units

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/environments/{e}/units` | | List. Each item: `id`, `label`, `schema`, `hosts`, `etag`, `version`, optional `errors` |
| `GET /api/environments/{e}/units/{u}` | | The document `unit` |
| `PUT`, `DELETE /api/environments/{e}/units/{u}` | 5.5 | 5.5 |
| `GET /api/environments/{e}/units/{u}/history` | `limit`, `before` | 5.13 |

### 5.11 Values

| Method and path | Document kind |
|---|---|
| `GET`, `PUT`, `DELETE /api/environments/{e}/common` | `common` |
| `GET`, `PUT`, `DELETE /api/environments/{e}/units/{u}/values` | `unit_values` |
| `GET`, `PUT`, `DELETE /api/environments/{e}/units/{u}/hosts/{h}/values` | `host_values` |

Each path also has `/history` (5.13). A `DELETE` removes the layer: the values of the layer below then apply.

### 5.12 Effective values

`GET /api/environments/{e}/units/{u}/effective` gives the view of the unit: the layers 0 to 2 (2.13). With the parameter `host={h}`, it gives the view of the unit on the host: the layers 0 to 3.

```json
{
  "environment": "production",
  "host": "host-b",
  "keys": [
    {"kind": "integer", "layer": "host", "layers": {"default": 100, "environment": 200, "host": 500, "unit": 300},
     "path": "limits.max_clients", "secret": false, "value": 500},
    {"kind": "text", "layer": "unit", "layers": {"unit": {"fingerprint": "3f9a0c1d2e4b5a69"}},
     "path": "storage.password", "secret": true}
  ],
  "schema": "relay",
  "unit": "relay"
}
```

The members of each key are those of 2.14. A host that is not in the unit gives `404 not_found` after the decision.

### 5.13 History

`GET <document path>/history` lists the versions of the document, newest first. `limit` is from 1 to `server.max_page_items`. `before` is a version number: the list starts below it. Each item has `version`, `kind`, `saved_by`, `saved_at`, `message` and `etag` (absent for a delete). The answer has `next` with the next `before` value, or no `next` on the last page.

`GET <document path>/history/{n}` gives the bytes of the version `n`, with its `ETag`. A version `delete` gives `404 not_found`. The history of a key that has no current document and no version gives `404 not_found`.

### 5.14 Secrets

| Method and path | Body | Answer |
|---|---|---|
| `GET /api/environments/{e}/secrets` | | `{"items": [{"name", "fingerprint", "updated_at", "updated_by"}]}`, in the byte order of `name` |
| `PUT /api/environments/{e}/secrets/{name}` | `{"value": "<text>"}` | `200 {"fingerprint": "<16 hex>"}` |
| `DELETE /api/environments/{e}/secrets/{name}` | | `200 {}` |
| The same three routes under `/api/environments/{e}/units/{u}/secrets` | | |

- No answer holds a value. A `PUT` has no precondition: a new value replaces the old one.
- A name that is not a secret key of 2.24 gives `422 invalid_secret`. A value that fails 2.25 gives `422 invalid_secret`, and the message never holds the value.
- A `DELETE` of a secret that does not exist gives `404 not_found`.
- The log line of a write holds the scope, the name, the subject and the new fingerprint. It never holds the value.

### 5.15 The catalog

`GET /api/catalog` gives the catalog of capabilities (4.2), with no path:

```json
{
  "commands": [
    {"check": false, "id": "restart-service", "label": "Restart the service",
     "params": [{"label": "Grace period", "max_duration": "5m", "min_duration": "1s", "name": "grace",
                 "required": true, "type": "duration"}],
     "playbook": "restart-service", "target": "unit"}
  ],
  "playbooks": [{"id": "deploy-relay", "kind": "deploy"}],
  "schemas": [{"group": "relay_hosts", "id": "relay", "keys": [], "variable": "relay_config"}]
}
```

Each schema has its keys in the form of 2.11. With `runner.enabled = false`, the answer has no `commands` and no `playbooks`.

### 5.16 Runs

| Method and path | Body or parameters | Answer |
|---|---|---|
| `POST /api/runs` | The run request of 4.4 | `201` with the run record (4.38) and `Location: <base>api/runs/<id>` |
| `GET /api/runs` | `environment`, `status`, `limit`, `after` | List of run records, newest first (5.6) |
| `GET /api/runs/{r}` | | The run record |

### 5.17 Events and output of a run

`GET /api/runs/{r}/events?after=<seq>&limit=<n>` gives the event lines with a `seq` above `after`. `after` is an integer from 0, and absent means 0. `limit` is from 1 to `server.max_page_items`. Absent, it is `server.max_page_items`. The answer is `{"items": [...], "last_seq": <n>, "complete": <bool>}`.

- Each item is a stored event line as JSON. A line that is not JSON shows as `{"kind":"invalid","seq":<n>}`.
- `last_seq` is the highest `seq` of the page. It equals `after` when no line comes.
- `complete` is `true` when the run is final and the page has fewer than `limit` lines.

`GET /api/runs/{r}/output?offset=<n>&limit=<bytes>` gives the bytes of the output from `offset`. `offset` is an integer from 0, and absent means 0. `limit` is from 1 to 1,048,576, and absent means 1,048,576. The answer has `Content-Type: application/octet-stream` and these headers:

| Header | Value |
|---|---|
| `X-Pupitre-Output-Length` | The current length of the stored output |
| `X-Pupitre-Run-Complete` | `true` when the run is final, else `false` |

The page asks again each second while `X-Pupitre-Run-Complete` is `false`. The server holds no request open.

### 5.18 Cancel

`POST /api/runs/{r}/cancel` has no body. It follows 4.27. The answer is `200` with the run record. A `queued` run also gets the member `cancel`. A run that has the member `cancel` already gives `200` with the record, unchanged. When another write changes the status at the same time, the server reads the run again three times at most, then it gives `409 conflict`.

### 5.19 The ProcurAuth routes

| Route | Rule |
|---|---|
| `GET`, `HEAD /.well-known/procurauth/actions` | 3.11 |
| `GET`, `HEAD /.well-known/procurauth/resources` | 3.16 |

These two routes are below `server.base_path`, so the action document has the origin and the base path of the service. In the mode `open`, both give `404 not_found`.

### 5.20 The ops listener

| Route | Answer |
|---|---|
| `GET /livez` | `200` with the body `ok`, while the process serves |
| `GET /readyz` | `200` with `ok` when each check of the list below holds, else `503` with the name of the first check that fails as the body |
| `GET /metrics` | The metrics of 6.28 in the Prometheus text format 0.0.4 |

The checks of `/readyz` are three. The store is open (`store`). The catalog has a load within the limits (`catalog`). On the runner node, the runner has started (`runner_lease`). A PDP outage never changes `/readyz`.

The check `store` is the result of the last poll of the catalog. The flag of `runner_lease` is true after the start of the runner, and it never changes later. The process exits when it loses the lease `runner` (4.8).

### 5.21 The pages

The page is plain HTML, CSS and JavaScript in the binary. It has no framework, no build step and no request to another origin. It copies the components of Hune, and the tokens of ClickHouse Dash through Hune. It invents no screen. The page uses pages, not popups, except the confirmations of 4.35.

| Path | Page | Content | Hune or Dash component |
|---|---|---|---|
| `/` | Environments | The environments that the subject sees, with a search | The list page of Hune: `list-page`, `list-search`, `list-table`, `list-empty` |
| `/e/{e}` | Environment | Tabs: units, hosts, common values, secrets, runs | The tabs of Hune (`tabs`, `tab`) over the list table of Hune |
| `/e/{e}/units/{u}` | Unit | The values with their layers, the hosts, the history, the run buttons | The table of Hune (`table`), the form fields (`field`, `input`, `select`, `check`), the save bar of the editor of Hune (`edit-save`) |
| `/e/{e}/units/{u}/hosts/{h}` | Host of a unit | The values of the host layer and the effective values | The same components as the unit page |
| `/e/{e}/hosts/{h}` | Host | The address, the port and the labels | The form of Hune (`form`, `form-grid`, `field`) |
| `/e/{e}/units/{u}/history` | History | The versions and the difference of two versions | The history list and the difference view of the editor of Hune |
| `/e/{e}/units/{u}/run` | Run a unit | The hosts, the check, the result of the check, then the apply | The list box of Hune (`listbox`) for the hosts, the callouts (`callout--warning`) for the confirmation |
| `/e/{e}/commands` | Commands | The commands that the subject can launch | The list table of Hune |
| `/e/{e}/commands/{c}` | Launch a command | A form from the parameters, the hosts, the confirmation | The form fields of Hune, the callouts for the confirmation |
| `/runs` | Runs | The runs, newest first, with their status | The list table of Hune with the badges (`badge--ok`, `badge--error`, `badge--warn`) |
| `/runs/{r}` | Run | The record, one row for each host, the events, the output, the cancel button | The table of Hune, the log view of the logs panel of Hune, the status dots (`dot--ok`, `dot--error`) |
| Unknown path | Not found | A message and a link to `/` | The full page of Hune (`full-page--error`) |

A `GET` of a path that no page names gives the shell with the status 404, after the token check and the question `access` (3.20). A page ignores its query string: the page keeps its state there (`tab`, `new`, `check`), so a reload gives the shell. With `runner.enabled = false`, the run pages give the JSON `404 not_found` with no question (3.3).

Each page has the header of Hune, with the theme menu (`theme-select`) and the subject of the session. The theme follows the system or the choice of the person, as in Hune.

### 5.22 Rules of the page

- The page builds each element with `createElement`, `textContent` and `setAttribute`. It never uses `innerHTML`, `outerHTML`, `insertAdjacentHTML` or `document.write` with data.
- The output and the events of a run show as plain text. The page shows each control character as a visible symbol (U+2400 to U+241F, and U+2421), so a log cannot hide text or move the cursor.
- The page sends each write with `Content-Type: application/json`. It keeps the etag of each document that it shows and sends it in `If-Match`. A `409 conflict` shows the two versions and asks the person to merge by hand.
- The page checks each value with the rules of 2.12 and each parameter with the rules of 4.33 before a write. The server checks them again. The page never changes a value to make it fit.
- The page shows the action of a refusal, never a detail of the PDP.
- The text of the page is English ASD-STE100.
- A button keeps its size and its place when its state changes. A list never pushes content when it loads. The page has no horizontal scroll at a width of 390 pixels.
- The page reads the limits from `GET /api/session` (5.7). Without a session, it shows the error page and sends no write.

### 5.23 Static files

The shell of each page is one HTML file with a `<base>` of `server.base_path`. The static files are below `/static/<build>/`, where `<build>` is the first 12 hexadecimal characters of a SHA-256 over all static files. The server computes it at start. The hash reads the files in the order of their paths. For each file it reads the path, a zero byte, the bytes and a zero byte. A path with another build id gives `404 not_found`.

| Answer | `Cache-Control` |
|---|---|
| Each answer of `/api/`, the two ProcurAuth routes, and each error | `no-store` |
| The shell of a page | `no-cache` |
| A file below `/static/<build>/` | `private, max-age=31536000, immutable` |

The page has no favicon and no product name before the session answers. The shell holds no data.
