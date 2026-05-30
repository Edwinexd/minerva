//! Pod-wide memory budget for fat background operations that allocate
//! *outside* the fastembed model cache.
//!
//! ## Why this exists
//!
//! The fastembed cache has its own measured-cost LRU budget (see
//! `fastembed_embedder::DEFAULT_CACHE_BUDGET_FRACTION`). Everything else
//! that allocates non-trivial memory in the background (the cross-encoder
//! reranker, per-doc ingest jobs, the MBZ parser, the bulk
//! reclassify-all-in-course task, the KG linker) was running with no
//! global accounting. Individually each was bounded only by its own
//! local concurrency knob (`max_concurrent_ingests` for the worker,
//! "one at a time" for the MBZ parser, "load once and never evict" for
//! the reranker). Collectively, on a 6 GiB pod, they could overrun the
//! cgroup any time enough of them happened to coincide, and the system
//! had no way to back off because no one task knew about the others.
//!
//! ## What this is
//!
//! A semaphore whose permits are measured in MiB. Each fat operation
//! calls `acquire(mib, label)` before starting heavy allocation, gets
//! an RAII guard, and releases the permits when the guard drops. When
//! permits aren't available, the call waits; callers that want to back
//! off instead (most importantly the ingest worker deciding whether to
//! claim a new doc) use `try_acquire`.
//!
//! ## Sizing
//!
//! `from_cgroup_with_reserve` picks the budget at startup:
//!
//! ```text
//!     total = cgroup_limit - fastembed_cache_budget - baseline_reserve
//! ```
//!
//! - `fastembed_cache_budget` is what fastembed's LRU is allowed to
//!   hold; we don't double-count it.
//! - `baseline_reserve` is the steady-state non-cache, non-fat-job
//!   floor: Rust + tokio + sqlx pool + qdrant client + axum + ORT
//!   runtime + glibc fragmentation overhead. The fastembed comment
//!   estimates this at ~500-1000 MiB; we err on the high side so the
//!   pod has slack.
//!
//! On the prod 6 GiB pod with the post-fix 55% cache budget that's
//! roughly 6144 - 3379 - 1024 = ~1740 MiB available for fat jobs,
//! which fits a couple of concurrent ingest jobs plus a reranker
//! model resident, with margin for the MBZ parser and chat strategy.
//!
//! `MINERVA_MEM_BUDGET_MIB` overrides everything for ops / testing.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Fallback total when the cgroup limit isn't readable and no override
/// is set. Chosen to be obviously-bounded; a dev host with this fallback
/// won't oversubscribe even on a small laptop.
const DEFAULT_TOTAL_MIB: u32 = 1024;

/// Hard cap so a misconfigured env var (or a weird cgroup number)
/// can't ask the semaphore for u32::MAX permits and panic. 64 GiB is
/// well beyond any expected pod size.
const MAX_TOTAL_MIB: u32 = 64 * 1024;

#[derive(Clone)]
pub struct MemBudget {
    inner: Arc<MemBudgetInner>,
}

struct MemBudgetInner {
    sem: Arc<Semaphore>,
    total_mib: u32,
}

/// RAII guard that returns the held MiB to the budget when dropped.
/// Holding the guard keeps the permits checked out; dropping it
/// releases them. The label is logged on release so a trace can show
/// which subsystem freed.
#[derive(Debug)]
pub struct MemBudgetGuard {
    // Permit retains the semaphore's MiB until dropped. We carry the
    // owned variant so the guard is `'static` and can travel into
    // `tokio::spawn`-ed tasks without lifetime contortions.
    _permit: OwnedSemaphorePermit,
    mib: u32,
    label: String,
}

impl MemBudgetGuard {
    pub fn mib(&self) -> u32 {
        self.mib
    }
}

impl Drop for MemBudgetGuard {
    fn drop(&mut self) {
        tracing::debug!("mem_budget: released {} MiB for {}", self.mib, self.label);
    }
}

impl MemBudget {
    /// Construct from an explicit total. Mostly for tests; production
    /// uses `from_cgroup_with_reserve`.
    pub fn new(total_mib: u32) -> Self {
        let total_mib = total_mib.clamp(1, MAX_TOTAL_MIB);
        Self {
            inner: Arc::new(MemBudgetInner {
                sem: Arc::new(Semaphore::new(total_mib as usize)),
                total_mib,
            }),
        }
    }

    /// Sized from cgroup. Returns the budget *plus* a textual
    /// description of where the number came from, so startup logs can
    /// say e.g. "mem_budget: 1740 MiB (cgroup 6144 - cache 3379 - reserve 1024)".
    ///
    /// `MINERVA_MEM_BUDGET_MIB` overrides everything; otherwise the
    /// cgroup-derived computation is used, with `DEFAULT_TOTAL_MIB`
    /// as a final fallback (no cgroup, no override).
    pub fn from_cgroup_with_reserve(
        fastembed_cache_budget_bytes: u64,
        baseline_reserve_mib: u32,
    ) -> (Self, String) {
        if let Ok(v) = std::env::var("MINERVA_MEM_BUDGET_MIB") {
            if let Ok(n) = v.parse::<u32>() {
                let source = format!("env override MINERVA_MEM_BUDGET_MIB={}", n);
                return (Self::new(n), source);
            }
        }
        let Some(cgroup_bytes) = read_cgroup_memory_limit() else {
            return (
                Self::new(DEFAULT_TOTAL_MIB),
                format!(
                    "static default {} MiB (no cgroup, no env override)",
                    DEFAULT_TOTAL_MIB
                ),
            );
        };
        let cgroup_mib = (cgroup_bytes / (1024 * 1024)) as u32;
        let cache_mib = (fastembed_cache_budget_bytes / (1024 * 1024)) as u32;
        // Saturating subtraction: if the cache budget + reserve add up
        // to more than the cgroup limit (misconfiguration), fall back
        // to a sane minimum rather than panicking.
        let total_mib = cgroup_mib
            .saturating_sub(cache_mib)
            .saturating_sub(baseline_reserve_mib)
            .max(128);
        let source = format!(
            "cgroup {} MiB - cache {} MiB - reserve {} MiB = {} MiB",
            cgroup_mib, cache_mib, baseline_reserve_mib, total_mib
        );
        (Self::new(total_mib), source)
    }

    /// Total MiB the budget was sized at. Constant for the life of the
    /// process.
    pub fn total_mib(&self) -> u32 {
        self.inner.total_mib
    }

    /// MiB currently available (not held by any guard).
    pub fn available_mib(&self) -> u32 {
        self.inner.sem.available_permits() as u32
    }

    /// Acquire `mib` permits. Waits until they're available. Returns
    /// an RAII guard; drop to release.
    ///
    /// If `mib` exceeds `total_mib`, the request can never succeed --
    /// rather than block forever we return an error so the caller can
    /// log + fall back. Practically this only fires for a
    /// misconfigured estimate.
    pub async fn acquire(&self, mib: u32, label: &str) -> Result<MemBudgetGuard, String> {
        if mib > self.inner.total_mib {
            return Err(format!(
                "mem_budget: requested {} MiB > total {} MiB ({})",
                mib, self.inner.total_mib, label
            ));
        }
        let mib_nonzero = mib.max(1);
        let permit = Arc::clone(&self.inner.sem)
            .acquire_many_owned(mib_nonzero)
            .await
            .map_err(|e| format!("mem_budget: semaphore closed: {e}"))?;
        let available = self.available_mib();
        tracing::debug!(
            "mem_budget: acquired {} MiB for {} (free {}/{} MiB)",
            mib_nonzero,
            label,
            available,
            self.inner.total_mib,
        );
        Ok(MemBudgetGuard {
            _permit: permit,
            mib: mib_nonzero,
            label: label.to_string(),
        })
    }

    /// Non-blocking variant. Returns `None` if permits aren't
    /// immediately available; the caller is expected to back off.
    /// Used by the ingest worker to decide whether to claim a new doc.
    pub fn try_acquire(&self, mib: u32, label: &str) -> Option<MemBudgetGuard> {
        if mib > self.inner.total_mib {
            return None;
        }
        let mib_nonzero = mib.max(1);
        let permit = Arc::clone(&self.inner.sem)
            .try_acquire_many_owned(mib_nonzero)
            .ok()?;
        Some(MemBudgetGuard {
            _permit: permit,
            mib: mib_nonzero,
            label: label.to_string(),
        })
    }
}

/// Read the active cgroup memory limit. Cgroup v2 first, v1 fallback.
/// Returns `None` for unlimited or unreadable. Mirrors the helper in
/// `fastembed_embedder.rs`; deliberately duplicated to avoid having
/// the budget module depend on the embedder's internals.
fn read_cgroup_memory_limit() -> Option<u64> {
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
            // v1 sentinel "unlimited" is a value larger than any
            // realistic RAM. Treat anything north of 1 PiB as unlimited.
            if n >= 1u64 << 50 {
                return None;
            }
            return Some(n);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acquire_and_drop_releases() {
        let b = MemBudget::new(100);
        assert_eq!(b.available_mib(), 100);
        {
            let _g = b.acquire(30, "test").await.unwrap();
            assert_eq!(b.available_mib(), 70);
        }
        assert_eq!(b.available_mib(), 100);
    }

    #[tokio::test]
    async fn try_acquire_fails_when_full() {
        let b = MemBudget::new(50);
        let _g = b.acquire(40, "first").await.unwrap();
        // 10 MiB left; a 20 MiB try should fail.
        assert!(b.try_acquire(20, "second").is_none());
        // A 5 MiB try should succeed.
        let _g2 = b.try_acquire(5, "third").unwrap();
        assert_eq!(b.available_mib(), 5);
    }

    #[tokio::test]
    async fn acquire_more_than_total_errors() {
        let b = MemBudget::new(100);
        let err = b.acquire(200, "huge").await.unwrap_err();
        assert!(err.contains("> total"));
    }

    #[tokio::test]
    async fn try_acquire_more_than_total_returns_none() {
        let b = MemBudget::new(100);
        assert!(b.try_acquire(200, "huge").is_none());
    }

    #[test]
    fn new_clamps_to_max() {
        let b = MemBudget::new(MAX_TOTAL_MIB + 1000);
        assert_eq!(b.total_mib(), MAX_TOTAL_MIB);
    }

    #[test]
    fn new_clamps_zero_to_one() {
        let b = MemBudget::new(0);
        assert_eq!(b.total_mib(), 1);
    }
}
