//! Axum middleware recording HTTP request count + latency through the
//! `metrics` facade (exported by `minerva-metrics`). Lives in the api
//! crate because it's the only binary with an axum router; the worker /
//! scheduler / model-server pods export process + domain metrics but run
//! no HTTP server.
//!
//! Two series, both labelled `method` / `path` / `status`:
//!   - `http_requests_total` (counter)
//!   - `http_request_duration_seconds` (histogram; `_seconds` suffix picks
//!     up the latency buckets configured in `minerva_metrics::init`)
//!
//! `path` is the *matched route template* (`/api/courses/:id`), never the
//! raw URL (`/api/courses/abc-123`), so label cardinality stays bounded;
//! unbounded label values are the classic way to OOM a Prometheus, which
//! on this single node we very much want to avoid.

use std::time::Instant;

use axum::{extract::MatchedPath, extract::Request, middleware::Next, response::Response};

pub async fn track_metrics(req: Request, next: Next) -> Response {
    let start = Instant::now();

    // MatchedPath is populated by the router during routing and is visible
    // to a layer wrapping the nested routers. Requests that fall through to
    // the static-file / dev-proxy fallback have no matched route; bucket
    // them under a single fixed label rather than leaking raw paths.
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "<unmatched>".to_owned());
    let method = req.method().as_str().to_owned();

    let response = next.run(req).await;

    let latency = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    metrics::counter!(
        "http_requests_total",
        "method" => method.clone(),
        "path" => path.clone(),
        "status" => status.clone(),
    )
    .increment(1);
    metrics::histogram!(
        "http_request_duration_seconds",
        "method" => method,
        "path" => path,
        "status" => status,
    )
    .record(latency);

    response
}
