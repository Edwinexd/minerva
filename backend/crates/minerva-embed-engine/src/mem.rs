//! Shared memory primitives for the model caches.
//!
//! Both model caches (`fastembed_embedder::FastEmbedder` and
//! `reranker::FastReranker`) front the same `fastembed` runtime, so the
//! machinery that keeps their resident set under a cgroup budget is
//! identical: read process RSS, size a budget off the cgroup limit,
//! estimate a model's cost *before* loading it, and after an eviction
//! wait for the freed pages to actually leave RSS (glibc holds them
//! otherwise and the next load lands on top, OOM-killing the pod).
//!
//! These were originally private to `fastembed_embedder`; they live here
//! now so `model_cache::ModelCache` can drive both caches from one
//! implementation rather than two copies that drift. The functions are
//! byte-for-byte the same logic; the only generalization is that
//! `wait_for_rss_drop` takes a `label` so its log lines name the cache
//! they belong to (`fastembed` / `reranker`).

/// One mebibyte, for the `/ MIB` conversions in log + metric math.
pub(crate) const MIB: u64 = 1024 * 1024;

/// Cost assumed for a model when RSS introspection isn't available
/// (non-Linux dev hosts) or returns nonsense, and the fallback estimate
/// when nothing better is known. Picked on the high side of the largest
/// ONNX model we load so the budget logic still throttles without a real
/// measurement. (Was `ESTIMATED_MODEL_COST_BYTES` in `fastembed_embedder`.)
pub(crate) const DEFAULT_ESTIMATE_BYTES: u64 = 800 * MIB;

/// Floor for a model's recorded RSS cost when the static table has no
/// entry for it. Nothing we load, ONNX embedder or cross-encoder, warms
/// to less than this, so a measurement below it is a failed measurement
/// rather than a cheap model. See `ModelCache::acquire` for why a bogus
/// low reading is worse than no reading at all.
pub(crate) const MIN_PLAUSIBLE_MODEL_BYTES: u64 = 256 * MIB;

/// Fraction of the cgroup limit that live RSS plus an incoming model's
/// estimated cost may reach before a load is refused.
pub(crate) const RSS_ADMISSION_FRACTION: f64 = 0.85;

/// Guard a model load against the *actual* cgroup limit rather than the
/// cache's own accounting.
///
/// The LRU budget is bookkeeping, and bookkeeping drifts: a cost
/// measurement can come back wrong, an evicted model's pages can outlive
/// the wait, a model outside the static table gets a guessed cost. Live
/// RSS against `memory.max` cannot drift. Refusing one request is
/// strictly better than an OOM kill, which fails every in-flight request
/// in the pod and then costs a cold start on top.
///
/// `Ok(())` when there is room, or when either number is unavailable (no
/// cgroup, no `/proc`), so non-Linux dev hosts are unaffected.
/// Block until the process actually has room for `needed` bytes, or give
/// up after `HEADROOM_WAIT_BUDGET`. Returns true when there is room.
///
/// This replaces "wait 5s for RSS to halve, then load anyway and hope".
/// Two things were wrong with that. It asked the wrong question, since
/// what matters is whether the *incoming* model fits, not whether the
/// outgoing one shrank by an arbitrary fraction. And giving up meant
/// proceeding into the allocation regardless, which is precisely the
/// OOM: "next load may be tight" was the last line before the kill.
///
/// Tying the wait to the real question makes it self-scaling: a load
/// that already fits does not wait at all, and one that does not fit
/// keeps waiting and keeps trimming rather than gambling. The budget is
/// generous because glibc routinely takes longer than the old 5s to
/// return ORT arenas: prod logged "released 7 of 2816 MiB" at 5s, and
/// the next memprobe 5s later showed the pages had in fact gone. Those
/// few extra seconds are the difference between serving the request and
/// refusing it.
pub(crate) async fn wait_for_headroom(needed: u64, label: &str) -> bool {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
    const HEADROOM_WAIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

    if check_rss_headroom(needed).is_ok() {
        return true;
    }

    let started = std::time::Instant::now();
    while started.elapsed() < HEADROOM_WAIT_BUDGET {
        trim_glibc_heap().await;
        tokio::time::sleep(POLL_INTERVAL).await;
        if check_rss_headroom(needed).is_ok() {
            tracing::debug!(
                "{} cache: headroom for {} MiB arrived after {:.1}s",
                label,
                needed / MIB,
                started.elapsed().as_secs_f64(),
            );
            return true;
        }
    }

    tracing::warn!(
        "{} cache: waited {}s and still no room for {} MiB ({}); refusing the load rather than \
         allocating into an OOM",
        HEADROOM_WAIT_BUDGET.as_secs(),
        label,
        needed / MIB,
        check_rss_headroom(needed).err().unwrap_or_default(),
    );
    false
}

pub(crate) fn check_rss_headroom(estimate: u64) -> Result<(), String> {
    let (Some(limit), Some(rss)) = (read_cgroup_memory_limit(), read_rss_bytes()) else {
        return Ok(());
    };
    let ceiling = (limit as f64 * RSS_ADMISSION_FRACTION) as u64;
    if rss.saturating_add(estimate) > ceiling {
        return Err(format!(
            "live RSS {} MiB + estimated {} MiB would exceed {} MiB ({}% of the {} MiB cgroup limit)",
            rss / MIB,
            estimate / MIB,
            ceiling / MIB,
            (RSS_ADMISSION_FRACTION * 100.0).round() as u32,
            limit / MIB,
        ));
    }
    Ok(())
}

/// Conservative a-priori warmed-RSS estimate for a model, in bytes.
///
/// This is the number the admission gate uses *before* a model is ever
/// loaded, so it can refuse a load that can't fit instead of loading it
/// and OOM-killing the pod. It is deliberately a hand-maintained table
/// rather than derived from file size: the dominant cost is the ORT
/// arena that only materializes on the first warmed forward pass (2-3x
/// the bare weights; see `fastembed_embedder`'s warmup-before-measure
/// note), which isn't visible from disk before the model is loaded.
///
/// Values are conservative upper bounds (rounded up from the prod
/// memprobe / load-log numbers). They only govern the *first* admission
/// decision for a model: once it's been loaded once, the cache records
/// the real measured cost and uses that instead. Unknown ids return
/// `None`, which the cache treats as "no confident estimate" and falls
/// back to its load-anyway-the-limit-is-the-backstop behavior.
pub(crate) fn estimated_model_rss_bytes(model: &str) -> Option<u64> {
    // MiB; see module note. Keep these >= observed warmed RSS so the
    // gate never green-lights a load that then overshoots the cgroup.
    let mib: u64 = match model {
        // --- Embedding models (warmed RSS deltas observed in prod) ---
        "sentence-transformers/all-MiniLM-L6-v2" => 1024,
        "BAAI/bge-small-en-v1.5" => 1024,
        "BAAI/bge-base-en-v1.5" => 1280,
        // Warmed to 2846 MiB in a 6 GiB container, over the old 2816
        // entry. The table has to stay an upper bound or the floor below
        // stops being one.
        "nomic-ai/nomic-embed-text-v1.5" => 3072,
        "intfloat/multilingual-e5-small" => 1024,
        "intfloat/multilingual-e5-base" => 1408,
        "intfloat/multilingual-e5-large" => 2304,
        "BAAI/bge-m3" => 2304,
        "google/embeddinggemma-300m" => 1408,
        "Snowflake/snowflake-arctic-embed-m-v2.0" => 3456,
        "mixedbread-ai/mxbai-embed-large-v1" => 1792,
        "Alibaba-NLP/gte-large-en-v1.5" => 1792,
        "snowflake/snowflake-arctic-embed-l" => 1792,
        "Qwen/Qwen3-Embedding-0.6B" => 2560,

        // --- Cross-encoder rerankers ---
        "jinaai/jina-reranker-v2-base-multilingual" => 1856,
        // fp32, ~568M params + external data file; the heavy one that
        // OOM-killed the 3Gi reranker pod when stacked on top of jina.
        "rozgo/bge-reranker-v2-m3" => 2816,
        "BAAI/bge-reranker-base" => 1280,
        "jinaai/jina-reranker-v1-turbo-en" => 768,

        _ => return None,
    };
    Some(mib * MIB)
}

/// Byte budget for a model cache: env override first, then a fraction of
/// the cgroup memory limit, then a static fallback. Generalizes
/// `fastembed_embedder::compute_budget_bytes` so each cache can pass its
/// own env-var name + fraction + fallback.
pub(crate) fn budget_bytes(env_var: &str, fraction: f64, default: u64) -> u64 {
    if let Ok(v) = std::env::var(env_var) {
        if let Ok(n) = v.parse::<u64>() {
            return n;
        }
    }
    if let Some(limit) = read_cgroup_memory_limit() {
        return ((limit as f64) * fraction) as u64;
    }
    default
}

/// Human-readable description of where `budget_bytes` got its number,
/// for the startup log line.
pub(crate) fn budget_source(env_var: &str, fraction: f64) -> String {
    if std::env::var(env_var).is_ok() {
        format!("env override {env_var}")
    } else if read_cgroup_memory_limit().is_some() {
        format!(
            "{}% of cgroup memory.max",
            (fraction * 100.0).round() as u32
        )
    } else {
        "static default (no cgroup, no env override)".to_string()
    }
}

/// Poll `/proc/self/status` until VmRSS drops by at least
/// `evicted_cost / 2` (measured against the value at entry), or a 5s
/// budget elapses. Called after dropping a cache entry's handle (and
/// awaiting any per-model task) so the next load doesn't allocate on top
/// of a model that hasn't actually left RSS yet.
///
/// `label` names the cache in the log lines (`fastembed` / `reranker`).
/// The mutex is released by the caller before this runs so other
/// acquire callers can make progress.
pub(crate) async fn wait_for_rss_drop(evicted_cost: u64, label: &str) {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
    const EVICTION_WAIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);
    let Some(before) = read_rss_bytes() else {
        return; // RSS introspection unavailable, can't tell either way
    };

    // Force glibc to return freed pages to the kernel. Without this the
    // handle has dropped but ORT's arena pool sits in glibc's freelist as
    // RSS for an indeterminate time, so the next model load allocates on
    // top of the lingering arena and OOM-kills the pod.
    trim_glibc_heap().await;

    let target_drop = evicted_cost / 2;
    let deadline = std::time::Instant::now() + EVICTION_WAIT_BUDGET;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        if let Some(now) = read_rss_bytes() {
            if before.saturating_sub(now) >= target_drop {
                tracing::debug!(
                    "{} cache: eviction freed {} MiB (target {} MiB)",
                    label,
                    (before - now) / MIB,
                    target_drop / MIB,
                );
                return;
            }
        }
    }
    let now = read_rss_bytes().unwrap_or(before);
    tracing::warn!(
        "{} cache: eviction did not free expected memory within {}s (released {} of {} MiB); next load may be tight",
        label,
        EVICTION_WAIT_BUDGET.as_secs(),
        before.saturating_sub(now) / MIB,
        evicted_cost / MIB,
    );
}

/// glibc-specific: ask malloc to return freed pages from its arena
/// freelist back to the kernel. Without this post-eviction RSS stays
/// artificially high because freed ORT arenas sit in glibc's freelist,
/// even after the handle has dropped, and the next model load allocates
/// ON TOP of the lingering arena and trips the cgroup limit.
#[cfg(target_env = "gnu")]
pub(crate) async fn trim_glibc_heap() {
    // malloc_trim can take low-tens-of-ms on a fragmented heap; hand off
    // to the blocking pool so the caller's await point doesn't block.
    let _ = tokio::task::spawn_blocking(|| {
        // SAFETY: `malloc_trim` is a glibc extension with a stable
        // signature `int malloc_trim(size_t pad)`. We want the side
        // effect (return freed pages to the kernel), not the return value.
        unsafe {
            libc::malloc_trim(0);
        }
    })
    .await;
}

#[cfg(not(target_env = "gnu"))]
pub(crate) async fn trim_glibc_heap() {
    // musl / macOS: nothing equivalent. Allocator returns pages promptly
    // on free anyway.
}

/// Current process resident set size, in bytes, from
/// `/proc/self/status:VmRSS`. `None` on hosts without that file.
pub(crate) fn read_rss_bytes() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

/// Read the active cgroup memory limit. Cgroup v2 first, v1 fallback.
/// Returns `None` for unlimited or unreadable.
pub(crate) fn read_cgroup_memory_limit() -> Option<u64> {
    // v2 uses "max" (literal) for unlimited; v1 uses a sentinel that
    // exceeds physical memory. In either unlimited case we return None so
    // the caller falls back to the static default.
    if let Ok(s) = std::fs::read_to_string("/sys/fs/cgroup/memory.max") {
        let trimmed = s.trim();
        if trimmed == "max" {
            return None;
        }
        if let Ok(n) = trimmed.parse::<u64>() {
            return Some(n);
        }
    }
    if let Ok(s) = std::fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes") {
        if let Ok(n) = s.trim().parse::<u64>() {
            // v1 sentinel "unlimited" is a value larger than any realistic
            // RAM. Treat anything north of 1 PiB as unlimited.
            if n >= 1u64 << 50 {
                return None;
            }
            return Some(n);
        }
    }
    None
}
