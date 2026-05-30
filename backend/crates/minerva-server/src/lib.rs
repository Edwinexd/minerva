//! Library face of `minerva-server`. Phase 3 of the microservices
//! split (see `docs/microservices-split.md`) turns the api crate into
//! both a library and a binary so the new `minerva-worker` and
//! `minerva-scheduler` binaries can share its module tree without
//! duplicating routes, AppState, classification, the relink
//! scheduler, etc.
//!
//! `src/main.rs` is now a thin wrapper that calls [`api_main`];
//! `src/bin/minerva-worker.rs` calls [`worker_main`]. Both run the
//! same `AppState::new` and (since Phase 3.5 hasn't moved the
//! schedulers yet) the same `worker::start` machinery; the only
//! difference is whether the HTTP listener is bound.
//!
//! ## Why two `pub async fn` instead of branching on env in `main`
//!
//! Putting `if env == "api" else if env == "worker"` in a single main
//! ties pod role to runtime config, which is fragile. Two binaries
//! match the cleaner "image determines role" pattern; the env var
//! controls only the cutover toggle (`MINERVA_RUN_WORKER` on the api
//! binary).

pub mod auth;
pub mod classification;
pub mod config;
pub mod dev_seed;
pub mod error;
pub mod ext_obfuscate;
pub mod feature_flags;
pub mod github_url;
pub mod lti;
pub mod lti_nrps;
pub mod model_capabilities;
pub mod relink_scheduler;
pub mod routes;
pub mod rules;
pub mod state;
pub mod strategy;
pub mod system_defaults;
pub mod worker;

use axum::response::{IntoResponse, Response};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

/// Initialise tracing once. Shared by every binary in this crate so a
/// `minerva-worker` log line looks identical to a `minerva` log line.
fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("minerva=debug,tower_http=debug")),
        )
        .init();
}

/// Spawn the periodic `/proc/self/status` memprobe. Each binary calls
/// this from its own main; the trace line is labelled by the binary's
/// own log target so a grep for `memprobe` lands on the right pod.
fn spawn_memprobe() {
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        loop {
            if let Some(stats) = read_proc_self_status() {
                tracing::info!(
                    "memprobe: uptime={}s vm_rss={} MiB vm_hwm={} MiB vm_size={} MiB vm_data={} MiB threads={}",
                    started.elapsed().as_secs(),
                    stats.vm_rss_kb / 1024,
                    stats.vm_hwm_kb / 1024,
                    stats.vm_size_kb / 1024,
                    stats.vm_data_kb / 1024,
                    stats.threads,
                );
            }
            let interval = if started.elapsed() < std::time::Duration::from_secs(5 * 60) {
                std::time::Duration::from_secs(5)
            } else {
                std::time::Duration::from_secs(60)
            };
            tokio::time::sleep(interval).await;
        }
    });
}

/// API binary entrypoint. Boots HTTP routes, the worker (gated on
/// `MINERVA_RUN_WORKER`), the LTI provider, and the background
/// backfills + benchmarks. Same body as the pre-Phase-3 `main.rs`,
/// moved here so the worker binary can share the module tree.
pub async fn api_main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let config = config::Config::from_env()?;
    let state = state::AppState::new(&config).await?;

    // Memory probe: see the function's doc for cadence / rationale.
    spawn_memprobe();

    // One-shot backfill of the document_id payload index across pre-existing
    // course_* collections. New collections get the index at creation time.
    // Runs in the background so a slow/unhealthy Qdrant doesn't block startup.
    {
        let qdrant = state.qdrant.clone();
        tokio::spawn(async move {
            backfill_document_id_indexes(&qdrant).await;
        });
    }

    // Start the background document-processing worker, unless this
    // pod is the api in a Phase 3+ topology where the worker pod
    // already runs the same loop. The queue uses `FOR UPDATE SKIP
    // LOCKED` so dual-running during the cutover window is safe; the
    // env var is the eventual off switch.
    if config.run_worker {
        worker::start(state.clone(), config.max_concurrent_ingests);
    } else {
        tracing::info!(
            "worker: disabled in this pod (MINERVA_RUN_WORKER=false); claim loop runs in the minerva-worker pod"
        );
    }

    // GDPR retention sweep for rule-attribute observations: drop rows whose
    // `last_seen` is older than the TTL. Active users keep refreshing
    // last_seen on every login, so this only purges values for users who
    // haven't shown up in a week. Six-hour cadence is plenty; the row
    // count is small (one per (attribute, value, user_id) triple) and a
    // few hours of overshoot past the TTL is fine for a privacy-floor.
    {
        let db = state.db.clone();
        tokio::spawn(async move {
            const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);
            // First sweep runs immediately so a restart after a long
            // downtime doesn't leave aged-out rows around for another
            // six hours. The TTL is read per-iteration from
            // `system_defaults` so an admin can dial it on
            // /admin/defaults without restarting.
            loop {
                let ttl_days = system_defaults::observation_ttl_days(&db).await;
                match minerva_db::queries::role_rule_attribute_observations::prune_older_than(
                    &db, ttl_days,
                )
                .await
                {
                    Ok(0) => {}
                    Ok(n) => {
                        tracing::info!("pruned {} rule-attribute observation row(s) past TTL", n)
                    }
                    Err(e) => tracing::warn!("rule-attribute observation prune failed: {}", e),
                }
                tokio::time::sleep(PRUNE_INTERVAL).await;
            }
        });
    }

    // One-shot backfill of `documents.content_hash` for pre-existing rows.
    // The column was added in the slice-1 schema migration; new uploads
    // populate it server-side, but legacy rows stay NULL until something
    // reads the file back off disk and hashes it. Until then, the partial
    // unique index `(course_id, content_hash) WHERE content_hash IS NOT NULL`
    // simply skips them, so they don't conflict, but server-side dedup
    // doesn't work for those docs either. We rate-limit to one batch /
    // second so an installation with 10k legacy rows doesn't saturate
    // disk I/O on a single pod restart. Self-terminates when the table
    // is fully backfilled.
    {
        let state = state.clone();
        let docs_path = config.docs_path.clone();
        tokio::spawn(async move {
            const BATCH: i64 = 50;
            // Keyset cursor on (created_at, id). Advances past every row
            // the SELECT returns, including those whose UPDATE fails
            // (dup-collision, missing file, etc.) so the sweep can't get
            // stuck re-selecting the same rows forever and OOM the pod.
            let mut cursor: Option<(chrono::DateTime<chrono::Utc>, uuid::Uuid)> = None;
            loop {
                let rows = match minerva_db::queries::documents::list_active_missing_content_hash(
                    &state.db, cursor, BATCH,
                )
                .await
                {
                    Ok(rows) if rows.is_empty() => {
                        tracing::info!("content_hash backfill complete");
                        return;
                    }
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::warn!("content_hash backfill query failed: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        continue;
                    }
                };

                // Advance the cursor to the last row of this page BEFORE
                // doing any per-row work, so a transient error mid-batch
                // can't cause us to re-process the same prefix.
                if let Some(last) = rows.last() {
                    cursor = Some((last.4, last.0));
                }

                for (doc_id, course_id, filename, _mime_type, _created_at) in rows {
                    let ext = routes::documents::extension_from_filename(&filename);
                    let path = format!("{}/{}/{}.{}", docs_path, course_id, doc_id, ext);
                    // Stream-hash the file in 64 KiB chunks rather than
                    // slurping it into a Vec<u8>. The sweep hits every
                    // legacy doc on disk; with multi-MB PDFs that
                    // churn was leaving glibc badly fragmented while
                    // fastembed was concurrently loading its models,
                    // and the pod's 6 GiB ceiling (sized for the
                    // fastembed cache, not for a parallel allocation
                    // stream) tripped.
                    let hash = match routes::documents::compute_content_hash_streaming(
                        std::path::Path::new(&path),
                    )
                    .await
                    {
                        Ok(h) => h,
                        Err(e) => {
                            // Files can legitimately be missing for failed
                            // uploads or post-delete races; log at debug and
                            // skip rather than blocking the whole sweep.
                            tracing::debug!(
                                "content_hash backfill: skip doc {} (read {} failed: {})",
                                doc_id,
                                path,
                                e
                            );
                            continue;
                        }
                    };
                    if let Err(e) = minerva_db::queries::documents::set_content_hash_if_null(
                        &state.db, doc_id, &hash,
                    )
                    .await
                    {
                        // Most likely a unique-index collision against an
                        // active row that already has this hash: another
                        // active doc in the same course has the same
                        // bytes (a pre-existing duplicate the backfill
                        // can't dedup retroactively without orphaning).
                        // Log and move on; the row stays NULL but the
                        // cursor already advanced past it above, so we
                        // won't re-pick it on the next iteration.
                        tracing::debug!("content_hash backfill: doc {} skip ({})", doc_id, e);
                    }
                }

                // One batch per second is enough to drain a 10k-doc
                // installation in ~3 minutes without contending with
                // live uploads.
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    }

    // Benchmark FastEmbed models in the background (doesn't block startup).
    // Only the small ONNX models in `STARTUP_BENCHMARK_MODELS` are warmed
    // here; loading every entry in `VALID_LOCAL_MODELS` (which now
    // includes Qwen3 0.6B, bge-m3, e5-large, etc.) would OOM-kill the
    // pod. Heavier candidates are benchmarked on demand via the admin
    // `POST /admin/embedding-benchmark` endpoint.
    let fastembed = state.fastembed.clone();
    // The trait carries `&[(String, u64)]` (matches the protobuf shape
    // for the Phase 1 remote variant); convert the borrowed-string
    // constant once at startup.
    let startup_models: Vec<(String, u64)> = minerva_ingest::pipeline::STARTUP_BENCHMARK_MODELS
        .iter()
        .map(|(m, d)| ((*m).to_string(), *d))
        .collect();
    tokio::spawn(async move {
        tracing::info!("running fastembed model benchmarks...");
        match fastembed.run_benchmarks(&startup_models).await {
            Ok(results) => {
                tracing::info!("fastembed benchmarks complete ({} models)", results.len());
            }
            Err(e) => {
                tracing::warn!("fastembed benchmarks failed: {}", e);
            }
        }
    });

    let mut app = Router::new()
        .nest("/api", routes::api_router(state.clone()))
        .nest(
            "/lti",
            routes::lti::public_router().with_state(state.clone()),
        );

    if let Some(ref static_dir) = config.static_dir {
        let index = format!("{}/index.html", static_dir);
        app = app.fallback_service(ServeDir::new(static_dir).fallback(ServeFile::new(index)));
        tracing::info!("serving static files from {}", static_dir);
    } else if let Some(ref proxy_url) = config.dev_proxy {
        let proxy_url_log = proxy_url.clone();
        let proxy_url = proxy_url.clone();
        let client = state.http_client.clone();
        app = app.fallback(move |req: axum::extract::Request| {
            let proxy_url = proxy_url.clone();
            let client = client.clone();
            async move {
                let uri = req.uri().to_string();
                let url = format!("{}{}", proxy_url, uri);
                match client.get(&url).send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        let headers = resp.headers().clone();
                        let body = resp.bytes().await.unwrap_or_default();
                        let mut response = Response::builder().status(status);
                        for (k, v) in headers.iter() {
                            response = response.header(k, v);
                        }
                        response
                            .body(axum::body::Body::from(body))
                            .unwrap()
                            .into_response()
                    }
                    Err(_) => axum::http::StatusCode::BAD_GATEWAY.into_response(),
                }
            }
        });
        tracing::info!("dev proxy fallback to {}", proxy_url_log);
    }

    let app = app
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = format!("{}:{}", config.host, config.port);
    tracing::info!("minerva listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Worker binary entrypoint.
///
/// Always spawns the doc-claim loop + stale-doc sweeper + relink
/// sweeper. Also spawns the periodic scheduler loops (Canvas / LTI
/// NRPS / platform health / pending platform cleanup) when
/// `MINERVA_RUN_SCHEDULER` is true (the Phase 3.5 default). The
/// Phase 3.5 cutover flips it to false on the worker once the
/// dedicated `minerva-scheduler` pod is confirmed running the loops.
pub async fn worker_main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let config = config::Config::from_env()?;
    let state = state::AppState::new(&config).await?;

    spawn_memprobe();

    tracing::info!("starting worker (doc claim + stale/relink sweepers)");
    worker::start_worker_loops(state.clone(), config.max_concurrent_ingests);

    if config.run_scheduler {
        tracing::info!("starting scheduler loops in-process (MINERVA_RUN_SCHEDULER=true)");
        worker::start_scheduler_loops(state);
    } else {
        tracing::info!(
            "scheduler: disabled in this pod (MINERVA_RUN_SCHEDULER=false); periodic loops run in the minerva-scheduler pod"
        );
    }

    tokio::signal::ctrl_c().await?;
    tracing::info!("worker: ctrl_c received, shutting down");
    Ok(())
}

/// Scheduler binary entrypoint. Boots AppState (so the loops can
/// reach DB / qdrant / embedder if they need to call route handlers
/// that happen to require them, e.g. Canvas downloads documents and
/// the worker pipeline embeds them; the scheduler itself doesn't
/// embed, but `routes::canvas::run_sync` is the same function the
/// admin "Sync now" path calls), then spawns just the periodic
/// scheduler loops and blocks on Ctrl-C.
pub async fn scheduler_main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let config = config::Config::from_env()?;
    let state = state::AppState::new(&config).await?;

    spawn_memprobe();

    tracing::info!("starting scheduler (canvas / lti nrps / platform health / pending cleanup)");
    worker::start_scheduler_loops(state);

    tokio::signal::ctrl_c().await?;
    tracing::info!("scheduler: ctrl_c received, shutting down");
    Ok(())
}

/// Walk every `course_*` collection and idempotently add the `document_id`
/// payload index. Existing indexes return an error from Qdrant which we
/// log and ignore; the goal is to bring older deployments up to speed
/// without requiring a manual migration step.
async fn backfill_document_id_indexes(qdrant: &qdrant_client::Qdrant) {
    let collections = match qdrant.list_collections().await {
        Ok(resp) => resp.collections,
        Err(e) => {
            tracing::warn!(
                "qdrant: index backfill skipped, list_collections failed: {}",
                e
            );
            return;
        }
    };
    let course_collections: Vec<String> = collections
        .into_iter()
        .map(|c| c.name)
        .filter(|n| n.starts_with("course_"))
        .collect();
    tracing::info!(
        "qdrant: backfilling document_id index across {} course collection(s)",
        course_collections.len()
    );
    for name in course_collections {
        minerva_ingest::pipeline::ensure_document_id_index(qdrant, &name).await;
    }
}

/// Selected fields from `/proc/self/status`, all in KiB / counts. Used by
/// the periodic `memprobe` task to give us a trace of process memory and
/// thread count right up to an OOM kill, so the next incident points at
/// the actual offender instead of needing a guess. Only Linux pods set
/// these; on a non-Linux dev host this returns `None` and the probe
/// silently skips.
struct ProcStatus {
    vm_rss_kb: u64,
    vm_hwm_kb: u64,
    vm_size_kb: u64,
    vm_data_kb: u64,
    threads: u64,
}

fn read_proc_self_status() -> Option<ProcStatus> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    let mut vm_rss_kb = None;
    let mut vm_hwm_kb = None;
    let mut vm_size_kb = None;
    let mut vm_data_kb = None;
    let mut threads = None;
    for line in content.lines() {
        let parse_kb = |rest: &str| -> Option<u64> {
            rest.split_whitespace().next().and_then(|s| s.parse().ok())
        };
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            vm_rss_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            vm_hwm_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("VmSize:") {
            vm_size_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("VmData:") {
            vm_data_kb = parse_kb(rest);
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            threads = parse_kb(rest);
        }
    }
    Some(ProcStatus {
        vm_rss_kb: vm_rss_kb?,
        vm_hwm_kb: vm_hwm_kb?,
        vm_size_kb: vm_size_kb?,
        vm_data_kb: vm_data_kb?,
        threads: threads?,
    })
}
