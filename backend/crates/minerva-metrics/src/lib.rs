//! Process-wide Prometheus metrics for every Minerva service binary.
//!
//! Each binary calls [`init`] once from `main`, passing its service name
//! (e.g. `"minerva-app"`). That installs a global `metrics` recorder and
//! spawns a tiny HTTP listener serving the Prometheus text exposition
//! format on `MINERVA_METRICS_PORT` (default [`DEFAULT_METRICS_PORT`]).
//! Grafana Alloy scrapes the pod on that port, selected by the
//! `prometheus.io/scrape` pod annotation in `k8s/base/`.
//!
//! Everything else in the workspace emits metrics through the lightweight
//! `metrics` facade (`counter!`, `gauge!`, `histogram!`). Those macros are
//! no-ops until [`init`] installs the recorder, so library crates depend
//! only on the facade, never on this crate; a `cargo run` of a single
//! binary with no recorder simply drops the samples.
//!
//! [`spawn_memprobe`] is the relocated `memprobe` task (was
//! `minerva_app_core::memprobe`). It now lives here so all five binaries
//! share it: the embedder / reranker pods are the OOM-prone ones and had
//! no probe before the split. It still emits the byte-identical
//! `memprobe: uptime=...` trace line a runbook greps for, and additionally
//! publishes the same `/proc/self/status` fields as gauges.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};

/// Default port the metrics HTTP listener binds. Overridable via
/// `MINERVA_METRICS_PORT`; set it to `0` to disable the exporter entirely
/// (handy for local `cargo run` where the port may be taken). 9464 is the
/// de-facto Prometheus app-exporter port, distinct from node-exporter's
/// 9100 and Prometheus' own 9090, so a reader of the pod spec isn't misled
/// into thinking this is one of those.
pub const DEFAULT_METRICS_PORT: u16 = 9464;

/// Histogram bucket bounds (seconds) applied to every metric whose name
/// ends in `_seconds`: HTTP request latency, ingest durations, model-load
/// times. Spans sub-millisecond cache hits up to multi-second model loads
/// so `histogram_quantile()` and Grafana heatmaps stay meaningful across
/// the whole range.
const SECONDS_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

/// Install the global recorder + spawn the `/metrics` HTTP listener.
///
/// `service` is attached as a `service="..."` global label to every
/// metric, so a single Grafana panel can break a value down by binary.
/// Call exactly once per process from `main`; a second call fails the
/// `metrics` facade's already-installed check, which we log and swallow so
/// a mis-wired binary degrades to "no metrics" rather than panicking on
/// boot. Must run inside a tokio runtime (the listener spawns onto it);
/// every caller is already under `#[tokio::main]`.
pub fn init(service: &'static str) {
    let port = std::env::var("MINERVA_METRICS_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(DEFAULT_METRICS_PORT);
    if port == 0 {
        tracing::info!("metrics exporter disabled (MINERVA_METRICS_PORT=0)");
        return;
    }

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    let builder = PrometheusBuilder::new()
        .with_http_listener(addr)
        .add_global_label("service", service);
    let builder = match builder
        .set_buckets_for_metric(Matcher::Suffix("_seconds".to_string()), SECONDS_BUCKETS)
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("metrics: bucket config failed, exporter not started: {e}");
            return;
        }
    };

    match builder.install() {
        Ok(()) => {
            tracing::info!("metrics exporter for {service} listening on http://{addr}/metrics")
        }
        Err(e) => tracing::warn!("metrics: exporter install failed, metrics disabled: {e}"),
    }
}

/// Spawn the periodic `/proc/self/status` memory probe.
///
/// 5 s cadence for the first 5 min of process life (the fragile startup
/// window where the embedder cache is filling), 60 s thereafter. Emits the
/// `memprobe: uptime=...` trace line (unchanged wording so existing log
/// greps keep working) and the matching `process_vm_*_bytes` /
/// `process_threads` gauges. On a non-Linux dev host `/proc/self/status`
/// is absent, so the read returns `None` and the probe silently skips.
pub fn spawn_memprobe(service: &'static str) {
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

                // Same fields as the trace line, exposed as gauges in bytes
                // (Prometheus convention: base units, no MiB scaling) so a
                // Grafana panel can plot RSS against the pod's cgroup limit
                // and the next OOM incident is a graph, not a log grep.
                // The `service` label is already attached globally by
                // `init`, distinguishing the five pods.
                let _ = service;
                metrics::gauge!("process_vm_rss_bytes").set((stats.vm_rss_kb * 1024) as f64);
                metrics::gauge!("process_vm_hwm_bytes").set((stats.vm_hwm_kb * 1024) as f64);
                metrics::gauge!("process_vm_size_bytes").set((stats.vm_size_kb * 1024) as f64);
                metrics::gauge!("process_vm_data_bytes").set((stats.vm_data_kb * 1024) as f64);
                metrics::gauge!("process_threads").set(stats.threads as f64);
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

/// Selected fields from `/proc/self/status`, all in KiB / counts. Only
/// Linux pods populate these; on a non-Linux dev host this returns `None`
/// and the probe skips.
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
