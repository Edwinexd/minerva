//! Persistent debounced per-course relink queue.
//!
//! Every time a document's classification changes -- worker auto-classify
//! after ingest, single-doc reclassify endpoint, teacher kind override --
//! the course gets marked dirty. A background sweep loop running inside
//! `worker::start` drains due courses every `SWEEP_INTERVAL` seconds and
//! runs the cross-doc linker on each.
//!
//! Why debounce: Moodle sync uploads N docs at once. Without coalescing
//! we'd fire N linker calls, each rebuilding the whole course graph,
//! N-1 of them throwaway. With a 60s debounce, a normal sync settles
//! into a single linker call after the burst.
//!
//! Why a max-defer cap: a previous in-memory implementation always
//! pushed `due_at` forward to `now + RELINK_DEBOUNCE` on every mark.
//! That meant a slow Moodle sync (one doc every ~20-30s for 50 docs)
//! kept resetting the timer indefinitely and the linker NEVER fired
//! during the burst -- the user-reported "auto-ingest doesn't update
//! the graph" bug. We now cap the wait at `first_marked_at +
//! MAX_PENDING_AGE` so even a sustained burst guarantees a relink
//! within ~5 minutes of the first mark.
//!
//! Why DB-backed: a server restart used to silently drop every
//! pending mark, so a course that was 30 seconds away from a relink
//! could end up not relinked at all if the pod restarted in that
//! window. The queue now lives in `relink_queue` and the DB-side
//! `ON CONFLICT (course_id)` upsert collapses concurrent marks into
//! one row safely. The scheduler struct is now a thin async wrapper
//! around `minerva_db::queries::relink_queue`.

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

/// How long to wait after the most recent dirty-mark before a course is
/// considered "due" for linking. Short enough that a teacher uploading
/// a single doc sees fresh edges within ~half a minute, long enough
/// that a bursty Moodle sync of N docs (typical inter-arrival ~1-3s)
/// still coalesces into one linker call rather than N.
pub const RELINK_DEBOUNCE: Duration = Duration::from_secs(20);

/// Hard cap on how long the debounce can defer a queued relink. After
/// this many seconds since `first_marked_at`, the linker fires on the
/// next sweep regardless of any subsequent marks. Prevents a slow,
/// sustained burst (Moodle sync of 50 docs across 10 minutes) from
/// indefinitely starving the linker.
pub const MAX_PENDING_AGE: Duration = Duration::from_secs(5 * 60);

/// How often the sweep loop wakes up to look for due courses. Tightened
/// to 5s (from 10s) so the worst-case "ingest finishes, relink debounce
/// expires, but we wait for next tick" window stays short for
/// single-doc uploads.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Async wrapper around the DB-backed relink queue. Holds a pool clone
/// so callers can keep using `state.relink_scheduler.mark_dirty(course_id)`
/// without threading the pool everywhere.
#[derive(Clone)]
pub struct RelinkScheduler {
    db: PgPool,
}

impl RelinkScheduler {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }

    /// Mark a course dirty. The actual linker call fires after
    /// `RELINK_DEBOUNCE` of quiescence -- i.e. no further marks --
    /// OR `MAX_PENDING_AGE` after the FIRST mark, whichever comes
    /// first. The cap is what makes long bursts not starve the linker.
    pub async fn mark_dirty(&self, course_id: Uuid) {
        if let Err(e) = minerva_db::queries::relink_queue::mark_dirty(
            &self.db,
            course_id,
            RELINK_DEBOUNCE.as_secs() as i64,
            MAX_PENDING_AGE.as_secs() as i64,
        )
        .await
        {
            tracing::warn!(
                "relink_scheduler: mark_dirty({}) failed: {} -- linker will not run",
                course_id,
                e
            );
        }
    }

    /// Mark a course dirty for immediate processing on the next sweep
    /// tick. Used by the explicit "Re-classify all" / admin backfill
    /// completion paths where the user has finished a batch and
    /// reasonably wants edges refreshed without a 60s wait.
    pub async fn mark_dirty_immediate(&self, course_id: Uuid) {
        if let Err(e) =
            minerva_db::queries::relink_queue::mark_dirty_immediate(&self.db, course_id).await
        {
            tracing::warn!(
                "relink_scheduler: mark_dirty_immediate({}) failed: {}",
                course_id,
                e
            );
        }
    }

    /// Drain courses whose due time has passed.
    pub async fn take_due(&self) -> Vec<Uuid> {
        match minerva_db::queries::relink_queue::take_due(&self.db).await {
            Ok(rows) => rows.into_iter().map(|r| r.course_id).collect(),
            Err(e) => {
                tracing::warn!("relink_scheduler: take_due failed: {}", e);
                Vec::new()
            }
        }
    }

    /// Number of courses currently waiting; surfaced for telemetry.
    #[allow(dead_code)]
    pub async fn pending_count(&self) -> i64 {
        minerva_db::queries::relink_queue::pending_count(&self.db)
            .await
            .unwrap_or(0)
    }
}

/// Spawn the background sweep loop. Wakes every `SWEEP_INTERVAL`, takes
/// every due course off the queue, and runs `relink_course` on each.
/// Sequential per tick so we don't fire many concurrent linker calls
/// at gpt-oss; in practice the typical pending-N is small.
pub fn spawn_sweep(state: crate::state::AppState) {
    tracing::info!(
        "relink sweeper: spawning (debounce {}s, max-age {}s, sweep every {}s)",
        RELINK_DEBOUNCE.as_secs(),
        MAX_PENDING_AGE.as_secs(),
        SWEEP_INTERVAL.as_secs(),
    );
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_INTERVAL).await;
            let due = state.relink_scheduler.take_due().await;
            if due.is_empty() {
                continue;
            }
            tracing::info!("relink sweeper: firing linker for {} course(s)", due.len());
            for course_id in due {
                let started = std::time::Instant::now();
                match crate::routes::documents::relink_course(&state, course_id).await {
                    Ok(edges_written) => {
                        tracing::info!(
                            "relink sweeper: course {} done in {}ms ({} edges)",
                            course_id,
                            started.elapsed().as_millis(),
                            edges_written,
                        );
                    }
                    Err(e) => {
                        tracing::warn!("relink sweeper: course {} failed: {:?}", course_id, e);
                        // On error, re-mark immediate so the next sweep
                        // retries. Avoids losing a relink to a transient
                        // Cerebras 5xx.
                        state.relink_scheduler.mark_dirty_immediate(course_id).await;
                    }
                }
            }
        }
    });
}
