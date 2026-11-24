// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ops listener (5.20): `/livez`, `/readyz` and `/metrics`. The routes have no base path,
//! no token and no question. The edge never reaches this listener (7.2).

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http::header::{CACHE_CONTROL, HeaderValue};
use http::{Method, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use pupitre_core::run::RunStatus;
use pupitre_core::time::UtcMillis;
use pupitre_store::Store;
use pupitre_store::types::RunFilter;
use tokio::net::TcpListener;
use tokio::sync::watch;

use super::conn::{ConnShared, ConnTimes, run_connection};
use super::error::ApiError;
use super::limits::{OPS_HEADER_TIMEOUT, OPS_MAX_CONNECTIONS, SCRAPE_MAX_RUNS, SCRAPE_STORE_TIMEOUT};
use super::metrics::Gauges;
use super::response::{Response, bytes_response, error_response};
use super::state::AppState;
use crate::access::Access;

/// The check of `/readyz` that fails first, or `None` when each check holds (5.20).
/// Check 1 is `store`: the store is open. Check 2 is `catalog`: the last load is within the
/// limits. Check 3 is `runner_lease`: the runner node holds the lease `runner`. A PDP outage
/// never changes the answer.
pub fn first_failing_check<A: Access, S: Store>(state: &AppState<A, S>) -> Option<&'static str> {
    if !state.store_ok() {
        return Some("store");
    }
    if !state.catalog().is_ready() {
        return Some("catalog");
    }
    if state.settings.runner.is_some_and(|r| r.execute) && !state.runner_lease() {
        return Some("runner_lease");
    }
    None
}

fn text(status: StatusCode, body: String) -> Response {
    let mut r = bytes_response(status, "text/plain; charset=utf-8", Bytes::from(body));
    r.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// The gauges that the store gives (6.28): the queued runs, the `running` runs and the age of
/// the oldest queued run. A store that does not answer in time gives no value.
async fn run_gauges<A: Access, S: Store>(state: &AppState<A, S>, gauges: &mut Gauges) {
    let Some(runner) = state.settings.runner else {
        gauges.runs_queued = Some(0);
        gauges.runs_running = Some(0);
        gauges.oldest_queued_seconds = Some(0);
        return;
    };
    let queued_filter = RunFilter { environment: None, status: Some(RunStatus::Queued) };
    let queued = tokio::time::timeout(
        SCRAPE_STORE_TIMEOUT,
        state.store.list_runs(&queued_filter, None, runner.max_queued_runs.saturating_add(1)),
    )
    .await;
    match queued {
        Ok(Ok(list)) => {
            gauges.runs_queued = Some(list.len() as u64);
            // The list is newest first: the last run is the oldest.
            gauges.oldest_queued_seconds = Some(list.last().map_or(0, |r| UtcMillis::now().since(r.created_at) / 1000));
        }
        _ => state.metrics.store_error("list_runs"),
    }
    let running_filter = RunFilter { environment: None, status: Some(RunStatus::Running) };
    match tokio::time::timeout(SCRAPE_STORE_TIMEOUT, state.store.list_runs(&running_filter, None, SCRAPE_MAX_RUNS))
        .await
    {
        Ok(Ok(list)) => gauges.runs_running = Some(list.len() as u64),
        _ => state.metrics.store_error("list_runs"),
    }
}

async fn answer<A: Access, S: Store>(state: &AppState<A, S>, method: &Method, path: &str) -> Response {
    let known = matches!(path, "/livez" | "/readyz" | "/metrics");
    if !known {
        return error_response(&ApiError::not_found());
    }
    if *method != Method::GET && *method != Method::HEAD {
        return error_response(&ApiError::method_not_allowed("GET, HEAD".to_string()));
    }
    match path {
        "/livez" => text(StatusCode::OK, "ok".to_string()),
        "/readyz" => match first_failing_check(state) {
            None => text(StatusCode::OK, "ok".to_string()),
            Some(check) => text(StatusCode::SERVICE_UNAVAILABLE, check.to_string()),
        },
        _ => {
            let mut gauges = Gauges {
                catalog_documents: state.catalog().document_counts().into_iter().collect::<BTreeMap<_, _>>(),
                spans_dropped: state.telemetry.dropped(),
                ..Gauges::default()
            };
            run_gauges(state, &mut gauges).await;
            let mut r = bytes_response(
                StatusCode::OK,
                "text/plain; version=0.0.4; charset=utf-8",
                Bytes::from(state.metrics.render(&gauges)),
            );
            r.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }
    }
}

/// Serve one request of the ops listener.
pub async fn serve_ops_request<A: Access, S: Store>(
    state: Arc<AppState<A, S>>,
    conn: Arc<ConnShared>,
    req: hyper::Request<Incoming>,
) -> Result<Response, Infallible> {
    conn.request_started();
    let (parts, _body) = req.into_parts();
    let mut response = answer(&state, &parts.method, parts.uri.path()).await;
    if parts.method == Method::HEAD {
        use hyper::body::Body as _;
        let len = response.body().size_hint().exact().unwrap_or(0);
        response.headers_mut().insert(http::header::CONTENT_LENGTH, HeaderValue::from(len));
        *response.body_mut() = Full::new(Bytes::new());
    }
    conn.request_finished();
    Ok(response)
}

/// The accept loop of `server.ops_listen`.
pub async fn ops_accept_loop<A: Access + 'static, S: Store + 'static>(
    listener: TcpListener,
    state: Arc<AppState<A, S>>,
    mut stop: watch::Receiver<bool>,
    close_connections: watch::Receiver<bool>,
) {
    let open = Arc::new(AtomicUsize::new(0));
    let times = ConnTimes { idle: Duration::from_secs(60), header: OPS_HEADER_TIMEOUT };
    loop {
        let accepted = tokio::select! {
            r = listener.accept() => r,
            _ = stop.changed() => {
                if *stop.borrow() { return; }
                continue;
            }
        };
        let Ok((stream, _peer)) = accepted else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        if open.fetch_add(1, Ordering::SeqCst) >= OPS_MAX_CONNECTIONS {
            open.fetch_sub(1, Ordering::SeqCst);
            drop(stream);
            continue;
        }
        let shared = ConnShared::new();
        let st = state.clone();
        let sh = shared.clone();
        let close_rx = close_connections.clone();
        let counter = open.clone();
        tokio::spawn(async move {
            let handler = move |req: hyper::Request<Incoming>| serve_ops_request(st.clone(), sh.clone(), req);
            run_connection(stream, shared, handler, times, close_rx).await;
            counter.fetch_sub(1, Ordering::SeqCst);
        });
    }
}
