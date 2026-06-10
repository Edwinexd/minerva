//! Service API for automated pipelines (e.g. transcript fetcher).
//!
//! Authenticated via `Authorization: Bearer <key>` where the key matches
//! the `MINERVA_SERVICE_API_KEY` environment variable. This is a global
//! key, not scoped to any course.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

/// Body cap on `/api/service/daisy-courses`. Each course carries
/// nested participants (name + 1-2 eppns + free-text role list), so
/// row size is ~1.5 KB rather than the catalog endpoint's ~50 B per
/// entry. At today's DSV scale a single semester batch is ~250 KB;
/// the 20 MB ceiling leaves room for an order-of-magnitude growth
/// (cross-department sync, larger course rosters) before the
/// chunker in `scripts/sync_daisy_courses.py` would need to drop
/// its batch size. The script still chunks at 25 courses per POST
/// to bound backend memory + per-request handler latency regardless.
const DAISY_IMPORT_MAX_BYTES: usize = 20 * 1_000_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/pending-transcripts", get(pending_transcripts))
        .route(
            "/documents/{document_id}/transcript",
            post(submit_transcript),
        )
        .route("/play-designations", get(list_play_designations))
        .route(
            "/play-designations/{designation_id}/mark-synced",
            post(mark_designation_synced),
        )
        .route(
            "/courses/{course_id}/documents/url",
            post(create_url_document),
        )
        .route("/play-courses", put(replace_play_course_catalog))
        .route(
            "/daisy-courses",
            post(import_daisy_courses)
                .layer(axum::extract::DefaultBodyLimit::max(DAISY_IMPORT_MAX_BYTES)),
        )
}

/// Authenticate using the global service API key (MINERVA_SERVICE_API_KEY).
fn authenticate_service(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let configured_key = state
        .config
        .service_api_key
        .as_deref()
        .ok_or(AppError::Unauthorized)?;

    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(AppError::Unauthorized)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(AppError::Unauthorized)?;

    if token != configured_key {
        return Err(AppError::Unauthorized);
    }
    Ok(())
}

#[derive(Serialize)]
struct PendingTranscriptInfo {
    id: Uuid,
    course_id: Uuid,
    filename: String,
    url: String,
    /// Last component of the cursor key. The script echoes this back
    /// as `after_created_at` on the next page request, paired with
    /// `id` as `after_id`. RFC3339 / ISO 8601 via serde.
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct PendingTranscriptsQuery {
    /// Page size cap. Defaults to 512, clamped to 1024. The python
    /// caller picks 512 to bound in-process memory; admins running
    /// the script by hand can crank it higher.
    limit: Option<i64>,
    /// First half of the cursor: `created_at` of the last item from
    /// the previous page. RFC3339. Pair with `after_id`.
    after_created_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Second half of the cursor: `id` of the last item from the
    /// previous page. UUID. Tiebreaks ties on `created_at` when
    /// several docs were inserted by the same statement.
    after_id: Option<Uuid>,
}

const PENDING_TRANSCRIPTS_DEFAULT_LIMIT: i64 = 512;
const PENDING_TRANSCRIPTS_MAX_LIMIT: i64 = 1024;

/// List URL documents that are waiting for external transcript processing.
/// Returns the URL content from each `.url` file so the caller knows
/// what to fetch. Cursor-paginated; ordered by `(created_at, id)` ASC.
/// To drain everything, the caller loops:
///   1. GET `/pending-transcripts?limit=512` (no cursor)
///   2. process all returned items; remember the last item's
///      `(created_at, id)`.
///   3. GET `/pending-transcripts?limit=512&after_created_at=...&after_id=...`
///   4. repeat until the response is empty.
///
/// Items that fail to process during a run (e.g. Play hasn't finished
/// processing the recording yet) stay in `awaiting_transcript` status
/// and so re-appear on the *next* hourly cron, but never re-appear
/// within the same run; the cursor moves strictly forward.
async fn pending_transcripts(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<PendingTranscriptsQuery>,
) -> Result<Json<Vec<PendingTranscriptInfo>>, AppError> {
    authenticate_service(&state, &headers)?;

    // Clamp the limit. A caller asking for `limit=0` would otherwise
    // get an empty response and infinite-loop in the script; a
    // negative limit is a postgres error. We coerce to the default
    // for both pathological cases.
    let limit = q
        .limit
        .filter(|n| *n > 0)
        .unwrap_or(PENDING_TRANSCRIPTS_DEFAULT_LIMIT)
        .min(PENDING_TRANSCRIPTS_MAX_LIMIT);

    // Cursor: both halves required together, or neither. A half-
    // specified cursor is a client bug and we reject it loudly rather
    // than silently treating it as "first page".
    let after = match (q.after_created_at, q.after_id) {
        (Some(t), Some(i)) => Some((t, i)),
        (None, None) => None,
        _ => return Err(AppError::bad_request("service.cursor_half_specified")),
    };

    let docs =
        minerva_db::queries::documents::list_awaiting_transcripts_page(&state.db, after, limit)
            .await?;
    let mut result = Vec::new();

    for doc in docs {
        let ext = super::documents::extension_from_filename(&doc.filename);
        let file_path = format!(
            "{}/{}/{}.{}",
            state.config.docs_path, doc.course_id, doc.id, ext
        );
        let url = match tokio::fs::read_to_string(&file_path).await {
            Ok(content) => content.trim().to_string(),
            Err(_) => continue,
        };
        result.push(PendingTranscriptInfo {
            id: doc.id,
            course_id: doc.course_id,
            filename: doc.filename,
            url,
            created_at: doc.created_at,
        });
    }

    Ok(Json(result))
}

#[derive(Deserialize)]
struct SubmitTranscriptRequest {
    /// Transcript text content. If provided, the document is re-queued for ingestion.
    text: Option<String>,
    /// Error message. If provided (and text is absent), the document is marked as failed.
    error: Option<String>,
}

/// Submit a transcript for a URL document, or report that no transcript is available.
async fn submit_transcript(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(document_id): Path<Uuid>,
    Json(body): Json<SubmitTranscriptRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;

    let doc = minerva_db::queries::documents::find_by_id(&state.db, document_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if doc.status != "awaiting_transcript" {
        return Err(AppError::bad_request_with(
            "service.wrong_status",
            [("status", doc.status.clone())],
        ));
    }

    if let Some(text) = &body.text {
        if text.is_empty() {
            return Err(AppError::bad_request("service.transcript_empty"));
        }

        // Materialize the transcript as a child of the URL doc. The
        // classifier never sees filenames; it decides lecture_transcript
        // vs lecture from the actual content (a VTT transcript is
        // recognisable by its disfluencies and lack of structure). So
        // we just drop the `.url` suffix and append `.txt` without
        // injecting any marker token.
        let child_filename = doc
            .filename
            .strip_suffix(".url")
            .unwrap_or(&doc.filename)
            .to_string()
            + ".txt";
        let size_bytes = text.len() as i64;
        let content_hash = super::documents::compute_content_hash(text.as_bytes());

        // Write file under the child's id so the parent URL stub stays
        // intact on disk. If the DB transaction below fails we clean
        // up the orphaned file before returning.
        let child_id = Uuid::new_v4();
        let dir = format!("{}/{}", state.config.docs_path, doc.course_id);
        let txt_path = format!("{}/{}.txt", dir, child_id);
        tokio::fs::write(&txt_path, text.as_bytes())
            .await
            .map_err(|e| AppError::Internal(format!("failed to write transcript: {}", e)))?;

        let result = minerva_db::queries::documents::insert_tracked_child(
            &state.db,
            doc.id,
            "awaiting_transcript",
            minerva_db::queries::documents::NewDocument {
                id: child_id,
                course_id: doc.course_id,
                filename: &child_filename,
                mime_type: "text/plain",
                size_bytes,
                uploaded_by: doc.uploaded_by,
                // URL identity lives on the parent only; the per-course
                // `source_url` unique index would otherwise collide.
                // Consumers follow `parent_document_id` to recover the URL.
                source_url: None,
                content_hash: Some(&content_hash),
                source_system: None,
                source_ref: None,
                parent_document_id: Some(doc.id),
            },
        )
        .await;

        match result {
            Ok(_) => {}
            Err(sqlx::Error::RowNotFound) => {
                let _ = tokio::fs::remove_file(&txt_path).await;
                return Err(AppError::bad_request("service.status_changed_concurrently"));
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&txt_path).await;
                return Err(e.into());
            }
        }

        tracing::info!(
            "transcript submitted for url doc {} ({} bytes); materialized as child {} ({}), parent now tracked",
            doc.id,
            size_bytes,
            child_id,
            child_filename,
        );

        Ok(Json(serde_json::json!({
            "status": "queued",
            "child_id": child_id,
            "filename": child_filename,
        })))
    } else if let Some(error) = &body.error {
        // Mark as failed so we don't retry.
        let _ = sqlx::query!(
            "UPDATE documents SET status = 'failed', error_msg = $1 WHERE id = $2",
            error,
            doc.id,
        )
        .execute(&state.db)
        .await;

        tracing::info!("document {} marked as failed: {}", doc.id, error);

        Ok(Json(
            serde_json::json!({ "status": "failed", "error": error }),
        ))
    } else {
        Err(AppError::bad_request("service.missing_text_or_error"))
    }
}

//; Play designations (discovery) --

#[derive(Serialize)]
struct PlayDesignationServiceInfo {
    id: Uuid,
    course_id: Uuid,
    designation: String,
    last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// List all configured play.dsv.su.se designations across all courses.
/// Used by the transcript pipeline to discover new presentations.
async fn list_play_designations(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PlayDesignationServiceInfo>>, AppError> {
    authenticate_service(&state, &headers)?;

    let rows = minerva_db::queries::play_designations::list_all(&state.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| PlayDesignationServiceInfo {
                id: r.id,
                course_id: r.course_id,
                designation: r.designation,
                last_synced_at: r.last_synced_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct MarkSyncedRequest {
    /// Optional error message. If absent, sync is marked as successful.
    error: Option<String>,
}

async fn mark_designation_synced(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(designation_id): Path<Uuid>,
    Json(body): Json<MarkSyncedRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;

    let existing = minerva_db::queries::play_designations::find_by_id(&state.db, designation_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if let Some(err) = &body.error {
        minerva_db::queries::play_designations::mark_synced_error(&state.db, existing.id, err)
            .await?;
        Ok(Json(serde_json::json!({ "status": "error", "error": err })))
    } else {
        minerva_db::queries::play_designations::mark_synced_ok(&state.db, existing.id).await?;
        Ok(Json(serde_json::json!({ "status": "ok" })))
    }
}

#[derive(Deserialize)]
struct CreateUrlDocumentRequest {
    /// URL to index (e.g. `https://play.dsv.su.se/presentation/{uuid}`).
    url: String,
    /// Human-readable filename (without `.url` suffix required).
    filename: String,
}

#[derive(Serialize)]
struct CreateUrlDocumentResponse {
    id: Uuid,
    course_id: Uuid,
    filename: String,
    status: String,
    created: bool,
}

/// Sanitize a filename: strip path separators, disallow `..`, trim whitespace,
/// and cap length. Ensures `.url` suffix.
fn sanitize_url_filename(raw: &str) -> Result<String, AppError> {
    let mut name: String = raw
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0'))
        .collect::<String>()
        .trim()
        .to_string();

    if name.is_empty() || name == "." || name == ".." {
        return Err(AppError::bad_request("service.filename_empty"));
    }

    // Cap at 200 chars before the .url suffix.
    if !name.ends_with(".url") {
        if name.len() > 200 {
            name.truncate(200);
        }
        name.push_str(".url");
    } else if name.len() > 204 {
        name.truncate(200);
        name.push_str(".url");
    }

    Ok(name)
}

/// Idempotently create a URL document in a course.
///
/// Dedup key is the `source_url` column (enforced atomically by a partial
/// unique index on `(course_id, source_url)`). If a document with the same
/// origin URL already exists; regardless of its current status or mime_type
/// (a successful transcript fetch rewrites mime_type to text/plain); return
/// it with `created=false`.
async fn create_url_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(course_id): Path<Uuid>,
    Json(body): Json<CreateUrlDocumentRequest>,
) -> Result<Json<CreateUrlDocumentResponse>, AppError> {
    authenticate_service(&state, &headers)?;

    let url = body.url.trim().to_string();
    if url.is_empty() || url.len() > 2048 {
        return Err(AppError::bad_request("service.url_invalid_length"));
    }

    let filename = sanitize_url_filename(&body.filename)?;

    let course = minerva_db::queries::courses::find_by_id(&state.db, course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Fast path: already tracked.
    if let Some(doc) =
        minerva_db::queries::documents::find_by_course_source_url(&state.db, course_id, &url)
            .await?
    {
        return Ok(Json(CreateUrlDocumentResponse {
            id: doc.id,
            course_id: doc.course_id,
            filename: doc.filename,
            status: doc.status,
            created: false,
        }));
    }

    // Create new URL document.
    let doc_id = Uuid::new_v4();
    let dir = format!("{}/{}", state.config.docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("failed to create directory: {}", e)))?;

    let file_path = format!("{}/{}.url", dir, doc_id);
    tokio::fs::write(&file_path, url.as_bytes())
        .await
        .map_err(|e| AppError::Internal(format!("failed to write url file: {}", e)))?;

    let size_bytes = url.len() as i64;
    // content_hash of the URL bytes for cross-system dedup: two different
    // discovery paths landing on the same play.dsv.su.se URL now collapse
    // even if one of them omitted `source_url` (none currently do, but the
    // hash is cheap insurance). The URL doc stays `text/x-url` for its
    // entire lifetime; the materialized transcript is a separate child
    // doc (see `submit_transcript`), so the hash here is permanently the
    // hash of the URL string, never re-targeted.
    let content_hash = super::documents::compute_content_hash(url.as_bytes());
    let insert_result = minerva_db::queries::documents::insert(
        &state.db,
        minerva_db::queries::documents::NewDocument {
            id: doc_id,
            course_id,
            filename: &filename,
            mime_type: "text/x-url",
            size_bytes,
            uploaded_by: course.owner_id,
            source_url: Some(&url),
            content_hash: Some(&content_hash),
            source_system: None,
            source_ref: None,
            // The URL stub is itself a first-class doc; it's the *parent*
            // for whatever the ingest pipeline materializes from it.
            parent_document_id: None,
        },
    )
    .await;

    let row = match insert_result {
        Ok(row) => row,
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            // Concurrent creator won the race. Clean up our orphan file and
            // return the winner.
            let _ = tokio::fs::remove_file(&file_path).await;
            let existing = minerva_db::queries::documents::find_by_course_source_url(
                &state.db, course_id, &url,
            )
            .await?
            .ok_or_else(|| {
                AppError::Internal(
                    "unique violation on source_url but no matching row found".into(),
                )
            })?;
            return Ok(Json(CreateUrlDocumentResponse {
                id: existing.id,
                course_id: existing.course_id,
                filename: existing.filename,
                status: existing.status,
                created: false,
            }));
        }
        Err(e) => return Err(e.into()),
    };

    tracing::info!(
        "service created url document {} in course {} ({})",
        row.id,
        course_id,
        url,
    );

    Ok(Json(CreateUrlDocumentResponse {
        id: row.id,
        course_id: row.course_id,
        filename: row.filename,
        status: row.status,
        created: true,
    }))
}

//; Play course catalog --

#[derive(Deserialize)]
struct PlayCourseEntry {
    code: String,
    name: String,
}

/// Replace/upsert the cached catalog of play.dsv.su.se course designations.
/// Pushed by the transcript pipeline at the start of each run.
async fn replace_play_course_catalog(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Vec<PlayCourseEntry>>,
) -> Result<Json<serde_json::Value>, AppError> {
    authenticate_service(&state, &headers)?;

    let entries: Vec<(String, String)> = body
        .into_iter()
        .filter_map(|e| {
            let code = e.code.trim().to_string();
            let name = e.name.trim().to_string();
            if code.is_empty() || name.is_empty() {
                None
            } else {
                Some((code, name))
            }
        })
        .collect();

    let n = entries.len();
    let upserted =
        minerva_db::queries::play_course_catalog::upsert_many(&state.db, &entries).await?;

    tracing::info!(
        "play catalog upsert: {} submitted, {} rows touched",
        n,
        upserted
    );
    Ok(Json(
        serde_json::json!({ "submitted": n, "upserted": upserted }),
    ))
}

//; Daisy course auto-import --

/// One staff member from the Daisy momentinfo page, post-username
/// resolution. The python sync script resolves every login a person
/// holds via `daisy.get_staff_details(person_id).usernames` and
/// flattens them into `eppns` (in newest-first order). `kind`
/// distinguishes employed staff from student-handledare so the role
/// mapping below stays predictable.
///
/// `Serialize` is derived (not just `Deserialize`) so the staging
/// path can store the participants list as JSONB and the admin
/// Apply path can round-trip them back into the same struct.
#[derive(Deserialize, Serialize)]
pub(super) struct DaisyParticipantInput {
    /// One or more SU eppns, newest first. The first entry is treated
    /// as the canonical (primary) login; the rest are registered as
    /// `user_eppn_aliases` once we know which Minerva user they map to.
    pub eppns: Vec<String>,
    pub display_name: Option<String>,
    /// Free-text role labels from Daisy. Used here only to decide
    /// `eligible_for_owner`; not persisted on the course row itself.
    pub daisy_roles: Vec<String>,
    /// `"staff"` or `"student"`. Determines the Minerva
    /// `course_members.role` we add: staff -> teacher, student -> ta.
    /// Only `"staff"` is eligible for owner promotion (a student
    /// handledare should never become course owner).
    pub kind: String,
}

#[derive(Deserialize, Serialize)]
pub(super) struct DaisyCourseInputPayload {
    pub momenttillf_id: String,
    pub beteckning: String,
    pub name: String,
    /// Required for both the staging and apply paths. Marked `Option`
    /// for backwards-compat with any client that omits it, but the
    /// handler rejects None with `daisy.course_missing_semester`.
    pub semester_label: Option<String>,
    pub info_url: Option<String>,
    pub syllabus_url: Option<String>,
    pub unit: Option<String>,
    #[serde(default)]
    pub participants: Vec<DaisyParticipantInput>,
}

#[derive(Serialize, Default)]
pub(super) struct DaisyImportSummary {
    pub courses_received: usize,
    /// Auto-apply path: rows written straight to `courses`.
    pub courses_created: usize,
    pub courses_updated: usize,
    pub members_added: usize,
    pub aliases_registered: usize,
    pub designations_created: usize,
    /// Staging path: rows written to `daisy_pending_imports` for
    /// admin review. Set when `daisy_settings.auto_apply` is FALSE.
    pub courses_staged: usize,
    /// Staging path: offerings a re-apply would not change (no new
    /// course, no metadata delta, no member delta), so they are skipped
    /// instead of re-staged. Keeps the review page to genuine changes.
    pub courses_skipped: usize,
    /// TRUE when this request hit the staging path (everything in
    /// `courses_staged` rather than `courses_created`/`updated`).
    /// Lets the python caller distinguish "staged for review" from
    /// "applied". When auto_apply is ON, all counters live in the
    /// applied-side fields above and this stays FALSE.
    pub staged_for_review: bool,
    pub errors: Vec<String>,
}

/// Normalize an inbound eppn the same way `auth_middleware` does so
/// alias / primary lookups line up across the two paths. Lowercased;
/// empty / whitespace-only rejected by the caller.
fn normalize_eppn(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// True when one of the Daisy role labels marks this person as the
/// course-responsible (Swedish: kurs-/delkursansvarig). Only staff
/// (kind == "staff") get promoted to owner; student-handledare are
/// course-listed but never own.
///
/// The literal strings we match against are Daisy data values, not
/// Minerva identifiers; comparison is fully case-insensitive because
/// Daisy occasionally rewords or recases its role headings, and
/// silently losing owner-promotion the next time they do would be a
/// hard-to-diagnose failure mode.
fn is_course_responsible(daisy_roles: &[String]) -> bool {
    daisy_roles.iter().any(|r| {
        let lower = r.trim().to_lowercase();
        lower.starts_with("kurs-/delkursansvarig") || lower == "kursansvarig"
    })
}

/// Map (kind, daisy_roles) onto a Minerva `course_members.role` plus
/// the `eligible_for_owner` flag. Daisy distinguishes employed staff
/// (`/anstalld/anstalldinfo.jspa`) from student-handledare
/// (`/anstalld/student/studentinfo.jspa`); the python script sends
/// that via `kind`. Anything we don't recognise becomes `teacher` so
/// the import doesn't silently drop course members.
fn minerva_role_for(kind: &str, daisy_roles: &[String]) -> (&'static str, bool) {
    match kind {
        "student" => ("ta", false),
        _ => ("teacher", is_course_responsible(daisy_roles)),
    }
}

/// What a re-apply of one Daisy offering would actually change, used to
/// gate staging and to drive the admin review page. The daily sync
/// re-sends every current+next-semester offering, so most rows map to
/// a course that already exists with identical data; without this the
/// review page perpetually lists the whole catalogue as "Update". A
/// row is only worth staging (and showing) when this diff is non-empty.
#[derive(Serialize, Default)]
pub(super) struct OfferingDiff {
    /// No matching course yet; an apply would INSERT one. New offerings
    /// are always worth staging, so this alone makes the diff non-empty.
    pub is_new_course: bool,
    /// Offering-metadata fields a re-apply would overwrite (an Update
    /// only ever refreshes the `course_daisy_offerings` snapshot, never
    /// the live `courses` row, so this compares against that snapshot).
    pub metadata_changes: Vec<FieldChange>,
    /// Participants a re-apply would add as members, or whose course
    /// role it would change. Resolved read-only: a participant whose
    /// eppn matches no user counts as "added" because the apply path
    /// `find_or_create`s the user.
    pub member_changes: Vec<MemberChange>,
}

impl OfferingDiff {
    /// True when applying this offering would change nothing an admin
    /// can see (no new course, no metadata delta, no member delta).
    pub(super) fn is_empty(&self) -> bool {
        !self.is_new_course && self.metadata_changes.is_empty() && self.member_changes.is_empty()
    }
}

#[derive(Serialize)]
pub(super) struct FieldChange {
    /// Stable field key (`name`, `course_code`, `semester_label`,
    /// `info_url`, `syllabus_url`, `unit`); the frontend maps it to a
    /// localized label.
    pub field: &'static str,
    pub old: Option<String>,
    pub new: Option<String>,
}

#[derive(Serialize)]
pub(super) struct MemberChange {
    pub display_name: Option<String>,
    pub primary_eppn: Option<String>,
    /// `"added"` or `"role_changed"`.
    pub change: &'static str,
    /// Resulting Minerva role the apply would set (`teacher` / `ta`).
    pub role: &'static str,
    /// Prior role; set only when `change == "role_changed"`.
    pub previous_role: Option<String>,
}

/// Trim and collapse empty-to-`None` so `Some("")` and `None` (and
/// stray whitespace) never read as a change between Daisy snapshots.
fn norm_opt(v: Option<&str>) -> Option<String> {
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn push_field_change(
    changes: &mut Vec<FieldChange>,
    field: &'static str,
    old: Option<&str>,
    new: Option<&str>,
) {
    let (old, new) = (norm_opt(old), norm_opt(new));
    if old != new {
        changes.push(FieldChange { field, old, new });
    }
}

/// Compute what applying `input` would change, read-only (never creates
/// users or touches `courses`). Shared by `stage_one` (to decide
/// whether to stage at all) and the admin list endpoint (to show the
/// diff and drop rows that converged since they were staged).
pub(super) async fn compute_offering_diff(
    state: &AppState,
    input: &DaisyCourseInputPayload,
    existing_course_id: Option<Uuid>,
) -> Result<OfferingDiff, AppError> {
    let Some(course_id) = existing_course_id else {
        return Ok(OfferingDiff {
            is_new_course: true,
            ..Default::default()
        });
    };

    let momenttillf_id = input.momenttillf_id.trim();
    let mut diff = OfferingDiff::default();

    // Metadata: compare against the snapshot the last apply wrote. The
    // offering row is guaranteed to exist (existing_course_id was
    // resolved via the offerings join), but stay defensive on a race.
    if let Some(o) = minerva_db::queries::course_daisy_offerings::find_by_momenttillf_id(
        &state.db,
        momenttillf_id,
    )
    .await?
    {
        push_field_change(
            &mut diff.metadata_changes,
            "course_code",
            o.course_code.as_deref(),
            Some(input.beteckning.as_str()),
        );
        push_field_change(
            &mut diff.metadata_changes,
            "name",
            o.name.as_deref(),
            Some(input.name.as_str()),
        );
        push_field_change(
            &mut diff.metadata_changes,
            "semester_label",
            o.semester_label.as_deref(),
            input.semester_label.as_deref(),
        );
        push_field_change(
            &mut diff.metadata_changes,
            "info_url",
            o.info_url.as_deref(),
            input.info_url.as_deref(),
        );
        push_field_change(
            &mut diff.metadata_changes,
            "syllabus_url",
            o.syllabus_url.as_deref(),
            input.syllabus_url.as_deref(),
        );
        push_field_change(
            &mut diff.metadata_changes,
            "unit",
            o.unit.as_deref(),
            input.unit.as_deref(),
        );
    }

    // Members: replicate apply_one's resolution read-only. A participant
    // whose eppns resolve to no user would be created+added on apply, so
    // it counts as "added"; an existing user not yet a member is also
    // "added"; a member whose role would flip is "role_changed".
    let members = minerva_db::queries::courses::list_members(&state.db, course_id).await?;
    for participant in &input.participants {
        let eppns: Vec<String> = participant
            .eppns
            .iter()
            .map(|e| normalize_eppn(e))
            .filter(|e| !e.is_empty())
            .collect();
        if eppns.is_empty() {
            continue;
        }
        let (role, _eligible) = minerva_role_for(&participant.kind, &participant.daisy_roles);

        let mut user_id: Option<Uuid> = None;
        for eppn in &eppns {
            if let Some((row, _via_alias)) =
                minerva_db::queries::users::find_by_eppn_or_alias(&state.db, eppn).await?
            {
                user_id = Some(row.id);
                break;
            }
        }

        let change = match user_id.and_then(|uid| members.iter().find(|m| m.user_id == uid)) {
            // Resolved user already a member with this role: no change.
            Some(m) if m.role == role => continue,
            // Resolved user already a member, different role: a flip.
            Some(m) => MemberChange {
                display_name: participant.display_name.clone(),
                primary_eppn: eppns.first().cloned(),
                change: "role_changed",
                role,
                previous_role: Some(m.role.clone()),
            },
            // New member (resolved-but-not-a-member, or no user yet).
            None => MemberChange {
                display_name: participant.display_name.clone(),
                primary_eppn: eppns.first().cloned(),
                change: "added",
                role,
                previous_role: None,
            },
        };
        diff.member_changes.push(change);
    }

    Ok(diff)
}

/// Idempotently bulk-import Daisy course offerings + participants.
///
/// Called daily by `.github/workflows/daisy-sync.yml`
/// (`scripts/sync_daisy_courses.py`). The python side handles all the
/// dsv-wrapper interaction (course search, participants fetch,
/// username resolution via staff profile pages); the backend only
/// reasons about Minerva-side identity and idempotency.
///
/// Per-course flow (when `daisy_settings.auto_apply` is OFF, the
/// default, this stages the row in `daisy_pending_imports` instead
/// and the admin promotes via `/admin/daisy`):
///   1. Resolve every participant to a Minerva user via
///      `users::find_or_create_by_eppn`; secondary eppns become
///      `user_eppn_aliases`. Same pattern the auth middleware uses
///      for never-seen Shib eppns on first login.
///   2. Pick the first kursansvarig-eligible participant as owner.
///      Refuse the course if Daisy listed none (no random fallback
///      user; the staged row stays for admin attention).
///   3. Upsert by `daisy_momenttillf_id` via
///      `courses::upsert_from_daisy` (owner stamped on INSERT only).
///   4. On CREATE: insert a `play_designations` row so transcript
///      discovery picks the course up on the next transcripts.yml run.
///   5. Additively add all resolved participants as course members.
///
/// Errors per course are collected into `summary.errors` and the
/// import continues; a single broken course shouldn't take down the
/// daily sync.
async fn import_daisy_courses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Vec<DaisyCourseInputPayload>>,
) -> Result<Json<DaisyImportSummary>, AppError> {
    authenticate_service(&state, &headers)?;

    // Read the auto-apply toggle once per request. It can in theory
    // flip mid-batch if an admin times their click perfectly, but the
    // sync re-runs daily so a transient mismatch self-heals. Locking
    // it for the whole batch keeps the per-course dispatch simple.
    let auto_apply = minerva_db::queries::daisy_settings::auto_apply_enabled(&state.db).await?;

    let mut summary = DaisyImportSummary {
        courses_received: body.len(),
        staged_for_review: !auto_apply,
        ..Default::default()
    };

    if auto_apply {
        // Only the apply path needs the embedding-model default;
        // staging writes are pure metadata snapshots with no
        // settings stamped until an admin promotes them.
        let default_embedding_model =
            minerva_db::queries::embedding_models::current_default(&state.db).await?;
        let default_reranker_model =
            minerva_db::queries::reranker_models::current_default(&state.db).await?;

        for input in body {
            match apply_one(
                &state,
                &input,
                default_embedding_model.as_deref(),
                default_reranker_model.as_deref(),
                &mut summary,
            )
            .await
            {
                Ok(()) => {}
                Err(e) => {
                    summary
                        .errors
                        .push(format!("{}: {}", input.momenttillf_id, e));
                    tracing::warn!(
                        momenttillf_id = %input.momenttillf_id,
                        error = %e,
                        "daisy apply: per-course failure",
                    );
                }
            }
        }
    } else {
        for input in body {
            match stage_one(&state, &input, &mut summary).await {
                Ok(()) => {}
                Err(e) => {
                    summary
                        .errors
                        .push(format!("{}: {}", input.momenttillf_id, e));
                    tracing::warn!(
                        momenttillf_id = %input.momenttillf_id,
                        error = %e,
                        "daisy stage: per-course failure",
                    );
                }
            }
        }
    }

    tracing::info!(
        "daisy import: received={} staged={} skipped={} created={} updated={} members_added={} aliases={} auto_apply={}",
        summary.courses_received,
        summary.courses_staged,
        summary.courses_skipped,
        summary.courses_created,
        summary.courses_updated,
        summary.members_added,
        summary.aliases_registered,
        auto_apply,
    );
    Ok(Json(summary))
}

/// Write the payload to `daisy_pending_imports` for admin review.
/// Idempotent on `momenttillf_id`: a subsequent sync overwrites the
/// staged metadata + participants snapshot (and updates the linked
/// `existing_course_id` if a manual apply happened between syncs).
/// The actual `courses` table is untouched here; the admin Apply
/// route does that via `apply_one` on the staged row.
async fn stage_one(
    state: &AppState,
    input: &DaisyCourseInputPayload,
    summary: &mut DaisyImportSummary,
) -> Result<(), AppError> {
    let momenttillf_id = input.momenttillf_id.trim();
    let beteckning = input.beteckning.trim();
    let name = input.name.trim();
    let semester_label = input.semester_label.as_deref().unwrap_or("").trim();
    if momenttillf_id.is_empty() || beteckning.is_empty() || name.is_empty() {
        return Err(AppError::bad_request("daisy.course_missing_required"));
    }
    if semester_label.is_empty() {
        return Err(AppError::bad_request("daisy.course_missing_semester"));
    }

    // If a course with this momenttillf_id already exists, surface
    // the link so the admin UI can render "Update" instead of "New".
    let existing_course_id =
        minerva_db::queries::courses::find_by_daisy_momenttillf_id(&state.db, momenttillf_id)
            .await?
            .map(|c| c.id);

    // Gate on a real diff. The daily sync re-sends every current+next
    // semester offering, so the vast majority map to a course that
    // already exists with byte-identical data; staging those would keep
    // the review page perpetually full of no-op "Update" rows. Only
    // stage when applying would actually change something, and drop any
    // stale row a previous sync left for an offering that has since
    // converged (e.g. the admin applied it, or Daisy reverted).
    let diff = compute_offering_diff(state, input, existing_course_id).await?;
    if diff.is_empty() {
        minerva_db::queries::daisy_pending_imports::delete_by_momenttillf_id(
            &state.db,
            momenttillf_id,
        )
        .await?;
        summary.courses_skipped += 1;
        return Ok(());
    }

    // Snapshot the resolved-participant list verbatim. The admin
    // Apply route deserialises it back into a Vec<DaisyParticipantInput>
    // and routes it through apply_one; the round-trip is exercised by
    // the (de)serialise derive pair on both structs.
    let participants_json = serde_json::to_value(&input.participants)
        .map_err(|e| AppError::Internal(format!("serialise participants: {e}")))?;

    minerva_db::queries::daisy_pending_imports::upsert(
        &state.db,
        &minerva_db::queries::daisy_pending_imports::StageInput {
            momenttillf_id,
            course_code: beteckning,
            name,
            semester_label,
            daisy_info_url: input.info_url.as_deref(),
            daisy_syllabus_url: input.syllabus_url.as_deref(),
            daisy_unit: input.unit.as_deref(),
            participants: &participants_json,
            existing_course_id,
        },
    )
    .await?;
    summary.courses_staged += 1;
    Ok(())
}

/// Resolved Daisy participant ready for membership/owner assignment.
struct ResolvedParticipant {
    user_id: Uuid,
    /// Minerva `course_members.role` value ("teacher" or "ta").
    role: &'static str,
    /// TRUE when Daisy listed this person as kurs-/delkursansvarig
    /// AND they're employed staff (not a student-handledare). The
    /// first one we see becomes the course owner.
    eligible_for_owner: bool,
}

/// Apply a payload directly to the live `courses` table. Called from:
///   * `import_daisy_courses` when `daisy_settings.auto_apply` is ON.
///   * `daisy_admin::apply_pending` after manual review.
///
/// Two-phase per call:
///   1. Resolve every participant to a Minerva `user_id`, creating
///      a fresh row via `users::find_or_create_by_eppn` for anyone
///      Daisy lists who hasn't visited Minerva yet (same pattern the
///      auth middleware uses on first Shib launch). Identify the
///      owner candidate from the first kursansvarig-eligible row.
///   2. Upsert the course (with the resolved owner on INSERT;
///      existing rows keep their owner untouched), seed the play
///      designation on CREATE, additively add every resolved
///      participant as a course member.
///
/// Refuses to apply if Daisy listed zero owner-eligible participants
/// (kursansvarig with a staff profile). The course stays in the
/// staging table for admin attention; the alternative would be
/// inventing a "random" owner, which we'd then need machinery to
/// swap later.
pub(super) async fn apply_one(
    state: &AppState,
    input: &DaisyCourseInputPayload,
    default_embedding_model: Option<&str>,
    default_reranker_model: Option<&str>,
    summary: &mut DaisyImportSummary,
) -> Result<(), AppError> {
    let momenttillf_id = input.momenttillf_id.trim();
    let beteckning = input.beteckning.trim();
    let name = input.name.trim();
    if momenttillf_id.is_empty() || beteckning.is_empty() || name.is_empty() {
        return Err(AppError::bad_request("daisy.course_missing_required"));
    }

    // Phase 1: resolve participants. For each, find_or_create the
    // Minerva user row + register secondary eppns as aliases.
    // `default_owner_cap` is read once for any users we create; existing
    // users keep whatever cap they already have (find_or_create's
    // grandfathering semantics).
    let default_owner_cap = crate::system_defaults::owner_daily_cost_limit_usd(&state.db).await;
    let mut resolved: Vec<ResolvedParticipant> = Vec::with_capacity(input.participants.len());

    for participant in &input.participants {
        let eppns: Vec<String> = participant
            .eppns
            .iter()
            .map(|e| normalize_eppn(e))
            .filter(|e| !e.is_empty())
            .collect();
        if eppns.is_empty() {
            continue;
        }
        let (role, eligible_for_owner) =
            minerva_role_for(&participant.kind, &participant.daisy_roles);

        // Prefer an existing user found via any of the participant's
        // eppns (primary or alias). If none of the eppns resolve, we
        // create a row keyed on the canonical (first) eppn. Newly
        // created users land with role="student"; the auth
        // middleware's rule engine upgrades them on first Shib login
        // based on Shib attributes we don't have here.
        let mut user_id: Option<Uuid> = None;
        for eppn in &eppns {
            if let Some((row, _via_alias)) =
                minerva_db::queries::users::find_by_eppn_or_alias(&state.db, eppn).await?
            {
                user_id = Some(row.id);
                break;
            }
        }
        let user_id = match user_id {
            Some(id) => id,
            None => {
                let (row, created) = minerva_db::queries::users::find_or_create_by_eppn(
                    &state.db,
                    &eppns[0],
                    participant.display_name.as_deref(),
                    "student",
                    default_owner_cap,
                )
                .await?;
                if created {
                    tracing::info!(
                        eppn = %eppns[0],
                        display_name = ?participant.display_name,
                        "daisy import: created Minerva user for Daisy-resolved staff",
                    );
                }
                row.id
            }
        };

        // Register every other eppn as an alias of this user.
        for alias_eppn in eppns.iter().skip(1) {
            match minerva_db::queries::user_eppn_aliases::register(&state.db, user_id, alias_eppn)
                .await
            {
                Ok(true) => summary.aliases_registered += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    user = %user_id,
                    eppn = %alias_eppn,
                    error = %e,
                    "daisy import: alias register failed (continuing)",
                ),
            }
        }

        resolved.push(ResolvedParticipant {
            user_id,
            role,
            eligible_for_owner,
        });
    }

    // Owner = the first kursansvarig-staff we resolved. If Daisy
    // lists none we refuse to create the course; the admin can
    // either wait for Daisy to surface a kursansvarig on a future
    // sync or dismiss the staged row.
    let Some(owner_id) = resolved
        .iter()
        .find(|p| p.eligible_for_owner)
        .map(|p| p.user_id)
    else {
        return Err(AppError::bad_request("daisy.no_resolvable_owner"));
    };

    // Phase 2: upsert the course. Owner is stamped only on INSERT;
    // re-applies leave whatever owner the course currently has
    // (admin-edited ownership wins permanently). Course-AI defaults
    // are pulled from `system_defaults` so a Daisy import respects
    // /admin/defaults edits identically to a manual POST /courses
    // (these fields also stamp on INSERT only, so teacher tweaks
    // post-import survive re-syncs).
    let model = crate::system_defaults::course_model(&state.db).await;
    let strategy = crate::system_defaults::course_strategy(&state.db).await;
    let embedding_provider = crate::system_defaults::course_embedding_provider(&state.db).await;
    let system_prompt = crate::system_defaults::course_system_prompt(&state.db).await;
    let outcome = minerva_db::queries::courses::upsert_from_daisy(
        &state.db,
        &minerva_db::queries::courses::DaisyCourseInput {
            momenttillf_id,
            beteckning,
            name,
            semester_label: input.semester_label.as_deref(),
            info_url: input.info_url.as_deref(),
            syllabus_url: input.syllabus_url.as_deref(),
            unit: input.unit.as_deref(),
            owner_id,
            daily_cost_limit_usd: crate::system_defaults::course_daily_cost_limit_usd(&state.db)
                .await,
            model: Some(model.as_str()),
            temperature: Some(crate::system_defaults::course_temperature(&state.db).await),
            context_ratio: Some(crate::system_defaults::course_context_ratio(&state.db).await),
            max_chunks: Some(crate::system_defaults::course_max_chunks(&state.db).await),
            min_score: Some(crate::system_defaults::course_min_score(&state.db).await),
            strategy: Some(strategy.as_str()),
            tool_use_enabled: Some(
                crate::system_defaults::course_tool_use_enabled(&state.db).await,
            ),
            embedding_provider: Some(embedding_provider.as_str()),
            embedding_model: default_embedding_model,
            reranker_model: default_reranker_model,
            system_prompt: system_prompt.as_deref(),
        },
    )
    .await?;

    if outcome.created {
        summary.courses_created += 1;

        // Seed the play_designations row so transcript discovery
        // picks the course up the next hour. Unique on (course_id,
        // designation); a duplicate is harmless.
        match minerva_db::queries::play_designations::insert(
            &state.db,
            Uuid::new_v4(),
            outcome.course.id,
            beteckning,
            owner_id,
        )
        .await
        {
            Ok(_) => summary.designations_created += 1,
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                // Pre-registered manually; quiet.
            }
            Err(e) => return Err(e.into()),
        }
    } else {
        summary.courses_updated += 1;
    }

    // Add every resolved participant as a course member. Additive
    // only: add_member is ON CONFLICT DO UPDATE SET role, so a TA->
    // teacher promotion lands, but the membership itself never gets
    // removed by re-sync.
    for p in &resolved {
        minerva_db::queries::courses::add_member(&state.db, outcome.course.id, p.user_id, p.role)
            .await?;
        summary.members_added += 1;
    }

    minerva_db::queries::course_daisy_offerings::touch_synced(&state.db, momenttillf_id).await?;
    Ok(())
}

#[cfg(test)]
mod daisy_import_tests {
    use super::{is_course_responsible, minerva_role_for, normalize_eppn};

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn normalize_lowercases_and_trims() {
        assert_eq!(normalize_eppn(" Alice@SU.SE "), "alice@su.se");
    }

    #[test]
    fn course_responsible_recognises_canonical_and_legacy_labels() {
        // The canonical Daisy heading is "Kurs-/delkursansvarig".
        // Some older offerings use the shorter "kursansvarig". Both
        // should promote the staff person to owner-eligible; anything
        // else stays a plain teacher.
        assert!(is_course_responsible(&[s("Kurs-/delkursansvarig")]));
        assert!(is_course_responsible(&[s("kursansvarig")]));
        // Case-insensitive on the shorter form (the longer one is
        // matched via starts_with on a verbatim prefix, so casing is
        // preserved upstream).
        assert!(is_course_responsible(&[s("KURSANSVARIG")]));
        // Unrelated roles must not trigger owner promotion.
        assert!(!is_course_responsible(&[s("Examination")]));
        assert!(!is_course_responsible(&[
            s("Administration"),
            s("Handledare")
        ]));
        // Empty role list is fine; just means we don't promote.
        assert!(!is_course_responsible(&[]));
    }

    #[test]
    fn role_mapping_staff_course_responsible_is_owner_eligible() {
        let (role, owner) = minerva_role_for("staff", &[s("Kurs-/delkursansvarig")]);
        assert_eq!(role, "teacher");
        assert!(owner);
    }

    #[test]
    fn role_mapping_staff_non_owner_role_is_plain_teacher() {
        let (role, owner) = minerva_role_for("staff", &[s("Examination")]);
        assert_eq!(role, "teacher");
        assert!(!owner);
    }

    #[test]
    fn role_mapping_student_handledare_is_ta_never_owner() {
        // A student-handledare should never end up as course owner
        // even if Daisy lists them under the course-responsible
        // heading by accident (the kind=="student" branch wins).
        let (role, owner) = minerva_role_for("student", &[s("Handledare")]);
        assert_eq!(role, "ta");
        assert!(!owner);
        let (role, owner) = minerva_role_for("student", &[s("Kurs-/delkursansvarig")]);
        assert_eq!(role, "ta");
        assert!(!owner);
    }

    #[test]
    fn unrecognised_kind_falls_through_to_teacher() {
        // Defence against new dsv-wrapper kinds we haven't enumerated:
        // we'd rather over-grant teacher than silently drop a course
        // member. Owner eligibility still depends on the role labels.
        let (role, owner) = minerva_role_for("future_kind", &[s("Examination")]);
        assert_eq!(role, "teacher");
        assert!(!owner);
    }
}

#[cfg(test)]
mod offering_diff_tests {
    use super::{norm_opt, push_field_change, FieldChange, MemberChange, OfferingDiff};

    #[test]
    fn norm_opt_trims_and_empties_to_none() {
        assert_eq!(norm_opt(Some("  hi ")).as_deref(), Some("hi"));
        assert_eq!(norm_opt(Some("   ")), None);
        assert_eq!(norm_opt(Some("")), None);
        assert_eq!(norm_opt(None), None);
    }

    #[test]
    fn push_field_change_ignores_whitespace_and_empty_vs_none() {
        let mut changes: Vec<FieldChange> = Vec::new();
        // Trailing whitespace is not a real change.
        push_field_change(&mut changes, "name", Some("Algebra"), Some("Algebra "));
        // Empty string and NULL collapse to the same value.
        push_field_change(&mut changes, "unit", Some(""), None);
        assert!(changes.is_empty(), "no genuine change should be recorded");

        // A real edit is recorded with normalized old/new.
        push_field_change(&mut changes, "name", Some("Algebra"), Some("Algebra II"));
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, "name");
        assert_eq!(changes[0].old.as_deref(), Some("Algebra"));
        assert_eq!(changes[0].new.as_deref(), Some("Algebra II"));
    }

    #[test]
    fn is_empty_reflects_each_dimension() {
        // A converged update (no new course, no field/member deltas).
        assert!(OfferingDiff::default().is_empty());

        // A brand-new offering is always worth staging.
        let new_course = OfferingDiff {
            is_new_course: true,
            ..Default::default()
        };
        assert!(!new_course.is_empty());

        // A metadata-only delta is non-empty.
        let meta = OfferingDiff {
            metadata_changes: vec![FieldChange {
                field: "syllabus_url",
                old: None,
                new: Some("https://example.test/s".into()),
            }],
            ..Default::default()
        };
        assert!(!meta.is_empty());

        // A member-only delta is non-empty.
        let member = OfferingDiff {
            member_changes: vec![MemberChange {
                display_name: Some("Ada".into()),
                primary_eppn: Some("ada@su.se".into()),
                change: "added",
                role: "teacher",
                previous_role: None,
            }],
            ..Default::default()
        };
        assert!(!member.is_empty());
    }
}
