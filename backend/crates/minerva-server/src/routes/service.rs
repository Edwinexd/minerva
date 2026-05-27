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
#[derive(Deserialize)]
struct DaisyParticipantInput {
    /// One or more SU eppns, newest first. The first entry is treated
    /// as the canonical (primary) login; the rest are registered as
    /// `user_eppn_aliases` once we know which Minerva user they map to.
    eppns: Vec<String>,
    display_name: Option<String>,
    /// Free-text role labels from Daisy. Stored verbatim on
    /// `pending_course_memberships.daisy_roles`; used here only to
    /// decide `eligible_for_owner`.
    daisy_roles: Vec<String>,
    /// `"staff"` or `"student"`. Determines the Minerva
    /// `course_members.role` we add: staff → teacher, student → ta.
    /// Only `"staff"` is eligible for owner promotion (a student
    /// handledare should never become course owner).
    kind: String,
}

#[derive(Deserialize)]
struct DaisyCourseInputPayload {
    momenttillf_id: String,
    beteckning: String,
    name: String,
    /// Optional metadata; everything but `momenttillf_id`, `beteckning`,
    /// `name` is allowed to be absent so a search-page-only entry still
    /// imports cleanly.
    semester_label: Option<String>,
    info_url: Option<String>,
    syllabus_url: Option<String>,
    unit: Option<String>,
    #[serde(default)]
    participants: Vec<DaisyParticipantInput>,
}

#[derive(Serialize, Default)]
struct DaisyImportSummary {
    courses_received: usize,
    courses_created: usize,
    courses_updated: usize,
    members_added: usize,
    pending_memberships_added: usize,
    aliases_registered: usize,
    designations_created: usize,
    errors: Vec<String>,
}

/// Normalize an inbound eppn the same way `auth_middleware` does so
/// alias / primary lookups line up across the two paths. Lowercased;
/// empty / whitespace-only rejected by the caller.
fn normalize_eppn(raw: &str) -> String {
    raw.trim().to_lowercase()
}

/// True when one of the Daisy role labels marks this person as a
/// course-/delkursansvarig. Only staff (kind == "staff") get promoted
/// to owner; student-handledare are course-listed but never own.
fn is_kursansvarig(daisy_roles: &[String]) -> bool {
    daisy_roles
        .iter()
        .any(|r| r.starts_with("Kurs-/delkursansvarig") || r.eq_ignore_ascii_case("kursansvarig"))
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
        _ => ("teacher", is_kursansvarig(daisy_roles)),
    }
}

#[cfg(test)]
mod daisy_import_tests {
    use super::{is_kursansvarig, minerva_role_for, normalize_eppn};

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn normalize_lowercases_and_trims() {
        assert_eq!(normalize_eppn(" Alice@SU.SE "), "alice@su.se");
    }

    #[test]
    fn kursansvarig_recognises_canonical_and_legacy_labels() {
        // The canonical Daisy heading is "Kurs-/delkursansvarig".
        // Some older offerings use the shorter "kursansvarig". Both
        // should promote the staff person to owner-eligible; anything
        // else stays a plain teacher.
        assert!(is_kursansvarig(&[s("Kurs-/delkursansvarig")]));
        assert!(is_kursansvarig(&[s("kursansvarig")]));
        // Case-insensitive on the shorter form (the longer one is
        // matched via starts_with on a verbatim prefix, so casing is
        // preserved upstream).
        assert!(is_kursansvarig(&[s("KURSANSVARIG")]));
        // Unrelated roles must not trigger owner promotion.
        assert!(!is_kursansvarig(&[s("Examination")]));
        assert!(!is_kursansvarig(&[s("Administration"), s("Handledare")]));
        // Empty role list is fine; just means we don't promote.
        assert!(!is_kursansvarig(&[]));
    }

    #[test]
    fn role_mapping_staff_kursansvarig_is_owner_eligible() {
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
        // even if Daisy lists them under the kursansvarig heading by
        // accident (the kind=="student" branch wins).
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

/// Idempotently bulk-import Daisy course offerings + participants.
///
/// Called daily by `.github/workflows/daisy-sync.yml`
/// (`scripts/sync_daisy_courses.py`). The python side handles all the
/// dsv-wrapper interaction (course search, participants fetch,
/// username resolution via staff profile pages); the backend only
/// reasons about Minerva-side identity and idempotency.
///
/// Per-course flow:
///   1. Resolve the fallback owner from `MINERVA_DAISY_FALLBACK_OWNER_EPPN`
///      (or the first MINERVA_ADMINS entry). If missing, the whole batch
///      fails; we can't create courses without an owner.
///   2. Upsert by `daisy_momenttillf_id` via
///      `courses::upsert_from_daisy`.
///   3. On CREATE only: insert a `play_designations` row so
///      transcript discovery picks the course up on the next
///      transcripts.yml run.
///   4. For each participant: resolve their canonical eppn (primary or
///      alias of an existing user). If found, additively add to
///      `course_members` and register any other eppns they hold as
///      aliases. If not found, queue one `pending_course_memberships`
///      row per eppn (so a future login via any of them drains).
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

    let mut summary = DaisyImportSummary {
        courses_received: body.len(),
        ..Default::default()
    };

    // Resolve the env-var fallback owner once for the whole batch.
    // Missing config / missing user is a hard error: every auto-import
    // INSERT needs a non-NULL owner_id.
    let fallback_eppn = state
        .config
        .daisy_fallback_owner_eppn
        .as_deref()
        .ok_or_else(|| AppError::Internal("MINERVA_DAISY_FALLBACK_OWNER_EPPN unresolved".into()))?;
    let fallback_owner = minerva_db::queries::users::find_by_eppn(&state.db, fallback_eppn)
        .await?
        .ok_or_else(|| {
            AppError::Internal(format!(
                "daisy fallback owner eppn {fallback_eppn} not present in users table; \
                 ensure they have logged in at least once"
            ))
        })?;

    // Same admin-default embedding model the manual course-creation
    // endpoint honors. None falls through to the column DEFAULT.
    let default_embedding_model =
        minerva_db::queries::embedding_models::current_default(&state.db).await?;

    for input in body {
        match import_one(
            &state,
            &input,
            fallback_owner.id,
            default_embedding_model.as_deref(),
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
                    "daisy import: per-course failure",
                );
            }
        }
    }

    tracing::info!(
        "daisy import: received={} created={} updated={} members_added={} pending={} aliases={}",
        summary.courses_received,
        summary.courses_created,
        summary.courses_updated,
        summary.members_added,
        summary.pending_memberships_added,
        summary.aliases_registered,
    );
    Ok(Json(summary))
}

async fn import_one(
    state: &AppState,
    input: &DaisyCourseInputPayload,
    fallback_owner_id: Uuid,
    default_embedding_model: Option<&str>,
    summary: &mut DaisyImportSummary,
) -> Result<(), AppError> {
    let momenttillf_id = input.momenttillf_id.trim();
    let beteckning = input.beteckning.trim();
    let name = input.name.trim();
    if momenttillf_id.is_empty() || beteckning.is_empty() || name.is_empty() {
        return Err(AppError::bad_request("daisy.course_missing_required"));
    }

    // Keep the user-facing name simple: just the Daisy `name`. The
    // frontend shows `beteckning` separately via the semester badge.
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
            fallback_owner_id,
            daily_token_limit: state.config.default_course_daily_token_limit,
            embedding_model: default_embedding_model,
        },
    )
    .await?;

    if outcome.created {
        summary.courses_created += 1;

        // Seed the play_designations row so transcript discovery
        // picks the course up the next hour. Unique on (course_id,
        // designation); a duplicate is harmless (just means the row
        // already existed via a prior manual setup).
        match minerva_db::queries::play_designations::insert(
            &state.db,
            Uuid::new_v4(),
            outcome.course.id,
            beteckning,
            fallback_owner_id,
        )
        .await
        {
            Ok(_) => summary.designations_created += 1,
            Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
                // Already present (very unlikely on a fresh CREATE,
                // but possible if a teacher pre-registered it). Quiet.
            }
            Err(e) => return Err(e.into()),
        }
    } else {
        summary.courses_updated += 1;
    }

    // Roster sync. Additive only: never remove a member who's no
    // longer in Daisy. add_member is ON CONFLICT DO UPDATE SET role,
    // so role changes between syncs (TA → teacher) propagate; but we
    // never demote a manually-added human (since add_member with the
    // resolved Daisy role would only ever stay the same or upgrade).
    for participant in &input.participants {
        if participant.eppns.is_empty() {
            continue;
        }
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

        // Find a Minerva user that already owns any of these eppns
        // (primary or alias). First hit wins; alias hits are NOT
        // promoted here (we don't want a Daisy sync to silently swap
        // a user's primary eppn out from under them; the auth path
        // does that explicitly when SAML hands us the alias).
        let mut resolved_user_id: Option<Uuid> = None;
        for eppn in &eppns {
            if let Some((row, _via_alias)) =
                minerva_db::queries::users::find_by_eppn_or_alias(&state.db, eppn).await?
            {
                resolved_user_id = Some(row.id);
                break;
            }
        }

        if let Some(user_id) = resolved_user_id {
            // Real Minerva user; add as member (idempotent).
            minerva_db::queries::courses::add_member(&state.db, outcome.course.id, user_id, role)
                .await?;
            summary.members_added += 1;

            // Register every additional eppn as an alias of this user.
            // No-op when an eppn is already the user's primary or
            // already an alias somewhere; the helper's docs cover the
            // edge cases.
            for eppn in &eppns {
                match minerva_db::queries::user_eppn_aliases::register(&state.db, user_id, eppn)
                    .await
                {
                    Ok(true) => summary.aliases_registered += 1,
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(
                            user = %user_id,
                            eppn = %eppn,
                            error = %e,
                            "daisy import: alias register failed (continuing)",
                        );
                    }
                }
            }

            // If they're a kursansvarig and the course currently sits
            // on the fallback owner, swap ownership now. (The login
            // drain path also does this for users who haven't logged
            // in yet; doing it here for users who have logged in
            // avoids a stale-owner window until they next visit.)
            if eligible_for_owner {
                let _ = minerva_db::queries::courses::swap_owner_from_fallback(
                    &state.db,
                    outcome.course.id,
                    fallback_owner_id,
                    user_id,
                )
                .await?;
            }
        } else {
            // No Minerva user yet. Queue one pending row per eppn so
            // a future login via any of them drains.
            for eppn in &eppns {
                minerva_db::queries::pending_course_memberships::upsert(
                    &state.db,
                    &minerva_db::queries::pending_course_memberships::PendingUpsert {
                        course_id: outcome.course.id,
                        eppn,
                        display_name: participant.display_name.as_deref(),
                        role,
                        eligible_for_owner,
                        daisy_roles: &participant.daisy_roles,
                        daisy_momenttillf_id: Some(momenttillf_id),
                    },
                )
                .await?;
                summary.pending_memberships_added += 1;
            }
        }
    }

    // Bump the synced-at timestamp at the end so a partial failure
    // leaves the row pointing at its previous successful sync.
    minerva_db::queries::courses::touch_daisy_synced(&state.db, outcome.course.id).await?;
    Ok(())
}
