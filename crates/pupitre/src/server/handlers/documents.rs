// SPDX-License-Identifier: MIT OR Apache-2.0
//! The routes of the documents (5.5, 5.8 to 5.11, 5.13): lists, reads, writes, deletes and
//! history.

use bytes::Bytes;
use http::StatusCode;
use http::header::ETAG;
use pupitre_core::doc::{DocKey, DocKind, Document};
use pupitre_core::error::{FieldError, errors_json};
use pupitre_core::ids::EnvId;
use pupitre_core::json::{Json, Members};
use pupitre_core::node::{Node, NodeType, SecretScope};
use pupitre_store::Store;
use pupitre_store::catalog::{CatalogEntry, CheckContext, EntryState, WriteRefusal};
use pupitre_store::types::{Message, Precondition, StoredDoc, VersionKind};

use super::{Candidate, display_page};
use crate::access::{Access, CachePolicy};
use crate::config::LogLevel;
use crate::server::ctx::Ctx;
use crate::server::error::{ApiError, Code};
use crate::server::headers::{PutPrecondition, delete_precondition, etag_value, put_precondition};
use crate::server::query::Cursor;
use crate::server::response::{Response, bytes_response, json_response, set_header};

fn refusal_error(r: WriteRefusal) -> ApiError {
    match r {
        WriteRefusal::Invalid(errors) => ApiError::with_errors(Code::InvalidDocument, &errors),
        WriteRefusal::InUse(errors) => ApiError::with_errors(Code::InUse, &errors),
        WriteRefusal::NotEmpty(errors) => ApiError::with_errors(Code::NotEmpty, &errors),
        WriteRefusal::Limit(key) => ApiError::limit_exceeded(key),
    }
}

fn etag_hex(entry: &CatalogEntry) -> Json {
    Json::str(entry.etag.to_hex())
}

fn with_errors_member(pairs: &mut Vec<(&'static str, Json)>, errors: &[FieldError]) {
    if !errors.is_empty() {
        pairs.push(("errors", errors_json(errors)));
    }
}

/// `GET /api/environments` (5.8): the display filter on each E(e) (3.15).
pub async fn environment_list<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let limit = ctx.page_limit()?;
    // Check the cursor before the questions: a cursor that the route did not make gives 400.
    if let Some(text) = ctx.query.get("after") {
        Cursor::decode("environments", text)?;
    }
    let candidates: Vec<Candidate> = ctx
        .state
        .catalog()
        .environments()
        .into_iter()
        .map(|e| Candidate { id: e.as_str().to_string(), node: Node::Environment(e.clone()) })
        .collect();
    let (visible, next) = display_page(ctx, "environments", &candidates, NodeType::Environment, limit).await?;
    let mut items = Vec::with_capacity(visible.len());
    {
        let cat = ctx.state.catalog();
        for i in visible {
            let Ok(env) = EnvId::parse(&candidates[i].id) else { continue };
            let key = DocKey::Environment(env);
            let Some(entry) = cat.get(&key) else { continue };
            let mut pairs = vec![
                ("id", Json::str(candidates[i].id.clone())),
                ("etag", etag_hex(entry)),
                ("version", Json::Int(entry.version)),
            ];
            if let EntryState::Valid(Document::Environment(d)) = &entry.state {
                pairs.push(("label", Json::str(d.label.clone())));
            }
            with_errors_member(&mut pairs, &cat.entry_errors(&key));
            items.push(Json::object(pairs));
        }
    }
    Ok(list_response(items, next))
}

fn list_response(items: Vec<Json>, next: Option<String>) -> Response {
    let mut pairs = vec![("items", Json::Arr(items))];
    if let Some(next) = next {
        pairs.push(("next", Json::str(next)));
    }
    json_response(StatusCode::OK, &Json::object(pairs))
}

/// `GET /api/environments/{e}/hosts` (5.9): `config.read` on E(e).
pub async fn host_list<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let limit = ctx.page_limit()?;
    let cursor = match ctx.query.get("after") {
        Some(text) => Cursor::decode("hosts", text)?,
        None => Cursor::default(),
    };
    let (action, spec) = ctx.use_rule()?;
    ctx.authorize(action, ctx.node(spec), CachePolicy::MayUseKept).await?;
    let env = ctx.env().ok_or_else(ApiError::not_found)?;
    let cat = ctx.state.catalog();
    let ids = cat.hosts(&env);
    let start = match &cursor.last {
        Some(id) => ids.partition_point(|h| h.as_str() <= id.as_str()),
        None => 0,
    };
    let page: Vec<_> = ids.iter().skip(start).take(limit).collect();
    let more = start + page.len() < ids.len();
    let mut items = Vec::with_capacity(page.len());
    for host in &page {
        let key = DocKey::Host(env.clone(), (**host).clone());
        let Some(entry) = cat.get(&key) else { continue };
        let mut pairs =
            vec![("id", Json::str(host.as_str())), ("etag", etag_hex(entry)), ("version", Json::Int(entry.version))];
        if let EntryState::Valid(doc) = &entry.state {
            pairs.push(("document", doc.to_json()));
        }
        with_errors_member(&mut pairs, &cat.entry_errors(&key));
        items.push(Json::object(pairs));
    }
    let next = more.then(|| Cursor { last: page.last().map(|h| h.as_str().to_string()), skip: 0 }.encode("hosts"));
    Ok(list_response(items, next))
}

/// `GET /api/environments/{e}/units` (5.10): the display filter on each U(e,u) (3.15).
pub async fn unit_list<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let limit = ctx.page_limit()?;
    if let Some(text) = ctx.query.get("after") {
        Cursor::decode("units", text)?;
    }
    // An environment that does not exist, or that has a bad id, gives an empty list: the list
    // of a subject that sees nothing looks the same (3.14).
    let candidates: Vec<Candidate> = match ctx.env() {
        Some(env) => ctx
            .state
            .catalog()
            .units(&env)
            .into_iter()
            .map(|u| Candidate { id: u.as_str().to_string(), node: Node::Unit(env.clone(), u.clone()) })
            .collect(),
        None => Vec::new(),
    };
    let (visible, next) = display_page(ctx, "units", &candidates, NodeType::Unit, limit).await?;
    let mut items = Vec::with_capacity(visible.len());
    if let Some(env) = ctx.env() {
        let cat = ctx.state.catalog();
        for i in visible {
            let Node::Unit(_, unit) = &candidates[i].node else { continue };
            let key = DocKey::Unit(env.clone(), unit.clone());
            let Some(entry) = cat.get(&key) else { continue };
            let mut pairs = vec![
                ("id", Json::str(unit.as_str())),
                ("etag", etag_hex(entry)),
                ("version", Json::Int(entry.version)),
            ];
            if let EntryState::Valid(Document::Unit(u)) = &entry.state {
                pairs.push(("label", Json::str(u.label.clone())));
                pairs.push(("schema", Json::str(u.schema.as_str())));
                pairs.push(("hosts", Json::Arr(u.hosts.iter().map(|h| Json::str(h.as_str())).collect())));
            }
            with_errors_member(&mut pairs, &cat.entry_errors(&key));
            items.push(Json::object(pairs));
        }
    }
    Ok(list_response(items, next))
}

fn document_response(status: StatusCode, bytes: Vec<u8>, etag: &pupitre_core::etag::Etag) -> Response {
    let mut r = bytes_response(status, "application/json; charset=utf-8", Bytes::from(bytes));
    r.headers_mut().insert(http::header::CACHE_CONTROL, http::HeaderValue::from_static("no-store"));
    r.headers_mut().insert(ETAG, etag_value(etag));
    r
}

/// `GET` of a document (5.5): the canonical bytes with `ETag`.
pub async fn get<A: Access, S: Store>(ctx: &Ctx<A, S>, kind: DocKind) -> Result<Response, ApiError> {
    let (action, spec) = ctx.use_rule()?;
    let key = ctx.doc_key(kind);
    let node = key.as_ref().and_then(|_| ctx.node(spec));
    ctx.authorize(action, node, CachePolicy::MayUseKept).await?;
    let key = key.ok_or_else(ApiError::not_found)?;
    let entry = ctx.state.catalog().get(&key).cloned().ok_or_else(ApiError::not_found)?;
    match &entry.state {
        // A document that fails 2.5 or 2.7 to 2.10 has no body to show. The list of its kind
        // shows its etag and its errors, so a person can save a new document with `If-Match`.
        EntryState::Invalid(errors) => Err(ApiError::with_errors(Code::InvalidDocument, errors)),
        EntryState::Valid(doc) => Ok(document_response(StatusCode::OK, doc.canonical().bytes, &entry.etag)),
    }
}

/// The members of the body of a write (5.5): `document` and the optional `message`.
fn write_body(body: &Json, id: bool) -> Result<(Option<Json>, Json, Message), ApiError> {
    let mut m = Members::of(body).map_err(|_| ApiError::bad_request())?;
    let document = m.take("document").ok_or_else(ApiError::bad_request)?;
    if !matches!(document, Json::Obj(_)) {
        return Err(ApiError::bad_request());
    }
    let id_member = if id {
        match m.take("id") {
            Some(v @ Json::Str(_)) => Some(v),
            _ => return Err(ApiError::bad_request()),
        }
    } else {
        None
    };
    let message = match m.take("message") {
        None => Message::default(),
        Some(Json::Str(t)) => Message::parse(&t).ok_or_else(ApiError::bad_request)?,
        Some(_) => return Err(ApiError::bad_request()),
    };
    if !m.finish().is_empty() {
        return Err(ApiError::bad_request());
    }
    Ok((id_member, document, message))
}

/// The key paths whose value differs between two documents of values (7.9).
fn changed_paths(old: Option<&Document>, new: &Document) -> Vec<String> {
    let (Some(new_values), old_values) = (new.values(), old.and_then(Document::values)) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for (path, value) in &new_values.values {
        if old_values.and_then(|v| v.values.get(path)) != Some(value) {
            out.push(path.as_str().to_string());
        }
    }
    if let Some(old_values) = old_values {
        out.extend(
            old_values.values.keys().filter(|p| !new_values.values.contains_key(*p)).map(|p| p.as_str().to_string()),
        );
    }
    out.sort();
    out
}

/// Steps 10 and 11 of 3.13 for the write of a document: the validation, the rules across
/// documents, the write and the catalog.
async fn apply_put<A: Access, S: Store>(
    ctx: &Ctx<A, S>,
    key: DocKey,
    document: &Json,
    precondition: Precondition,
    message: Message,
) -> Result<(StatusCode, Saved), ApiError> {
    let doc = Document::from_json(key.kind(), document)
        .map_err(|errors| ApiError::with_errors(Code::InvalidDocument, &errors))?;
    let canonical = doc.canonical();
    // One write at a time in this instance: the check and the write are one step.
    let _write = ctx.state.write_lock.lock().await;
    let before = {
        let cat = ctx.state.catalog();
        let check =
            CheckContext { capabilities: &ctx.state.capabilities, runner_enabled: ctx.state.settings.runner_enabled() };
        cat.check_put(&check, &key, &doc, canonical.bytes.len()).map_err(refusal_error)?;
        cat.get(&key).cloned()
    };
    let created = matches!(precondition, Precondition::Create);
    let meta = ctx.write_meta(message);
    let span = ctx.trace.child("store.put");
    let saved = ctx.state.store.put(&key, &canonical, &precondition, &meta).await;
    span.end(saved.is_err());
    let saved = saved.map_err(|e| ctx.store_error("put", &e))?;
    ctx.state.catalog_mut().put_own(StoredDoc {
        key: key.clone(),
        bytes: canonical.bytes.clone(),
        etag: canonical.etag,
        version: saved.version,
    });
    let old_doc = before.as_ref().and_then(|e| match &e.state {
        EntryState::Valid(d) => Some(d),
        EntryState::Invalid(_) => None,
    });
    let paths = changed_paths(old_doc, &doc);
    let mut line = ctx
        .state
        .logger
        .event(LogLevel::Info, "document.write")
        .request_id(&ctx.request_id)
        .text("key", &key.text())
        .text("kind", key.kind().name())
        .int("version", saved.version)
        .text("etag_after", &canonical.etag.to_hex())
        .field("paths", Json::Arr(paths.into_iter().map(Json::Str).collect()))
        .text("subject", ctx.subject.as_str());
    if let Some(entry) = &before {
        line = line.text("etag_before", &entry.etag.to_hex());
    }
    line.emit();
    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    Ok((status, Saved { etag: canonical.etag, version: saved.version }))
}

struct Saved {
    etag: pupitre_core::etag::Etag,
    version: u64,
}

fn saved_response(status: StatusCode, saved: &Saved) -> Response {
    let mut r = json_response(
        status,
        &Json::object([("etag", Json::str(saved.etag.to_hex())), ("version", Json::Int(saved.version))]),
    );
    r.headers_mut().insert(ETAG, etag_value(&saved.etag));
    r
}

/// `POST /api/environments` (5.8): `inventory.write` on the service.
pub async fn environment_create<A: Access, S: Store>(ctx: &Ctx<A, S>) -> Result<Response, ApiError> {
    let body = ctx.body_json()?;
    let (id, document, message) = write_body(&body, true)?;
    let (action, spec) = ctx.use_rule()?;
    ctx.authorize(action, ctx.node(spec), CachePolicy::Fresh).await?;
    let id_text = id.as_ref().and_then(Json::as_str).unwrap_or_default().to_string();
    let env = EnvId::parse(&id_text)
        .map_err(|e| ApiError::with_errors(Code::InvalidDocument, &[FieldError::new("id", e.to_string())]))?;
    let (status, saved) = apply_put(ctx, DocKey::Environment(env), &document, Precondition::Create, message).await?;
    Ok(saved_response(status, &saved))
}

/// `PUT` of a document (5.5).
pub async fn put<A: Access, S: Store>(ctx: &Ctx<A, S>, kind: DocKind) -> Result<Response, ApiError> {
    let precondition = match put_precondition(&ctx.headers)? {
        PutPrecondition::Create => Precondition::Create,
        PutPrecondition::Match(etag) => Precondition::Match(etag),
    };
    let body = ctx.body_json()?;
    let (_, document, message) = write_body(&body, false)?;
    let (action, spec) = ctx.use_rule()?;
    let key = ctx.doc_key(kind);
    let node = key.as_ref().and_then(|_| ctx.node(spec));
    ctx.authorize(action, node, CachePolicy::Fresh).await?;
    let key = key.ok_or_else(ApiError::not_found)?;
    let (status, saved) = apply_put(ctx, key, &document, precondition, message).await?;
    Ok(saved_response(status, &saved))
}

/// `DELETE` of a document (5.5).
pub async fn delete<A: Access, S: Store>(ctx: &Ctx<A, S>, kind: DocKind) -> Result<Response, ApiError> {
    let if_match = delete_precondition(&ctx.headers)?;
    let (action, spec) = ctx.use_rule()?;
    let key = ctx.doc_key(kind);
    let node = key.as_ref().and_then(|_| ctx.node(spec));
    ctx.authorize(action, node, CachePolicy::Fresh).await?;
    let key = key.ok_or_else(ApiError::not_found)?;
    let _write = ctx.state.write_lock.lock().await;
    let before = {
        let cat = ctx.state.catalog();
        let Some(entry) = cat.get(&key).cloned() else { return Err(ApiError::not_found()) };
        cat.check_delete(&key).map_err(refusal_error)?;
        entry
    };
    // A secret does not move with a document: a delete that leaves secrets would give them to
    // a new item with the same id (2.24). The delete is refused instead.
    let scope = match &key {
        DocKey::Environment(e) => Some(SecretScope::Environment(e.clone())),
        DocKey::Unit(e, u) => Some(SecretScope::Unit(e.clone(), u.clone())),
        _ => None,
    };
    if let Some(scope) = scope {
        let held = ctx.state.store.list_secrets(&scope).await.map_err(|e| ctx.store_error("list_secrets", &e))?;
        if !held.is_empty() {
            return Err(ApiError::with_errors(
                Code::NotEmpty,
                &[FieldError::new("", "the item still has secrets; delete them first")],
            ));
        }
    }
    let meta = ctx.write_meta(Message::default());
    let span = ctx.trace.child("store.delete");
    let saved = ctx.state.store.delete(&key, &if_match, &meta).await;
    span.end(saved.is_err());
    let saved = saved.map_err(|e| ctx.store_error("delete", &e))?;
    ctx.state.catalog_mut().remove_own(&key);
    ctx.state
        .logger
        .event(LogLevel::Info, "document.write")
        .request_id(&ctx.request_id)
        .text("key", &key.text())
        .text("kind", key.kind().name())
        .int("version", saved.version)
        .text("etag_before", &before.etag.to_hex())
        .field("paths", Json::Arr(Vec::new()))
        .text("subject", ctx.subject.as_str())
        .emit();
    Ok(json_response(StatusCode::OK, &Json::object([("version", Json::Int(saved.version))])))
}

fn kind_text(kind: VersionKind) -> &'static str {
    kind.name()
}

/// `GET <document path>/history` (5.13).
pub async fn history<A: Access, S: Store>(ctx: &Ctx<A, S>, kind: DocKind) -> Result<Response, ApiError> {
    let limit = ctx.page_limit()?;
    let before = ctx.query.integer("before", 1, pupitre_core::json::MAX_SAFE_INTEGER)?;
    let (action, spec) = ctx.use_rule()?;
    let key = ctx.doc_key(kind);
    let node = key.as_ref().and_then(|_| ctx.node(spec));
    ctx.authorize(action, node, CachePolicy::MayUseKept).await?;
    let key = key.ok_or_else(ApiError::not_found)?;
    let span = ctx.trace.child("store.history");
    let asked = u32::try_from(limit.saturating_add(1)).unwrap_or(u32::MAX);
    let versions = ctx.state.store.history(&key, before, asked).await;
    span.end(versions.is_err());
    let mut versions = versions.map_err(|e| ctx.store_error("history", &e))?;
    if versions.is_empty() && before.is_none() && ctx.state.catalog().get(&key).is_none() {
        // The key never had a document.
        return Err(ApiError::not_found());
    }
    let more = versions.len() > limit;
    versions.truncate(limit);
    let next = more.then(|| versions.last().map(|v| v.version)).flatten();
    let items: Vec<Json> = versions
        .iter()
        .map(|v| {
            let mut pairs = vec![
                ("version", Json::Int(v.version)),
                ("kind", Json::str(kind_text(v.kind))),
                ("saved_by", Json::str(v.meta.saved_by.as_str())),
                ("saved_at", Json::str(v.meta.saved_at.to_string())),
                ("message", Json::str(v.meta.message.as_str())),
            ];
            if let Some(etag) = &v.etag {
                pairs.push(("etag", Json::str(etag.to_hex())));
            }
            Json::object(pairs)
        })
        .collect();
    let mut pairs = vec![("items", Json::Arr(items))];
    if let Some(n) = next {
        pairs.push(("next", Json::Int(n)));
    }
    Ok(json_response(StatusCode::OK, &Json::object(pairs)))
}

/// `GET <document path>/history/{n}` (5.13): the bytes of one version.
pub async fn version<A: Access, S: Store>(ctx: &Ctx<A, S>, kind: DocKind) -> Result<Response, ApiError> {
    let number = ctx
        .params
        .get("n")
        .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) && (t.len() == 1 || !t.starts_with('0')))
        .and_then(|t| t.parse::<u64>().ok())
        .filter(|n| (1..=pupitre_core::json::MAX_SAFE_INTEGER).contains(n))
        .ok_or_else(ApiError::bad_request)?;
    let (action, spec) = ctx.use_rule()?;
    let key = ctx.doc_key(kind);
    let node = key.as_ref().and_then(|_| ctx.node(spec));
    ctx.authorize(action, node, CachePolicy::MayUseKept).await?;
    let key = key.ok_or_else(ApiError::not_found)?;
    let span = ctx.trace.child("store.version");
    let found = ctx.state.store.version(&key, number).await;
    span.end(found.is_err());
    let found = found.map_err(|e| ctx.store_error("version", &e))?;
    match (found.bytes, found.meta.etag) {
        (Some(bytes), Some(etag)) => {
            let mut r = document_response(StatusCode::OK, bytes, &etag);
            set_header(&mut r, http::header::CONTENT_TYPE, "application/json; charset=utf-8");
            Ok(r)
        }
        // A version `delete` has no bytes.
        _ => Err(ApiError::not_found()),
    }
}
