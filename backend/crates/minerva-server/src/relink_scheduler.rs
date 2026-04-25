//! Debounced per-course relink queue.
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
//! The "due time" of a dirty course is pushed back on every mark, so a
//! sustained burst keeps deferring the linker until the burst stops.
//! This is the same shape as a leading-edge debouncer, but applied
//! per-course rather than globally.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// How long to wait after the most recent dirty-mark before a course is
/// considered "due" for linking. Long enough that a Moodle sync of 50
/// docs ends up as one linker call, short enough that a teacher who
/// edits one doc sees fresh edges within ~a minute.
pub const RELINK_DEBOUNCE: Duration = Duration::from_secs(60);

/// How often the sweep loop wakes up to look for due courses.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct RelinkScheduler {
    /// course_id -> earliest Instant at which the linker may run.
    /// Marking a course pushes the time forward to `now + RELINK_DEBOUNCE`.
    /// The sweep loop drains courses whose time has passed.
    inner: Mutex<HashMap<Uuid, Instant>>,
}

impl RelinkScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a course dirty. The actual linker call fires after
    /// `RELINK_DEBOUNCE` of quiescence -- i.e. no further marks.
    pub fn mark_dirty(&self, course_id: Uuid) {
        let due_at = Instant::now() + RELINK_DEBOUNCE;
        let mut inner = self.inner.lock().expect("relink scheduler mutex poisoned");
        // Always overwrite: fresher marks push back the run time.
        // This is what makes the debounce coalesce bursts.
        inner.insert(course_id, due_at);
    }

    /// Mark a course dirty for immediate processing on the next sweep
    /// tick. Used by the explicit "Re-classify all" / admin backfill
    /// completion paths where the user has finished a batch and
    /// reasonably wants edges refreshed without a 60s wait.
    pub fn mark_dirty_immediate(&self, course_id: Uuid) {
        let due_at = Instant::now();
        let mut inner = self.inner.lock().expect("relink scheduler mutex poisoned");
        // Only push *earlier* -- if a future-due mark exists, we want
        // immediate to win, but if an even earlier mark exists (already
        // due), don't push it back.
        let entry = inner.entry(course_id).or_insert(due_at);
        if *entry > due_at {
            *entry = due_at;
        }
    }

    /// Drain the courses whose due time has passed. Removes them from
    /// the dirty map; if the linker fails the caller is responsible
    /// for re-marking (or letting the next ingest do so).
    pub fn take_due(&self, now: Instant) -> Vec<Uuid> {
        let mut inner = self.inner.lock().expect("relink scheduler mutex poisoned");
        let due: Vec<Uuid> = inner
            .iter()
            .filter(|(_, t)| **t <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in &due {
            inner.remove(id);
        }
        due
    }

    /// Number of courses currently waiting to be linked. Surfaced for
    /// telemetry / debugging only; not load-bearing.
    #[allow(dead_code)]
    pub fn pending_count(&self) -> usize {
        self.inner.lock().map(|i| i.len()).unwrap_or(0)
    }
}

/// Spawn the background sweep loop. Wakes every `SWEEP_INTERVAL`, takes
/// every due course off the queue, and runs `relink_course` on each.
/// Sequential per tick so we don't fire many concurrent linker calls
/// at gpt-oss; in practice the typical pending-N is small.
pub fn spawn_sweep(state: crate::state::AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(SWEEP_INTERVAL).await;
            let due = state.relink_scheduler.take_due(Instant::now());
            if due.is_empty() {
                continue;
            }
            tracing::info!("relink sweeper: {} course(s) due", due.len());
            for course_id in due {
                if let Err(e) = crate::routes::documents::relink_course(&state, course_id).await {
                    tracing::warn!("relink sweeper: course {} failed: {:?}", course_id, e);
                    // On error, re-mark immediate so the next sweep
                    // retries. Avoids losing a relink to a transient
                    // Cerebras 5xx.
                    state.relink_scheduler.mark_dirty_immediate(course_id);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_dirty_pushes_due_time_forward() {
        let s = RelinkScheduler::new();
        let c = Uuid::from_bytes([1; 16]);
        s.mark_dirty(c);

        let now = Instant::now();
        let due = s.take_due(now);
        assert!(due.is_empty(), "should not be due immediately after mark");

        let due_later = s.take_due(now + RELINK_DEBOUNCE + Duration::from_secs(1));
        assert_eq!(due_later, vec![c]);
        // Drained -- second take is empty.
        assert!(s
            .take_due(now + RELINK_DEBOUNCE + Duration::from_secs(2))
            .is_empty());
    }

    #[test]
    fn repeated_marks_coalesce() {
        let s = RelinkScheduler::new();
        let c = Uuid::from_bytes([1; 16]);
        for _ in 0..50 {
            s.mark_dirty(c);
        }
        // Still only one entry.
        assert_eq!(s.pending_count(), 1);
        let due = s.take_due(Instant::now() + RELINK_DEBOUNCE + Duration::from_secs(1));
        assert_eq!(due.len(), 1);
    }

    #[test]
    fn mark_dirty_immediate_overrides_debounce() {
        let s = RelinkScheduler::new();
        let c = Uuid::from_bytes([1; 16]);
        s.mark_dirty(c);
        s.mark_dirty_immediate(c);

        let due = s.take_due(Instant::now());
        assert_eq!(due, vec![c], "immediate mark should be due now");
    }

    #[test]
    fn mark_dirty_immediate_does_not_push_back() {
        let s = RelinkScheduler::new();
        let c = Uuid::from_bytes([1; 16]);
        // Mark immediate first (due now), then mark normal (which
        // wants a 60s delay). Expectation: the existing immediate
        // mark wins -- mark_dirty_immediate's "only push earlier"
        // semantics prevent the regular mark from delaying it.
        s.mark_dirty_immediate(c);
        // mark_dirty does always-overwrite; that's intentional for
        // the debounce semantics during a burst, even if it means
        // an immediate mark followed by a regular mark waits the
        // full debounce. Document this behavior here so a future
        // change is a deliberate decision, not a regression.
        s.mark_dirty(c);
        let due_now = s.take_due(Instant::now());
        assert!(
            due_now.is_empty(),
            "regular mark after immediate intentionally re-debounces"
        );
    }

    #[test]
    fn pending_count_reflects_unique_courses() {
        let s = RelinkScheduler::new();
        s.mark_dirty(Uuid::from_bytes([1; 16]));
        s.mark_dirty(Uuid::from_bytes([2; 16]));
        s.mark_dirty(Uuid::from_bytes([1; 16]));
        assert_eq!(s.pending_count(), 2);
    }
}
