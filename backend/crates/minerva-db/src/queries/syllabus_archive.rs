//! Course syllabi from SU's syllabus archive; see migration
//! `20261104000001_syllabus_archive.sql`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub struct PlanInput<'a> {
    pub code: &'a str,
    pub name_sv: &'a str,
    pub name_en: Option<&'a str>,
    pub education_type: &'a str,
    pub ects: Option<f32>,
    pub description: Option<&'a str>,
    pub is_discontinued: bool,
}

pub struct VersionInput<'a> {
    pub valid_from: &'a str,
    pub valid_from_term_id: i32,
    pub language: &'a str,
    pub pdf_url: &'a str,
    pub title: &'a str,
}

pub struct OfferingInput<'a> {
    pub momenttillf_id: &'a str,
    pub requirement: Option<&'a str>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ReconcileSummary {
    pub versions_added: u64,
    pub versions_url_changed: u64,
    pub versions_delisted: u64,
    pub versions_relisted: u64,
    pub offerings: u64,
}

/// Replace one plan's listing in a transaction: upsert the plan row, upsert
/// every listed version, delist versions the archive no longer shows, and
/// replace the plan's offering links.
///
/// A version is identified by (code, term, language). If its PDF URL changes
/// it is re-queued for download immediately.
pub async fn reconcile_plan(
    db: &PgPool,
    plan: &PlanInput<'_>,
    versions: &[VersionInput<'_>],
    offerings: &[OfferingInput<'_>],
) -> Result<ReconcileSummary, sqlx::Error> {
    let mut tx = db.begin().await?;
    let mut summary = ReconcileSummary::default();

    sqlx::query!(
        r#"INSERT INTO syllabus_plans
               (code, name_sv, name_en, education_type, ects, description, is_discontinued)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (code) DO UPDATE SET
               name_sv = EXCLUDED.name_sv,
               name_en = COALESCE(EXCLUDED.name_en, syllabus_plans.name_en),
               education_type = EXCLUDED.education_type,
               ects = EXCLUDED.ects,
               description = EXCLUDED.description,
               is_discontinued = EXCLUDED.is_discontinued,
               last_listed_at = NOW(),
               updated_at = NOW()"#,
        plan.code,
        plan.name_sv,
        plan.name_en,
        plan.education_type,
        plan.ects,
        plan.description,
        plan.is_discontinued,
    )
    .execute(&mut *tx)
    .await?;

    let mut kept: Vec<Uuid> = Vec::with_capacity(versions.len());
    for v in versions {
        let row = sqlx::query!(
            r#"WITH prev AS (
                   SELECT pdf_url, delisted_at FROM syllabus_versions
                   WHERE code = $1 AND valid_from = $2 AND language = $4
               )
               INSERT INTO syllabus_versions
                   (code, valid_from, valid_from_term_id, language, pdf_url, title)
               VALUES ($1, $2, $3, $4, $5, $6)
               ON CONFLICT (code, valid_from, language) DO UPDATE SET
                   title = EXCLUDED.title,
                   valid_from_term_id = EXCLUDED.valid_from_term_id,
                   pdf_url = EXCLUDED.pdf_url,
                   next_check_at = CASE
                       WHEN syllabus_versions.pdf_url <> EXCLUDED.pdf_url THEN NOW()
                       ELSE syllabus_versions.next_check_at
                   END,
                   delisted_at = NULL
               RETURNING id,
                   (SELECT pdf_url FROM prev) AS "prev_url?",
                   (SELECT delisted_at FROM prev) AS "prev_delisted_at?""#,
            plan.code,
            v.valid_from,
            v.valid_from_term_id,
            v.language,
            v.pdf_url,
            v.title,
        )
        .fetch_one(&mut *tx)
        .await?;
        match row.prev_url {
            None => summary.versions_added += 1,
            Some(prev) if prev != v.pdf_url => summary.versions_url_changed += 1,
            Some(_) => {}
        }
        if row.prev_delisted_at.is_some() {
            summary.versions_relisted += 1;
        }
        kept.push(row.id);
    }

    summary.versions_delisted = sqlx::query!(
        r#"UPDATE syllabus_versions SET delisted_at = NOW()
           WHERE code = $1 AND delisted_at IS NULL AND NOT (id = ANY($2))"#,
        plan.code,
        &kept,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let ids: Vec<String> = offerings
        .iter()
        .map(|o| o.momenttillf_id.to_owned())
        .collect();
    let requirements: Vec<Option<String>> = offerings
        .iter()
        .map(|o| o.requirement.map(str::to_owned))
        .collect();
    sqlx::query!(
        "DELETE FROM syllabus_plan_offerings WHERE code = $1 AND NOT (momenttillf_id = ANY($2))",
        plan.code,
        &ids,
    )
    .execute(&mut *tx)
    .await?;
    summary.offerings = sqlx::query!(
        r#"INSERT INTO syllabus_plan_offerings (code, momenttillf_id, requirement)
           SELECT $1, o.id, o.requirement
           FROM UNNEST($2::text[], $3::text[]) AS o(id, requirement)
           ON CONFLICT (code, momenttillf_id) DO UPDATE SET
               requirement = EXCLUDED.requirement,
               synced_at = NOW()"#,
        plan.code,
        &ids,
        &requirements as &[Option<String>],
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit().await?;
    Ok(summary)
}

pub struct DueVersion {
    pub id: Uuid,
    pub code: String,
    pub valid_from: String,
    pub language: String,
    pub pdf_url: String,
    pub content_hash: Option<String>,
}

/// Listed versions whose download or periodic re-check is due, oldest
/// first. Never-downloaded versions sort first because they are created
/// with `next_check_at = NOW()`.
pub async fn find_due_versions(db: &PgPool, limit: i64) -> Result<Vec<DueVersion>, sqlx::Error> {
    sqlx::query_as!(
        DueVersion,
        r#"SELECT id, code, valid_from, language, pdf_url, content_hash
           FROM syllabus_versions
           WHERE delisted_at IS NULL AND next_check_at <= NOW()
           ORDER BY next_check_at
           LIMIT $1"#,
        limit,
    )
    .fetch_all(db)
    .await
}

/// Record a successful download. `changed` moves `content_changed_at`, the
/// signal downstream indexing keys on.
pub async fn record_fetch_success(
    db: &PgPool,
    id: Uuid,
    content_hash: &str,
    size_bytes: i64,
    changed: bool,
    next_check_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE syllabus_versions SET
               content_hash = $2,
               size_bytes = $3,
               content_changed_at = CASE WHEN $4 THEN NOW() ELSE content_changed_at END,
               facts_parsed_at = CASE WHEN $4 THEN NULL ELSE facts_parsed_at END,
               last_checked_at = NOW(),
               next_check_at = $5,
               fetch_error = NULL,
               fetch_failures = 0
           WHERE id = $1"#,
        id,
        content_hash,
        size_bytes,
        changed,
        next_check_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Record a failed download and back off: 1h, 2h, 4h, ... capped at
/// `max_backoff_hours`. Returns the new failure count.
pub async fn record_fetch_failure(
    db: &PgPool,
    id: Uuid,
    error: &str,
    max_backoff_hours: i32,
) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"UPDATE syllabus_versions SET
               fetch_error = $2,
               fetch_failures = fetch_failures + 1,
               last_checked_at = NOW(),
               next_check_at = NOW() + make_interval(
                   hours => LEAST(power(2, LEAST(fetch_failures, 16))::int, $3)
               )
           WHERE id = $1
           RETURNING fetch_failures"#,
        id,
        error,
        max_backoff_hours,
    )
    .fetch_one(db)
    .await?;
    Ok(row.fetch_failures)
}

/// A downloaded PDF of a plan's current version, ready to index.
pub struct CurrentVersion {
    pub code: String,
    pub language: String,
    pub valid_from: String,
    pub content_hash: String,
    pub name_sv: String,
    pub name_en: Option<String>,
}

/// The newest listed version of every plan still given, in each language
/// that version has a downloaded PDF for. An English PDF from an older
/// version is left out: comparing against outdated outcomes would be wrong,
/// and the archive often only translated earlier versions. Discontinued
/// plans are left out because credit is granted towards courses DSV gives.
pub async fn current_versions(db: &PgPool) -> Result<Vec<CurrentVersion>, sqlx::Error> {
    sqlx::query_as!(
        CurrentVersion,
        r#"WITH latest AS (
               SELECT code, MAX(valid_from_term_id) AS term_id
               FROM syllabus_versions
               WHERE delisted_at IS NULL
               GROUP BY code
           )
           SELECT v.code, v.language, v.valid_from,
                  v.content_hash AS "content_hash!", p.name_sv, p.name_en
           FROM syllabus_versions v
           JOIN latest l ON l.code = v.code AND l.term_id = v.valid_from_term_id
           JOIN syllabus_plans p ON p.code = v.code
           WHERE v.delisted_at IS NULL
             AND v.content_hash IS NOT NULL
             AND NOT p.is_discontinued
           ORDER BY v.code, v.language"#,
    )
    .fetch_all(db)
    .await
}

/// A downloaded version whose level and modules have not been read yet:
/// new downloads, revisions, and everything downloaded before the facts
/// columns existed.
pub struct FactsPending {
    pub id: Uuid,
    pub content_hash: String,
}

pub async fn find_facts_pending(db: &PgPool, limit: i64) -> Result<Vec<FactsPending>, sqlx::Error> {
    sqlx::query_as!(
        FactsPending,
        r#"SELECT id, content_hash AS "content_hash!"
           FROM syllabus_versions
           WHERE content_hash IS NOT NULL AND facts_parsed_at IS NULL
           ORDER BY code
           LIMIT $1"#,
        limit,
    )
    .fetch_all(db)
    .await
}

/// Store what was read off a version's PDF. Called with `None`s when the
/// file could not be read, so a broken PDF is not retried every tick; a
/// re-download with new content clears `facts_parsed_at` again.
pub async fn record_facts(
    db: &PgPool,
    id: Uuid,
    text: Option<&str>,
    level: Option<&str>,
    modules: Option<&serde_json::Value>,
    page_count: Option<i32>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE syllabus_versions SET
               text = $2, level = $3, modules = $4, page_count = $5, facts_parsed_at = NOW()
           WHERE id = $1"#,
        id,
        text,
        level,
        modules,
        page_count,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// One language of a plan's current version, with everything an
/// assessment states about it.
#[derive(Debug, Clone)]
pub struct CurrentPlan {
    pub code: String,
    pub language: String,
    pub valid_from: String,
    pub pdf_url: String,
    pub content_hash: String,
    pub name_sv: String,
    pub name_en: Option<String>,
    pub ects: Option<f32>,
    pub level: Option<String>,
    pub modules: Option<serde_json::Value>,
    /// Page-marked text; NULL until the scheduler has read the PDF.
    pub text: Option<String>,
    pub page_count: Option<i32>,
}

/// The current version of the given plans (same rule as
/// [`current_versions`]), in every downloaded language.
pub async fn current_plans(db: &PgPool, codes: &[String]) -> Result<Vec<CurrentPlan>, sqlx::Error> {
    sqlx::query_as!(
        CurrentPlan,
        r#"WITH latest AS (
               SELECT code, MAX(valid_from_term_id) AS term_id
               FROM syllabus_versions
               WHERE delisted_at IS NULL AND code = ANY($1)
               GROUP BY code
           )
           SELECT v.code, v.language, v.valid_from, v.pdf_url,
                  v.content_hash AS "content_hash!", p.name_sv, p.name_en, p.ects,
                  v.level, v.modules, v.text, v.page_count
           FROM syllabus_versions v
           JOIN latest l ON l.code = v.code AND l.term_id = v.valid_from_term_id
           JOIN syllabus_plans p ON p.code = v.code
           WHERE v.delisted_at IS NULL
             AND v.content_hash IS NOT NULL
             AND NOT p.is_discontinued
           ORDER BY v.code, v.language"#,
        codes,
    )
    .fetch_all(db)
    .await
}

/// Plan codes among `candidates` (upper-cased words from a user's note)
/// that name a plan, either by its Ladok code or by the Daisy designation
/// of an offering it is linked to (`PROG1` for `IB138N`).
pub async fn resolve_codes(db: &PgPool, candidates: &[String]) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT DISTINCT p.code AS "code!"
           FROM syllabus_plans p
           LEFT JOIN syllabus_plan_offerings o ON o.code = p.code
           LEFT JOIN course_daisy_offerings d ON d.momenttillf_id = o.momenttillf_id
           WHERE NOT p.is_discontinued
             AND (p.code = ANY($1) OR UPPER(split_part(d.course_code, ' ', 1)) = ANY($1))"#,
        candidates,
    )
    .fetch_all(db)
    .await
}

pub struct PlanListing {
    pub code: String,
    pub name_sv: String,
    pub name_en: Option<String>,
    pub ects: Option<f32>,
    pub is_discontinued: bool,
}

pub async fn list_plans(db: &PgPool) -> Result<Vec<PlanListing>, sqlx::Error> {
    sqlx::query_as!(
        PlanListing,
        r#"SELECT code, name_sv, name_en, ects, is_discontinued
           FROM syllabus_plans ORDER BY code"#,
    )
    .fetch_all(db)
    .await
}

pub struct VersionListing {
    pub code: String,
    pub valid_from: String,
    pub valid_from_term_id: i32,
    pub language: String,
    pub pdf_url: String,
    pub downloaded: bool,
    pub fetch_error: Option<String>,
    pub level: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub content_changed_at: Option<DateTime<Utc>>,
    pub last_checked_at: Option<DateTime<Utc>>,
}

/// Every listed version, for the plan overview.
pub async fn list_versions(db: &PgPool) -> Result<Vec<VersionListing>, sqlx::Error> {
    sqlx::query_as!(
        VersionListing,
        r#"SELECT code, valid_from, valid_from_term_id, language, pdf_url,
                  content_hash IS NOT NULL AS "downloaded!", fetch_error, level,
                  first_seen_at, content_changed_at, last_checked_at
           FROM syllabus_versions
           WHERE delisted_at IS NULL
           ORDER BY code, valid_from_term_id DESC, language"#,
    )
    .fetch_all(db)
    .await
}

/// Text of every plan's current version, one language per plan (Swedish
/// when there is one: it is the authoritative text). For ranking plans
/// against an external syllabus without the embedding index.
pub struct PlanText {
    pub code: String,
    pub text: String,
}

pub async fn current_plan_texts(db: &PgPool) -> Result<Vec<PlanText>, sqlx::Error> {
    sqlx::query_as!(
        PlanText,
        r#"WITH latest AS (
               SELECT code, MAX(valid_from_term_id) AS term_id
               FROM syllabus_versions
               WHERE delisted_at IS NULL
               GROUP BY code
           )
           SELECT DISTINCT ON (v.code) v.code, v.text AS "text!"
           FROM syllabus_versions v
           JOIN latest l ON l.code = v.code AND l.term_id = v.valid_from_term_id
           JOIN syllabus_plans p ON p.code = v.code
           WHERE v.delisted_at IS NULL AND v.text IS NOT NULL AND NOT p.is_discontinued
           ORDER BY v.code, v.language = 'sv' DESC"#,
    )
    .fetch_all(db)
    .await
}

/// Plans whose current text contains `needle` (case-insensitive), for
/// searching the plan list by content as well as code and name.
pub async fn codes_with_text(db: &PgPool, needle: &str) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT DISTINCT code AS "code!"
           FROM syllabus_versions
           WHERE delisted_at IS NULL AND text IS NOT NULL
             AND strpos(lower(text), lower($1)) > 0"#,
        needle,
    )
    .fetch_all(db)
    .await
}
