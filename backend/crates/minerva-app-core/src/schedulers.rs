//! Periodic scheduler loops: Canvas auto-sync, LTI NRPS reconcile,
//! LTI platform-health probe, and pending-platform cleanup.
//!
//! Axum-free: the loops call into `crate::canvas` (the Canvas sync engine)
//! and `crate::lti_nrps`, both of which live in this crate, so the
//! standalone `minerva-scheduler` binary can run them without linking the
//! api's route tree. The api crate boots the same loops (alongside the
//! doc-claim worker) via [`start`] when `MINERVA_RUN_WORKER` is set; the
//! `minerva-scheduler` binary calls [`start_scheduler_loops`] directly.

use crate::state::AppState;

/// How often we wake any of the periodic-sync background tasks (Canvas
/// auto-sync, LTI NRPS reconcile, LTI platform-health probe). The actual
/// cadence each task enforces is DB-driven via a "find what's due" query
/// at every tick; the tick is just how often we ASK. Short tick = restart-
/// safe (a freshly deployed pod's first cycle fires within 60 s instead
/// of up to a day later, which mattered for the 24 h health probe loop
/// when pod restarts were more frequent than the probe interval) and
/// admin edits to the per-task interval settings propagate within 60 s
/// instead of waiting up to the old sleep window.
///
/// Implemented via `tokio::time::interval` with `MissedTickBehavior::Delay`
/// so a slow tick (e.g. NRPS reconciling a backlog) shortens the next
/// tick to "now" rather than firing the missed ticks back-to-back.
const SCHEDULE_TICK: std::time::Duration = std::time::Duration::from_secs(60);

/// How long between platform-health probes for any single platform. The
/// 30-day grace in `delete_long_orphaned_platforms` is calibrated against
/// this cadence (~30 consecutive `invalid_client` results before the row
/// is cascade-deleted). The worker queries `find_platforms_due_for_health_check`
/// at every `SCHEDULE_TICK`, so the actual lag for any one platform is at
/// most this + 60 s.
const PLATFORM_HEALTH_PROBE_INTERVAL_HOURS: i32 = 24;

/// Grace period before a platform that's been continuously returning
/// `invalid_client` is cascade-deleted (taking its bindings + NRPS
/// contexts with it via FK). See `record_platform_health` for how the
/// `invalid_client_since` timestamp is maintained.
const PLATFORM_ORPHAN_GRACE_DAYS: i32 = 30;

/// Build a `tokio::time::interval` calibrated for one of the periodic-sync
/// tasks. `Delay` (not `Burst`) so a slow tick doesn't cause a burst of
/// catch-up ticks once the slow run finishes.
fn schedule_ticker() -> tokio::time::Interval {
    let mut t = tokio::time::interval(SCHEDULE_TICK);
    t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    t
}

/// Start the background document-processing worker plus all the
/// periodic scheduler loops.
///
/// Back-compat wrapper used by the api's `api_main` when
/// `MINERVA_RUN_WORKER` is true (the pre-Phase-3 monolith path). The
/// `minerva-worker` and `minerva-scheduler` binaries boot just the half
/// they own; see [`crate::worker::start_worker_loops`] and
/// [`start_scheduler_loops`].
pub fn start(state: AppState, max_concurrent: usize) {
    crate::worker::start_worker_loops(state.clone(), max_concurrent);
    start_scheduler_loops(state);
}

/// Spawn the four periodic pollers. Owned by the `minerva-scheduler`
/// binary in the Phase 3.5 topology; also called by [`start`] in the
/// monolith / api-with-worker path.
pub fn start_scheduler_loops(state: AppState) {
    // Canvas auto-sync: periodic re-sync for connections with auto_sync=true
    // whose last_synced_at is older than the configured interval. Runs
    // sequentially across due connections so we don't stampede Canvas.
    //
    // Ticks every SCHEDULE_TICK (60 s); `find_due_for_auto_sync` is the
    // source of truth for "is anything actually due". `interval_hours`
    // is read from `system_defaults` per tick so admin edits propagate
    // within 60 s; 0 disables the sync (still queries each tick in case
    // it's flipped back on).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut ticker = schedule_ticker();
            loop {
                ticker.tick().await;
                let interval_hours =
                    crate::system_defaults::canvas_auto_sync_interval_hours(&state.db).await;
                if interval_hours <= 0 {
                    continue;
                }
                let due = match minerva_db::queries::canvas::find_due_for_auto_sync(
                    &state.db,
                    interval_hours,
                )
                .await
                {
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::error!("canvas auto-sync: query failed: {}", e);
                        continue;
                    }
                };
                if due.is_empty() {
                    continue;
                }
                tracing::info!(
                    "canvas auto-sync: {} connection(s) due (interval {}h)",
                    due.len(),
                    interval_hours,
                );
                for conn in due {
                    let conn_id = conn.id;
                    let name = conn.name.clone();
                    match crate::canvas::run_sync(&state, &conn).await {
                        Ok(r) => tracing::info!(
                            "canvas auto-sync: connection {} ({}): {} new, {} resynced, {} skipped, {} errors, {} warnings",
                            conn_id,
                            name,
                            r.synced,
                            r.resynced,
                            r.skipped,
                            r.errors.len(),
                            r.warnings.len(),
                        ),
                        Err(e) => tracing::error!(
                            "canvas auto-sync: connection {} ({}) failed: {}",
                            conn_id,
                            name,
                            e,
                        ),
                    }
                }
            }
        });
    }

    // LTI NRPS reconcile: periodically pull each syncable context's roster
    // from the LMS and add/remove course members. Runs sequentially across
    // due contexts so we don't stampede a platform's token + membership
    // endpoints. Removal is LTI-sourced-only (see lti_nrps::reconcile_context).
    //
    // Ticks every SCHEDULE_TICK; `find_due_for_sync` decides what's due.
    // `nrps_interval_hours` is read from `system_defaults` per tick so
    // admin edits propagate within 60 s; 0 = skip the run for this tick
    // (we still query each tick in case it's flipped back on).
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut ticker = schedule_ticker();
            loop {
                ticker.tick().await;
                let nrps_interval_hours =
                    crate::system_defaults::lti_nrps_sync_interval_hours(&state.db).await;
                if nrps_interval_hours <= 0 {
                    continue;
                }
                let due = match minerva_db::queries::lti_nrps::find_due_for_sync(
                    &state.db,
                    nrps_interval_hours,
                )
                .await
                {
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::error!("lti nrps: due query failed: {}", e);
                        continue;
                    }
                };
                if due.is_empty() {
                    continue;
                }
                tracing::info!(
                    "lti nrps: {} context(s) due (interval {}h)",
                    due.len(),
                    nrps_interval_hours,
                );
                for ctx in due {
                    match crate::lti_nrps::reconcile_context(&state, &ctx).await {
                        Ok(outcome) => {
                            tracing::info!(
                                "lti nrps: context {} (course {}): {} added, {} removed",
                                ctx.id,
                                ctx.course_id,
                                outcome.added,
                                outcome.removed,
                            );
                            if let Some(w) = outcome.warning.as_deref() {
                                tracing::warn!(
                                    "lti nrps: context {} (course {}) warning: {}",
                                    ctx.id,
                                    ctx.course_id,
                                    w
                                );
                            }
                            if let Err(e) = minerva_db::queries::lti_nrps::record_sync_result(
                                &state.db,
                                ctx.id,
                                "ok",
                                None,
                                outcome.warning.as_deref(),
                                Some(outcome.added),
                                Some(outcome.removed),
                            )
                            .await
                            {
                                tracing::error!(
                                    "lti nrps: failed to record sync result for {}: {}",
                                    ctx.id,
                                    e
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "lti nrps: context {} (course {}) failed: {}",
                                ctx.id,
                                ctx.course_id,
                                e,
                            );
                            let _ = minerva_db::queries::lti_nrps::record_sync_result(
                                &state.db,
                                ctx.id,
                                "error",
                                Some(&e.to_string()),
                                None,
                                None,
                                None,
                            )
                            .await;
                        }
                    }
                }
            }
        });
    }

    // Periodic cleanup of unapproved (dynreg-installed) platforms. Anyone
    // can hit `/lti/dynamic-register` so pending rows could otherwise pile
    // up indefinitely. After 7 days of no approval, drop them; the admin
    // either intended to approve and lost track (in which case the LMS
    // admin can re-run dynreg), or never intended to (spam / mistake).
    {
        let state = state.clone();
        tokio::spawn(async move {
            const SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60); // hourly
            const MAX_AGE_HOURS: i32 = 24 * 7;
            loop {
                tokio::time::sleep(SWEEP_INTERVAL).await;
                match minerva_db::queries::lti::delete_stale_pending_platforms(
                    &state.db,
                    MAX_AGE_HOURS,
                )
                .await
                {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(
                        "lti dynreg: dropped {} stale pending platform row(s) older than {}h",
                        n,
                        MAX_AGE_HOURS
                    ),
                    Err(e) => tracing::error!("lti dynreg: stale pending sweep failed: {}", e),
                }
            }
        });
    }

    // Platform-health probe: every active platform's token endpoint is
    // pinged ~daily with a throwaway client_credentials JWT. If the LMS
    // rejects with `invalid_client` continuously for 30 days, the row
    // is cascade-deleted (bindings + NRPS contexts go with it via FK).
    // This is how we detect "the LMS admin deleted us"; the spec
    // doesn't notify the tool, so we have to ask.
    //
    // Ticks every SCHEDULE_TICK; `find_platforms_due_for_health_check`
    // returns only platforms whose last probe is older than
    // `PLATFORM_HEALTH_PROBE_INTERVAL_HOURS`. Previously this loop
    // slept 24 h between probes, meaning a pod that restarted more
    // often than that NEVER probed and the 30-day orphan clock never
    // started. The short-tick + DB-due query makes restarts safe.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut ticker = schedule_ticker();
            loop {
                ticker.tick().await;
                let due = match minerva_db::queries::lti::find_platforms_due_for_health_check(
                    &state.db,
                    PLATFORM_HEALTH_PROBE_INTERVAL_HOURS,
                )
                .await
                {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("lti health: due query failed: {}", e);
                        continue;
                    }
                };
                for p in &due {
                    let status = crate::lti_nrps::probe_platform_health(&state, p).await;
                    if let Err(e) =
                        minerva_db::queries::lti::record_platform_health(&state.db, p.id, &status)
                            .await
                    {
                        tracing::error!(
                            "lti health: failed to record probe for platform {}: {}",
                            p.id,
                            e
                        );
                        continue;
                    }
                    if status != "ok" {
                        tracing::warn!(
                            "lti health: platform {} ({}) probe -> {}",
                            p.id,
                            p.issuer,
                            status
                        );
                    }
                }
                // Cheap when no platforms qualify (indexed on
                // `invalid_client_since`); fine to run on every tick.
                match minerva_db::queries::lti::delete_long_orphaned_platforms(
                    &state.db,
                    PLATFORM_ORPHAN_GRACE_DAYS,
                )
                .await
                {
                    Ok(0) => {}
                    Ok(n) => tracing::warn!(
                        "lti health: cascade-deleted {} platform row(s) the LMS has been rejecting for {}+ days",
                        n,
                        PLATFORM_ORPHAN_GRACE_DAYS
                    ),
                    Err(e) => tracing::error!("lti health: orphan delete failed: {}", e),
                }
            }
        });
    }
}
