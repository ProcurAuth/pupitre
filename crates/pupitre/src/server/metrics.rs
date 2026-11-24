// SPDX-License-Identifier: MIT OR Apache-2.0
//! The metrics of 6.28 in the Prometheus text format 0.0.4. A label never holds a subject, an
//! id of a node or a value: the route label is the route pattern.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The upper bounds of the histogram `pupitre_http_request_seconds`.
pub const BUCKETS: [f64; 11] = [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 10.0];

#[derive(Debug, Default, Clone)]
struct Histogram {
    buckets: [u64; BUCKETS.len()],
    count: u64,
    sum: f64,
}

#[derive(Debug, Default)]
struct Tables {
    http: BTreeMap<(String, u16), u64>,
    seconds: BTreeMap<String, Histogram>,
    pdp: BTreeMap<&'static str, u64>,
    store_errors: BTreeMap<&'static str, u64>,
    runs: BTreeMap<(&'static str, &'static str, &'static str), u64>,
}

/// The counters of the server. The gauges that the store gives (`pupitre_runs_queued` and the
/// others) are read at each scrape and passed to `render`.
#[derive(Debug, Default)]
pub struct Metrics {
    tables: Mutex<Tables>,
    pdp_cache_hits: AtomicU64,
}

/// The values that a scrape reads from the catalog and the store.
#[derive(Debug, Clone, Default)]
pub struct Gauges {
    /// The count of the entries of each kind (`pupitre_catalog_documents`).
    pub catalog_documents: BTreeMap<&'static str, usize>,
    /// The queued runs, when the store answered.
    pub runs_queued: Option<u64>,
    /// The runs with the status `running`, when the store answered.
    pub runs_running: Option<u64>,
    /// The age in seconds of the oldest queued run, when the store answered. Zero without one.
    pub oldest_queued_seconds: Option<u64>,
    /// The spans that the queue dropped (6.29).
    pub spans_dropped: u64,
}

impl Metrics {
    fn tables(&self) -> std::sync::MutexGuard<'_, Tables> {
        self.tables.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Count one request of the main listener.
    pub fn http_request(&self, route: &str, status: u16, elapsed: Duration) {
        let mut t = self.tables();
        *t.http.entry((route.to_string(), status)).or_default() += 1;
        let h = t.seconds.entry(route.to_string()).or_default();
        let secs = elapsed.as_secs_f64();
        for (i, bound) in BUCKETS.iter().enumerate() {
            if secs <= *bound {
                h.buckets[i] += 1;
            }
        }
        h.count += 1;
        h.sum += secs;
    }

    /// Count one question to the PDP with its result: `allow`, `deny` or `error`.
    pub fn pdp_request(&self, result: &'static str) {
        *self.tables().pdp.entry(result).or_default() += 1;
    }

    /// Count one answer of the PDP that came from the cache.
    pub fn pdp_cache_hit(&self) {
        self.pdp_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Count one failed call of the store, by method.
    pub fn store_error(&self, method: &'static str) {
        *self.tables().store_errors.entry(method).or_default() += 1;
    }

    /// Count one run that this instance wrote or changed.
    pub fn run(&self, kind: &'static str, mode: &'static str, status: &'static str) {
        *self.tables().runs.entry((kind, mode, status)).or_default() += 1;
    }

    /// The text of the metrics, with the gauges of this scrape.
    pub fn render(&self, gauges: &Gauges) -> String {
        let t = self.tables();
        let mut out = String::new();
        let family = |out: &mut String, name: &str, kind: &str, help: &str| {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} {kind}");
        };
        family(&mut out, "pupitre_http_requests_total", "counter", "Requests of the main listener.");
        for ((route, status), n) in &t.http {
            let _ = writeln!(out, "pupitre_http_requests_total{{route=\"{}\",status=\"{status}\"}} {n}", escape(route));
        }
        family(&mut out, "pupitre_http_request_seconds", "histogram", "Time of the requests of the main listener.");
        for (route, h) in &t.seconds {
            let route = escape(route);
            for (i, bound) in BUCKETS.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "pupitre_http_request_seconds_bucket{{route=\"{route}\",le=\"{bound}\"}} {}",
                    h.buckets[i]
                );
            }
            let _ = writeln!(out, "pupitre_http_request_seconds_bucket{{route=\"{route}\",le=\"+Inf\"}} {}", h.count);
            let _ = writeln!(out, "pupitre_http_request_seconds_sum{{route=\"{route}\"}} {}", h.sum);
            let _ = writeln!(out, "pupitre_http_request_seconds_count{{route=\"{route}\"}} {}", h.count);
        }
        family(&mut out, "pupitre_pdp_requests_total", "counter", "Questions to the PDP.");
        for result in ["allow", "deny", "error"] {
            let _ = writeln!(
                out,
                "pupitre_pdp_requests_total{{result=\"{result}\"}} {}",
                t.pdp.get(result).copied().unwrap_or(0)
            );
        }
        family(&mut out, "pupitre_pdp_cache_hits_total", "counter", "Answers of the PDP that the cache gave.");
        let _ = writeln!(out, "pupitre_pdp_cache_hits_total {}", self.pdp_cache_hits.load(Ordering::Relaxed));
        family(&mut out, "pupitre_store_errors_total", "counter", "Failed calls of the store.");
        for (method, n) in &t.store_errors {
            let _ = writeln!(out, "pupitre_store_errors_total{{method=\"{method}\"}} {n}");
        }
        family(&mut out, "pupitre_catalog_documents", "gauge", "Entries of the catalog.");
        for (kind, n) in &gauges.catalog_documents {
            let _ = writeln!(out, "pupitre_catalog_documents{{kind=\"{kind}\"}} {n}");
        }
        family(&mut out, "pupitre_runs_total", "counter", "Runs that this instance wrote or changed.");
        for ((kind, mode, status), n) in &t.runs {
            let _ = writeln!(out, "pupitre_runs_total{{kind=\"{kind}\",mode=\"{mode}\",status=\"{status}\"}} {n}");
        }
        for (name, help, value) in [
            ("pupitre_runs_queued", "Runs with the status queued.", gauges.runs_queued),
            ("pupitre_runs_running", "Runs with the status running.", gauges.runs_running),
            ("pupitre_runs_oldest_queued_seconds", "Age of the oldest queued run.", gauges.oldest_queued_seconds),
        ] {
            family(&mut out, name, "gauge", help);
            if let Some(v) = value {
                let _ = writeln!(out, "{name} {v}");
            }
        }
        family(&mut out, "pupitre_telemetry_spans_dropped_total", "counter", "Spans that the queue dropped.");
        let _ = writeln!(out, "pupitre_telemetry_spans_dropped_total {}", gauges.spans_dropped);
        out
    }
}

/// Escape a label value for the text format.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_has_each_metric_of_6_28() {
        let m = Metrics::default();
        m.http_request("GET /api/session", 200, Duration::from_millis(3));
        m.http_request("GET /api/session", 200, Duration::from_millis(30));
        m.pdp_request("allow");
        m.store_error("put");
        m.run("deploy", "check", "queued");
        let mut g = Gauges::default();
        g.catalog_documents.insert("host", 4);
        g.runs_queued = Some(1);
        g.spans_dropped = 2;
        let text = m.render(&g);
        for name in [
            "pupitre_http_requests_total",
            "pupitre_http_request_seconds",
            "pupitre_pdp_requests_total",
            "pupitre_pdp_cache_hits_total",
            "pupitre_store_errors_total",
            "pupitre_catalog_documents",
            "pupitre_runs_total",
            "pupitre_runs_queued",
            "pupitre_runs_running",
            "pupitre_runs_oldest_queued_seconds",
            "pupitre_telemetry_spans_dropped_total",
        ] {
            assert!(text.contains(&format!("# TYPE {name} ")), "{name}");
        }
        assert!(text.contains("pupitre_http_requests_total{route=\"GET /api/session\",status=\"200\"} 2"));
        assert!(text.contains("pupitre_http_request_seconds_bucket{route=\"GET /api/session\",le=\"0.005\"} 1"));
        assert!(text.contains("pupitre_http_request_seconds_bucket{route=\"GET /api/session\",le=\"+Inf\"} 2"));
        assert!(text.contains("pupitre_http_request_seconds_count{route=\"GET /api/session\"} 2"));
        assert!(text.contains("pupitre_pdp_requests_total{result=\"allow\"} 1"));
        assert!(text.contains("pupitre_pdp_requests_total{result=\"deny\"} 0"));
        assert!(text.contains("pupitre_catalog_documents{kind=\"host\"} 4"));
        assert!(text.contains("pupitre_runs_queued 1\n"));
        // A gauge that the store did not give has no sample.
        assert!(!text.contains("\npupitre_runs_running "));
        assert!(text.contains("pupitre_telemetry_spans_dropped_total 2"));
    }

    #[test]
    fn a_label_value_is_escaped() {
        assert_eq!(escape("a\"b\\c\n"), "a\\\"b\\\\c\\n");
    }
}
