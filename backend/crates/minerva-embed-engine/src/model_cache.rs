//! Generic memory-budgeted LRU model cache.
//!
//! Both the embedding cache (`fastembed_embedder::FastEmbedder`) and the
//! cross-encoder cache (`reranker::FastReranker`) front the same
//! `fastembed` runtime and need the *same* discipline to stay under a
//! cgroup memory budget:
//!
//!   1. **Estimate before loading.** A confident a-priori estimate
//!      (measured-before, else the static table in [`crate::mem`]) that
//!      exceeds the whole budget can never fit no matter what we evict,
//!      so the cache refuses the load up front rather than loading it and
//!      letting the kernel OOM-kill the pod. This is the
//!      "won't OOM-load" admission gate.
//!   2. **Evict LRU to make room**, using the estimate as the incoming
//!      cost; drop the evicted handle, await any per-model teardown, then
//!      [`crate::mem::wait_for_rss_drop`] so the freed pages actually
//!      leave RSS before the next allocation.
//!   3. **Warm before measuring.** ORT lazily allocates its arena on the
//!      first forward pass, 2-3x the bare weights; the loader warms the
//!      model at operational shape so the measured RSS delta is honest.
//!   4. **Post-load sweep.** The pre-load estimate can undershoot a
//!      first-of-its-kind larger model; once the real cost is known,
//!      sweep the LRU again (never evicting the entry we just inserted).
//!
//! The two caches differ only in *what* a model is (an embedding model
//! behind a priority dispatcher task vs. a cross-encoder behind a plain
//! mutex) and *how* it is loaded / warmed / torn down. Those differences
//! are the [`ModelLoader`] trait; everything above lives here, once.
//!
//! Metric *names* stay per-cache (`fastembed_*` / `reranker_*`) because
//! the Grafana dashboards key on them; the loader emits them through the
//! `metric_*` hooks so this generic core never has to build a metric
//! name at runtime.

use std::collections::HashMap;
use std::time::Instant;

use async_trait::async_trait;

use crate::mem;

/// How a concrete cache loads, warms, finalizes and tears down its model
/// type. `ModelCache` owns the LRU/budget algorithm and calls into this
/// for the type-specific bits.
#[async_trait]
pub(crate) trait ModelLoader: Send + Sync + 'static {
    /// Freshly-loaded model, before it's wrapped for caching. For the
    /// embedder this is the bare `LoadedModel` the warmup runs against;
    /// for the reranker it's already the final handle.
    type Raw: Send + 'static;

    /// Cheaply-cloneable handle handed to callers and stored in the
    /// cache (an `Arc` inside). Cloning must not duplicate the model.
    type Handle: Clone + Send + Sync + 'static;

    /// Opaque per-entry token the cache holds alongside the handle and
    /// hands back to [`ModelLoader::teardown`] on eviction. For the
    /// embedder this is the dispatcher task's `JoinHandle` (awaited so
    /// the model's RSS is provably freed before the next load); for the
    /// reranker there's no task, so it's `()`.
    type Teardown: Send + 'static;

    /// Short prefix naming this cache in logs (`fastembed` / `reranker`).
    fn label(&self) -> &'static str;

    /// Conservative a-priori warmed-RSS estimate for `model_id`, if
    /// known. `None` means "no confident estimate" and disables the
    /// hard refusal for this id (the cache loads it and relies on the
    /// pod limit as the backstop, as before).
    fn estimate_bytes(&self, model_id: &str) -> Option<u64>;

    /// Load (download + session build) the model. Heavy + blocking work
    /// belongs on `spawn_blocking` inside here.
    async fn load(&self, model_id: &str) -> Result<Self::Raw, String>;

    /// Run a forward pass at operational shape so ORT's arena
    /// materializes before the cache measures RSS. Best-effort: errors
    /// are ignored (the next real inference surfaces them).
    async fn warmup(&self, raw: &Self::Raw);

    /// Wrap the freshly-loaded `raw` into the cached handle + teardown
    /// token. For the embedder this spawns the dispatcher task.
    fn finalize(&self, raw: Self::Raw) -> (Self::Handle, Self::Teardown);

    /// Release an evicted entry so its RSS can be reclaimed: drop the
    /// handle and await any background task. The cache calls
    /// `wait_for_rss_drop` afterwards.
    async fn teardown(&self, handle: Self::Handle, teardown: Self::Teardown);

    // --- Metric hooks (default no-op; each cache emits its own
    //     literal-named metrics so the dashboards keep working). ---
    fn metric_hit(&self, _model_id: &str) {}
    fn metric_miss(&self, _model_id: &str) {}
    fn metric_evicted(&self, _model_id: &str) {}
    fn metric_resident(&self, _model_id: &str, _cost_bytes: u64) {}
    fn metric_load_seconds(&self, _model_id: &str, _secs: f64) {}
    fn metric_totals(&self, _count: usize, _footprint_bytes: u64, _budget_bytes: u64) {}
}

struct Entry<H, T> {
    name: String,
    handle: H,
    teardown: T,
    rss_cost_bytes: u64,
    last_used: Instant,
}

struct CacheState<H, T> {
    entries: Vec<Entry<H, T>>,
    /// Per-model measured warmed RSS, kept even after eviction so a
    /// re-load of the same model uses its real cost (not the static
    /// estimate) for both the admission gate and eviction math.
    measured: HashMap<String, u64>,
}

/// Memory-budgeted LRU over models loaded through a [`ModelLoader`].
///
/// The budget caps the sum of measured per-model costs; it does not cap
/// total process RSS. The admission gate ([`ModelCache::acquire`])
/// refuses any model whose confident estimate exceeds the whole budget.
pub(crate) struct ModelCache<L: ModelLoader> {
    loader: L,
    budget_bytes: u64,
    state: tokio::sync::Mutex<CacheState<L::Handle, L::Teardown>>,
}

impl<L: ModelLoader> ModelCache<L> {
    pub(crate) fn new(loader: L, budget_bytes: u64) -> Self {
        Self {
            loader,
            budget_bytes,
            state: tokio::sync::Mutex::new(CacheState {
                entries: Vec::new(),
                measured: HashMap::new(),
            }),
        }
    }

    /// Get (loading on first use) the handle for `model_id`. Holds the
    /// admission lock across the load so a concurrent first-use of the
    /// same model waits rather than racing a second download. Returns a
    /// clear error (and logs ERROR) if a confident estimate says the
    /// model can't fit the budget, instead of OOM-loading it.
    pub(crate) async fn acquire(&self, model_id: &str) -> Result<L::Handle, String> {
        let label = self.loader.label();
        let mut state = self.state.lock().await;

        if let Some(idx) = state.entries.iter().position(|e| e.name == model_id) {
            state.entries[idx].last_used = Instant::now();
            self.loader.metric_hit(model_id);
            return Ok(state.entries[idx].handle.clone());
        }

        // A confident estimate (measured before, else the static table)
        // is the basis for both the admission gate and the eviction
        // cost. An unconfident fallback (largest resident cost, else the
        // default) keeps the old "load anyway" behavior for unknown ids.
        let confident = state
            .measured
            .get(model_id)
            .copied()
            .or_else(|| self.loader.estimate_bytes(model_id));

        if let Some(c) = confident {
            if c > self.budget_bytes {
                // No eviction can help: the budget itself is smaller than
                // this one model. Refuse before loading rather than
                // OOM-killing the pod.
                tracing::error!(
                    "{} cache: refusing to load {} (estimated {} MiB > budget {} MiB); \
                     raise the pod memory limit / cache budget env, or pick a smaller model",
                    label,
                    model_id,
                    c / mem::MIB,
                    self.budget_bytes / mem::MIB,
                );
                return Err(format!(
                    "model {} needs ~{} MiB which exceeds this pod's {} cache budget of {} MiB; \
                     raise the pod memory limit / cache budget, or pick a smaller model",
                    model_id,
                    c / mem::MIB,
                    label,
                    self.budget_bytes / mem::MIB,
                ));
            }
        }

        let estimate = confident.unwrap_or_else(|| {
            state
                .entries
                .iter()
                .map(|e| e.rss_cost_bytes)
                .max()
                .unwrap_or(mem::DEFAULT_ESTIMATE_BYTES)
        });

        // Pre-load eviction: evict LRU until adding `estimate` fits under
        // budget. Empty cache => just load it; the pod limit is the
        // backstop (and the gate above already rejected anything that
        // provably can't fit).
        while !state.entries.is_empty() {
            let used: u64 = state.entries.iter().map(|e| e.rss_cost_bytes).sum();
            if used + estimate <= self.budget_bytes {
                break;
            }
            let lru_idx = state
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
                .expect("cache non-empty");
            let evicted = state.entries.remove(lru_idx);
            self.loader.metric_evicted(&evicted.name);
            let evicted_cost = evicted.rss_cost_bytes;
            let footprint: u64 = state.entries.iter().map(|e| e.rss_cost_bytes).sum();
            tracing::info!(
                "{} cache: evicting {} ({} MiB) to make room for {} (footprint {} MiB / budget {} MiB)",
                label,
                evicted.name,
                evicted_cost / mem::MIB,
                model_id,
                footprint / mem::MIB,
                self.budget_bytes / mem::MIB,
            );
            // Release the lock so other acquire callers progress, drop
            // the handle + await teardown (so the model's RSS is provably
            // released), then wait for the pages to actually leave RSS
            // before the next load allocates on top.
            drop(state);
            self.loader.teardown(evicted.handle, evicted.teardown).await;
            mem::wait_for_rss_drop(evicted_cost, label).await;
            state = self.state.lock().await;
        }

        self.loader.metric_miss(model_id);
        let load_start = Instant::now();
        let rss_before = mem::read_rss_bytes();
        let raw = self.loader.load(model_id).await?;
        self.loader
            .metric_load_seconds(model_id, load_start.elapsed().as_secs_f64());

        // Warm at operational shape before measuring; see module note.
        self.loader.warmup(&raw).await;
        let rss_after = mem::read_rss_bytes();

        let cost = match (rss_before, rss_after) {
            (Some(b), Some(a)) => a.saturating_sub(b),
            _ => confident.unwrap_or(mem::DEFAULT_ESTIMATE_BYTES),
        };

        let (handle, teardown) = self.loader.finalize(raw);
        state.entries.push(Entry {
            name: model_id.to_string(),
            handle: handle.clone(),
            teardown,
            rss_cost_bytes: cost,
            last_used: Instant::now(),
        });
        state.measured.insert(model_id.to_string(), cost);
        self.loader.metric_resident(model_id, cost);

        // Post-load sweep: the pre-load estimate can undershoot a
        // first-of-its-kind larger model, leaving us over budget once the
        // real cost is known. Sweep the LRU again; the just-inserted
        // entry is newest (`last_used = now`) so it's never the victim.
        while state.entries.len() > 1 {
            let used: u64 = state.entries.iter().map(|e| e.rss_cost_bytes).sum();
            if used <= self.budget_bytes {
                break;
            }
            let lru_idx = state
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
                .expect("cache len > 1");
            let evicted = state.entries.remove(lru_idx);
            self.loader.metric_evicted(&evicted.name);
            let evicted_cost = evicted.rss_cost_bytes;
            let footprint: u64 = state.entries.iter().map(|e| e.rss_cost_bytes).sum();
            tracing::info!(
                "{} cache: post-load eviction of {} ({} MiB), pre-load estimate was too low (footprint {} MiB / budget {} MiB)",
                label,
                evicted.name,
                evicted_cost / mem::MIB,
                footprint / mem::MIB,
                self.budget_bytes / mem::MIB,
            );
            drop(state);
            self.loader.teardown(evicted.handle, evicted.teardown).await;
            mem::wait_for_rss_drop(evicted_cost, label).await;
            state = self.state.lock().await;
        }

        let footprint: u64 = state.entries.iter().map(|e| e.rss_cost_bytes).sum();
        let cached_count = state.entries.len();
        self.loader
            .metric_totals(cached_count, footprint, self.budget_bytes);
        match rss_after {
            Some(after) => tracing::info!(
                "{}: loaded model {} (+{} MiB, RSS now {} MiB, cache footprint {} MiB / budget {} MiB, {} cached)",
                label,
                model_id,
                cost / mem::MIB,
                after / mem::MIB,
                footprint / mem::MIB,
                self.budget_bytes / mem::MIB,
                cached_count,
            ),
            None => tracing::info!(
                "{}: loaded model {} (RSS measurement unavailable, assumed +{} MiB, {} cached)",
                label,
                model_id,
                cost / mem::MIB,
                cached_count,
            ),
        };

        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A fake loader whose models are real, page-touched heap buffers, so
    /// the cache's RSS measurement sees an honest per-model cost (the
    /// eviction loop keys off *measured* cost, exactly as the embedder
    /// does, so a zero-byte fake can't exercise eviction). `estimate`
    /// feeds the admission gate + pre-load eviction estimate; `alloc` is
    /// how many bytes `load` actually allocates and touches.
    struct FakeLoader {
        estimate: HashMap<String, u64>,
        alloc: HashMap<String, usize>,
        loads: Arc<AtomicUsize>,
        teardowns: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ModelLoader for FakeLoader {
        type Raw = Arc<Vec<u8>>;
        type Handle = Arc<Vec<u8>>;
        type Teardown = ();

        fn label(&self) -> &'static str {
            "fake"
        }
        fn estimate_bytes(&self, model_id: &str) -> Option<u64> {
            self.estimate.get(model_id).copied()
        }
        async fn load(&self, model_id: &str) -> Result<Arc<Vec<u8>>, String> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            let bytes = self.alloc.get(model_id).copied().unwrap_or(0);
            let mut v = vec![0u8; bytes];
            // Touch each page so the pages are actually resident (RSS),
            // not just reserved.
            let mut i = 0;
            while i < bytes {
                v[i] = 1;
                i += 4096;
            }
            Ok(Arc::new(v))
        }
        async fn warmup(&self, _raw: &Arc<Vec<u8>>) {}
        fn finalize(&self, raw: Arc<Vec<u8>>) -> (Arc<Vec<u8>>, ()) {
            (raw, ())
        }
        async fn teardown(&self, _handle: Arc<Vec<u8>>, _teardown: ()) {
            self.teardowns.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn loader(models: &[(&str, u64, usize)]) -> (FakeLoader, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let loads = Arc::new(AtomicUsize::new(0));
        let teardowns = Arc::new(AtomicUsize::new(0));
        (
            FakeLoader {
                estimate: models.iter().map(|(k, e, _)| (k.to_string(), *e)).collect(),
                alloc: models.iter().map(|(k, _, a)| (k.to_string(), *a)).collect(),
                loads: loads.clone(),
                teardowns: teardowns.clone(),
            },
            loads,
            teardowns,
        )
    }

    #[tokio::test]
    async fn admission_gate_refuses_model_bigger_than_budget() {
        // estimate 5000 MiB > budget 3000 MiB => refused before loading.
        let (l, loads, _) = loader(&[("huge", 5000 * mem::MIB, 0)]);
        let cache = ModelCache::new(l, 3000 * mem::MIB);
        let err = cache.acquire("huge").await.unwrap_err();
        assert!(err.contains("exceeds"), "got: {err}");
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cache_hit_does_not_reload() {
        let (l, loads, _) = loader(&[("m", 100 * mem::MIB, 0)]);
        let cache = ModelCache::new(l, 3000 * mem::MIB);
        let _ = cache.acquire("m").await.unwrap();
        let _ = cache.acquire("m").await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn evicts_lru_when_resident_plus_estimate_exceeds_budget() {
        // Each model really allocates ~300 MiB so the measured footprint
        // is honest; the 400 MiB budget can't hold two at once, so loading
        // `b` must evict `a`. The 300/400 gap is wide enough that RSS
        // measurement noise can't flip the decision.
        let mib = mem::MIB as usize;
        let (l, loads, teardowns) = loader(&[
            ("a", 300 * mem::MIB, 300 * mib),
            ("b", 300 * mem::MIB, 300 * mib),
        ]);
        let cache = ModelCache::new(l, 400 * mem::MIB);
        let _ = cache.acquire("a").await.unwrap();
        let _ = cache.acquire("b").await.unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        // `a` was evicted to make room for `b`.
        assert_eq!(teardowns.load(Ordering::SeqCst), 1);
    }
}
