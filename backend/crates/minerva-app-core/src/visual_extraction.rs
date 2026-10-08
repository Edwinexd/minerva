//! Visual extraction pipeline: constants and the signed URLs that are the
//! only credential a Slurm worker ever holds. See "Visual extraction
//! pipeline" in docs/ARCHITECTURE.md.
//!
//! A grant is `base64url(json).base64url(hmac)` over a domain-separated
//! payload, so it can never be replayed as one of the other tokens signed
//! with `MINERVA_HMAC_SECRET`. Three scopes:
//!
//! - `worker`: names a worker row; lets a Slurm job ask for its next item
//!   until the job's wall time is up. Revoked by the row leaving an active
//!   state.
//! - `source` / `result`: one job attempt's download and upload. The attempt
//!   number is signed in, so a stale attempt can neither fetch nor land.
//! - `figure`: one figure image shown with a chat reply. Minted when a reply
//!   is served to someone allowed to read it, so the image loads in the app
//!   and in the cookieless embed iframe alike.

use std::collections::HashMap;
use std::process::Stdio;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::config::SlurmConfig;
use crate::state::AppState;
use minerva_db::queries::visual_extraction as queue;

/// Bump when the OCR model, its prompt or the slide thresholds change; done
/// jobs from an older version are re-run a few at a time.
pub const PIPELINE_VERSION: i32 = 1;
/// Attempts before a job is parked as `failed`.
pub const MAX_ATTEMPTS: i32 = 3;
/// How long one leased item may take before it goes back to the queue. A
/// 150-minute lecture or a 300-page PDF fits comfortably.
pub const LEASE_SECS: i64 = 2 * 3600;
/// Slurm wall time of one worker job, and how long it may wait queued on
/// top of that before its grant expires. The service account runs at low
/// priority, so a worker can sit in the queue for a long time.
pub const WORKER_WALL_SECS: i64 = 12 * 3600;
pub const WORKER_QUEUE_GRACE_SECS: i64 = 3 * 24 * 3600;
/// Size a fetch slot reserves until the real video size is known; a long
/// 1080p lecture.
pub const FETCH_ESTIMATE_BYTES: i64 = 400 * 1024 * 1024;
/// A reserved slot whose upload never arrived is released after this.
pub const FETCH_RESERVATION_SECS: i64 = 2 * 3600;
/// Lifetime of a figure image URL handed out with a reply.
pub const FIGURE_URL_SECS: i64 = 24 * 3600;
/// Material uploaded within this many days is processed before the backlog.
pub const RECENT_DAYS: i32 = 14;
/// Outdated done jobs re-enqueued per tick after a version bump.
const REENQUEUE_PER_TICK: i64 = 5;
/// Figures pushed to Qdrant per tick.
const INDEX_PER_TICK: i64 = 200;
/// A submit that never got a Slurm job id, or a job sacct no longer knows,
/// is written off after this.
const SUBMIT_GRACE_SECS: i64 = 10 * 60;
const SSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
/// Slurm states in which a job may still run (or is running). Anything
/// else means the worker is gone.
const SLURM_LIVE_STATES: &[&str] = &[
    "PENDING",
    "CONFIGURING",
    "RUNNING",
    "COMPLETING",
    "REQUEUED",
    "RESIZING",
    "SUSPENDED",
];

const DOMAIN: &[u8] = b"minerva-visual-extraction.v1.";

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Worker,
    Source,
    Result,
    Figure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub scope: Scope,
    /// Worker id for `Worker`, job id otherwise.
    pub id: Uuid,
    /// Job attempt for `Source` / `Result`; 0 for `Worker`.
    pub attempt: i32,
    /// Unix seconds.
    pub exp: i64,
}

/// Path (under `/api`) of a figure image, signed for `FIGURE_URL_SECS`.
pub fn figure_url(secret: &str, figure_id: Uuid) -> String {
    let expires_at = Utc::now() + chrono::Duration::seconds(FIGURE_URL_SECS);
    format!(
        "/api/embed/figures/{}",
        sign(secret, &Grant::new(Scope::Figure, figure_id, 0, expires_at))
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum GrantError {
    Malformed,
    BadSignature,
    WrongScope,
    Expired,
}

impl Grant {
    pub fn new(scope: Scope, id: Uuid, attempt: i32, expires_at: DateTime<Utc>) -> Self {
        Self {
            scope,
            id,
            attempt,
            exp: expires_at.timestamp(),
        }
    }
}

fn mac(secret: &str, payload: &str) -> HmacSha256 {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(DOMAIN);
    mac.update(payload.as_bytes());
    mac
}

pub fn sign(secret: &str, grant: &Grant) -> String {
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(grant).expect("grant serialises"));
    let signature = URL_SAFE_NO_PAD.encode(mac(secret, &payload).finalize().into_bytes());
    format!("{payload}.{signature}")
}

/// Check the signature (constant time), the scope and the expiry.
pub fn verify(secret: &str, token: &str, scope: Scope) -> Result<Grant, GrantError> {
    let (payload, signature) = token.split_once('.').ok_or(GrantError::Malformed)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| GrantError::Malformed)?;
    mac(secret, payload)
        .verify_slice(&signature)
        .map_err(|_| GrantError::BadSignature)?;
    let json = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| GrantError::Malformed)?;
    let grant: Grant = serde_json::from_slice(&json).map_err(|_| GrantError::Malformed)?;
    if grant.scope != scope {
        return Err(GrantError::WrongScope);
    }
    if grant.exp <= Utc::now().timestamp() {
        return Err(GrantError::Expired);
    }
    Ok(grant)
}

/// One scheduler pass, every minute from `minerva-scheduler`: keep the
/// queue honest, keep Olympus fed, push new figures to Qdrant, and publish
/// gauges. Each step logs its own failure and the next still runs.
pub async fn tick(state: &AppState) {
    if let Err(e) = maintain_queue(state).await {
        tracing::error!("visual extraction: queue upkeep failed: {e}");
    }
    if let Some(slurm) = &state.config.visual_extraction.slurm {
        if let Err(e) = reconcile_workers(state, slurm).await {
            tracing::error!("visual extraction: worker reconcile failed: {e}");
        }
    }
    if let Err(e) = index_pending_figures(state).await {
        tracing::error!("visual extraction: figure indexing failed: {e}");
    }
    if let Err(e) = record_metrics(state).await {
        tracing::warn!("visual extraction: metrics failed: {e}");
    }
}

async fn remove_staged(state: &AppState, staged_paths: &[String]) {
    for staged in staged_paths {
        let path = format!("{}/{staged}", state.config.visual_extraction.staging_path);
        let _ = tokio::fs::remove_file(path).await;
    }
}

async fn maintain_queue(state: &AppState) -> Result<(), sqlx::Error> {
    let created = queue::enqueue_missing(&state.db, PIPELINE_VERSION, RECENT_DAYS).await?;
    if created > 0 {
        tracing::info!("visual extraction: enqueued {created} new jobs");
    }
    queue::release_stale_fetches(&state.db, FETCH_RESERVATION_SECS).await?;
    let terminal = queue::requeue_expired(&state.db, MAX_ATTEMPTS).await?;
    remove_staged(state, &terminal).await;
    queue::reenqueue_outdated(&state.db, PIPELINE_VERSION, REENQUEUE_PER_TICK).await?;
    Ok(())
}

/// Path of the helper on Olympus, relative to the service account's home
/// (deployed there by the deploy-slide-ocr workflow).
const GATE: &str = "minerva-slide-ocr/olympus/slide-ocr-gate";

/// Run one `submit` / `status` / `cancel` through the helper script on
/// Olympus. `stdin` carries secrets, never the arguments, which other
/// cluster users could see in process listings.
async fn gate(slurm: &SlurmConfig, command: &str, stdin: Option<&str>) -> Result<String, String> {
    let known_hosts = format!("UserKnownHostsFile={}", slurm.known_hosts_path);
    let remote = format!("{GATE} {command}");
    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-i",
            &slurm.ssh_key_path,
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            &known_hosts,
            "-o",
            "ConnectTimeout=15",
            &slurm.ssh_target,
            &remote,
        ])
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("ssh spawn: {e}"))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(input.as_bytes())
            .await
            .map_err(|e| format!("ssh stdin: {e}"))?;
    }
    let output = tokio::time::timeout(SSH_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("ssh {command}: timed out"))?
        .map_err(|e| format!("ssh {command}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ssh {command}: {} {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `JobID|State` lines from `sacct -X -n -P -o JobID,State`. A state
/// can carry a suffix ("CANCELLED by 123"); only the first word counts.
fn parse_sacct(output: &str) -> HashMap<i64, String> {
    output
        .lines()
        .filter_map(|line| {
            let (id, state) = line.trim().split_once('|')?;
            let state = state.split_whitespace().next()?;
            Some((id.parse().ok()?, state.to_string()))
        })
        .collect()
}

/// Settle every worker Minerva thinks is alive against what Slurm says,
/// return work held by dead ones, then submit one more worker if there is
/// leasable work and a free slot.
async fn reconcile_workers(state: &AppState, slurm: &SlurmConfig) -> Result<(), String> {
    let db_err = |e: sqlx::Error| e.to_string();
    let active = queue::list_active_workers(&state.db)
        .await
        .map_err(db_err)?;
    let job_ids: Vec<String> = active
        .iter()
        .filter_map(|w| w.slurm_job_id)
        .map(|id| id.to_string())
        .collect();
    let slurm_states = if job_ids.is_empty() {
        HashMap::new()
    } else {
        parse_sacct(&gate(slurm, &format!("status {}", job_ids.join(",")), None).await?)
    };

    let now = Utc::now();
    let mut alive = 0;
    for worker in &active {
        let past_grace = (now - worker.submitted_at).num_seconds() > SUBMIT_GRACE_SECS;
        let verdict = match (
            worker.slurm_job_id,
            worker.slurm_job_id.and_then(|id| slurm_states.get(&id)),
        ) {
            (_, Some(s)) if SLURM_LIVE_STATES.contains(&s.as_str()) => None,
            (_, Some(s)) if s == "COMPLETED" => Some(("finished", format!("slurm job {s}"))),
            (_, Some(s)) => Some(("failed", format!("slurm job {s}"))),
            (Some(_), None) if past_grace => {
                Some(("failed", "slurm job unknown to sacct".to_string()))
            }
            (None, None) if past_grace => Some(("failed", "submit never completed".to_string())),
            _ => None,
        };
        let verdict = match verdict {
            None if worker.expires_at < now => {
                if let Some(id) = worker.slurm_job_id {
                    let _ = gate(slurm, &format!("cancel {id}"), None).await;
                }
                Some(("failed", "worker grant expired".to_string()))
            }
            other => other,
        };
        match verdict {
            Some((worker_state, reason)) => {
                let terminal = queue::requeue_worker(&state.db, worker.id, &reason, MAX_ATTEMPTS)
                    .await
                    .map_err(db_err)?;
                remove_staged(state, &terminal).await;
                let error = (worker_state == "failed").then_some(reason.as_str());
                queue::set_worker_state(&state.db, worker.id, worker_state, error)
                    .await
                    .map_err(db_err)?;
            }
            None => alive += 1,
        }
    }

    if alive >= slurm.max_workers || queue::count_leasable(&state.db).await.map_err(db_err)? == 0 {
        return Ok(());
    }
    submit_worker(state, slurm).await
}

async fn submit_worker(state: &AppState, slurm: &SlurmConfig) -> Result<(), String> {
    let worker_id = Uuid::new_v4();
    let expires_at =
        Utc::now() + chrono::Duration::seconds(WORKER_WALL_SECS + WORKER_QUEUE_GRACE_SECS);
    queue::insert_worker(&state.db, worker_id, expires_at)
        .await
        .map_err(|e| e.to_string())?;
    let token = sign(
        &state.config.hmac_secret,
        &Grant::new(Scope::Worker, worker_id, 0, expires_at),
    );
    let url = format!(
        "{}/api/service/visual-extraction/workers/{token}",
        state.config.base_url
    );

    match gate(slurm, &format!("submit {worker_id}"), Some(&url)).await {
        Ok(output) => {
            let job_id: i64 = output
                .trim()
                .split(';')
                .next()
                .and_then(|id| id.parse().ok())
                .ok_or_else(|| format!("sbatch printed no job id: {output}"))?;
            queue::set_worker_slurm_job(&state.db, worker_id, job_id)
                .await
                .map_err(|e| e.to_string())?;
            metrics::counter!("visual_extraction_worker_submits_total").increment(1);
            tracing::info!("visual extraction: submitted worker {worker_id} as slurm job {job_id}");
            Ok(())
        }
        Err(e) => {
            let _ = queue::set_worker_state(&state.db, worker_id, "failed", Some(&e)).await;
            Err(e)
        }
    }
}

/// Push figures whose vectors are not in Qdrant yet, course by course.
async fn index_pending_figures(state: &AppState) -> Result<(), String> {
    let pending = queue::list_unindexed_figures(&state.db, INDEX_PER_TICK)
        .await
        .map_err(|e| e.to_string())?;
    let mut by_course: HashMap<Uuid, Vec<queue::UnindexedFigure>> = HashMap::new();
    for figure in pending {
        by_course.entry(figure.course_id).or_default().push(figure);
    }
    for (course_id, course_figures) in by_course {
        let Some(course) = queue::course_embedding(&state.db, course_id)
            .await
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        let ids: Vec<Uuid> = course_figures.iter().map(|f| f.id).collect();
        let to_index: Vec<minerva_pipeline::figures::FigureToIndex> = course_figures
            .into_iter()
            .map(|f| minerva_pipeline::figures::FigureToIndex {
                id: f.id,
                document_id: f.document_id,
                context: f.context,
                visual_vector: f.visual_vector,
            })
            .collect();
        let embedding = minerva_pipeline::figures::CourseEmbedding {
            course_id,
            provider: &course.embedding_provider,
            model: &course.embedding_model,
            version: course.embedding_version,
        };
        match minerva_pipeline::figures::index_figures(
            &state.qdrant,
            state.fastembed.as_ref(),
            &state.http_client,
            &state.config.openai_api_key,
            &embedding,
            &to_index,
        )
        .await
        {
            Ok(()) => queue::mark_figures_indexed(&state.db, &ids)
                .await
                .map_err(|e| e.to_string())?,
            Err(e) => tracing::warn!(
                "visual extraction: indexing figures for course {course_id} failed: {e}"
            ),
        }
    }
    Ok(())
}

async fn record_metrics(state: &AppState) -> Result<(), sqlx::Error> {
    const STATUSES: &[&str] = &[
        "needs_source",
        "fetching",
        "ready",
        "leased",
        "done",
        "failed",
        "unavailable",
    ];
    let counts: HashMap<String, i64> = queue::count_by_status(&state.db)
        .await?
        .into_iter()
        .collect();
    for status in STATUSES {
        metrics::gauge!("visual_extraction_jobs", "status" => *status)
            .set(counts.get(*status).copied().unwrap_or(0) as f64);
    }
    let workers = queue::list_active_workers(&state.db).await?.len();
    metrics::gauge!("visual_extraction_workers_active").set(workers as f64);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    const SECRET: &str = "test-secret";

    fn grant(scope: Scope, offset: Duration) -> Grant {
        Grant::new(scope, Uuid::new_v4(), 2, Utc::now() + offset)
    }

    #[test]
    fn round_trips() {
        let g = grant(Scope::Result, Duration::hours(1));
        assert_eq!(verify(SECRET, &sign(SECRET, &g), Scope::Result), Ok(g));
    }

    #[test]
    fn rejects_other_scope() {
        let token = sign(SECRET, &grant(Scope::Source, Duration::hours(1)));
        assert_eq!(
            verify(SECRET, &token, Scope::Result),
            Err(GrantError::WrongScope)
        );
    }

    #[test]
    fn rejects_expired() {
        let token = sign(SECRET, &grant(Scope::Worker, Duration::seconds(-1)));
        assert_eq!(
            verify(SECRET, &token, Scope::Worker),
            Err(GrantError::Expired)
        );
    }

    #[test]
    fn rejects_tampered_payload() {
        let token = sign(SECRET, &grant(Scope::Source, Duration::hours(1)));
        let (_, signature) = token.split_once('.').unwrap();
        let forged = Grant {
            attempt: 3,
            ..grant(Scope::Source, Duration::hours(1))
        };
        let forged_payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
        assert_eq!(
            verify(
                SECRET,
                &format!("{forged_payload}.{signature}"),
                Scope::Source
            ),
            Err(GrantError::BadSignature)
        );
    }

    #[test]
    fn sacct_lines_parse_to_first_state_word() {
        let parsed =
            parse_sacct("22875|RUNNING\n22876|CANCELLED by 45423\nnoise\n22877|COMPLETED\n");
        assert_eq!(parsed.get(&22875).map(String::as_str), Some("RUNNING"));
        assert_eq!(parsed.get(&22876).map(String::as_str), Some("CANCELLED"));
        assert_eq!(parsed.get(&22877).map(String::as_str), Some("COMPLETED"));
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn rejects_other_secret() {
        let token = sign("other", &grant(Scope::Worker, Duration::hours(1)));
        assert_eq!(
            verify(SECRET, &token, Scope::Worker),
            Err(GrantError::BadSignature)
        );
    }
}
