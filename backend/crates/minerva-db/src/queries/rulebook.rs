//! SU's rulebook (Regelboken); see migration `20261105000001_rulebook.sql`.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgPool;

/// Section indexes whose daily read is due.
pub async fn due_sections(db: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT url FROM rulebook_sections WHERE next_check_at <= NOW() ORDER BY next_check_at"
    )
    .fetch_all(db)
    .await
}

pub async fn record_section_success(
    db: &PgPool,
    url: &str,
    next_check_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE rulebook_sections SET
               last_checked_at = NOW(),
               next_check_at = $2,
               fetch_error = NULL,
               fetch_failures = 0
           WHERE url = $1"#,
        url,
        next_check_at,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// Record a failed read and back off: 1h, 2h, 4h, ... capped at
/// `max_backoff_hours`. Returns the new failure count.
pub async fn record_section_failure(
    db: &PgPool,
    url: &str,
    error: &str,
    max_backoff_hours: i32,
) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"UPDATE rulebook_sections SET
               fetch_error = $2,
               fetch_failures = fetch_failures + 1,
               last_checked_at = NOW(),
               next_check_at = NOW() + make_interval(
                   hours => LEAST(power(2, LEAST(fetch_failures, 16))::int, $3)
               )
           WHERE url = $1
           RETURNING fetch_failures"#,
        url,
        error,
        max_backoff_hours,
    )
    .fetch_one(db)
    .await
}

#[derive(Clone, Copy)]
pub struct EntryInput<'a> {
    pub url: &'a str,
    pub title: &'a str,
    pub category: &'a str,
    pub is_file: bool,
}

#[derive(Debug, Default)]
pub struct IndexSummary {
    pub added: u64,
    pub relisted: u64,
    pub delisted: u64,
}

/// Replace a section's listing in a transaction: upsert every entry in
/// index order and delist the entries the index no longer shows.
///
/// A page keeps the title its own heading gives it (set when it is read),
/// so the index's link text does not flip it back every day. A file entry
/// has no page to read, so it takes the link text, and it is its own only
/// file.
pub async fn reconcile_section(
    db: &PgPool,
    section_url: &str,
    entries: &[EntryInput<'_>],
) -> Result<IndexSummary, sqlx::Error> {
    let mut tx = db.begin().await?;
    let mut summary = IndexSummary::default();
    let mut kept: Vec<String> = Vec::with_capacity(entries.len());
    for (position, e) in entries.iter().enumerate() {
        let position = i32::try_from(position).unwrap_or(i32::MAX);
        let row = sqlx::query!(
            r#"WITH prev AS (
                   SELECT delisted_at FROM rulebook_pages WHERE url = $1
               )
               INSERT INTO rulebook_pages (url, section_url, category, position, title, is_file)
               VALUES ($1, $2, $3, $4, $5, $6)
               ON CONFLICT (url) DO UPDATE SET
                   section_url = EXCLUDED.section_url,
                   category = EXCLUDED.category,
                   position = EXCLUDED.position,
                   title = CASE WHEN EXCLUDED.is_file THEN EXCLUDED.title
                                ELSE rulebook_pages.title END,
                   is_file = EXCLUDED.is_file,
                   delisted_at = NULL
               RETURNING (SELECT TRUE FROM prev) AS "existed?",
                         (SELECT delisted_at FROM prev) AS "prev_delisted_at?""#,
            e.url,
            section_url,
            e.category,
            position,
            e.title,
            e.is_file,
        )
        .fetch_one(&mut *tx)
        .await?;
        if row.existed.is_none() {
            summary.added += 1;
        } else if row.prev_delisted_at.is_some() {
            summary.relisted += 1;
        }
        if e.is_file {
            sqlx::query!(
                r#"INSERT INTO rulebook_files (page_url, url, position, title)
                   VALUES ($1, $1, 0, $2)
                   ON CONFLICT (page_url, url) DO UPDATE SET
                       title = EXCLUDED.title,
                       delisted_at = NULL"#,
                e.url,
                e.title,
            )
            .execute(&mut *tx)
            .await?;
        }
        kept.push(e.url.to_owned());
    }
    summary.delisted = sqlx::query!(
        r#"UPDATE rulebook_pages SET delisted_at = NOW()
           WHERE section_url = $1 AND delisted_at IS NULL AND NOT (url = ANY($2))"#,
        section_url,
        &kept,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(summary)
}

pub struct DuePage {
    pub url: String,
    pub title: String,
    pub category: String,
    pub content_hash: Option<String>,
}

/// Listed pages whose daily read is due, oldest first; new pages sort
/// first because they are created due.
pub async fn due_pages(db: &PgPool, limit: i64) -> Result<Vec<DuePage>, sqlx::Error> {
    sqlx::query_as!(
        DuePage,
        r#"SELECT url, title, category, content_hash
           FROM rulebook_pages
           WHERE delisted_at IS NULL AND NOT is_file AND next_check_at <= NOW()
           ORDER BY next_check_at
           LIMIT $1"#,
        limit,
    )
    .fetch_all(db)
    .await
}

#[derive(Clone, Copy)]
pub struct FileInput<'a> {
    pub url: &'a str,
    pub title: &'a str,
}

pub struct PageRead<'a> {
    pub url: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub content_hash: &'a str,
    pub updated_on: Option<NaiveDate>,
    pub files: &'a [FileInput<'a>],
    pub next_check_at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct PageSummary {
    pub changed: bool,
    pub files_added: u64,
    pub files_delisted: u64,
}

/// Record a read page: its text, and the files it links now. A file
/// another page already links is taken over downloaded. A changed text or
/// file list asks for what to mirror to be decided again.
pub async fn record_page_success(
    db: &PgPool,
    read: &PageRead<'_>,
) -> Result<PageSummary, sqlx::Error> {
    let mut tx = db.begin().await?;
    let prev_hash = sqlx::query_scalar!(
        "SELECT content_hash FROM rulebook_pages WHERE url = $1 FOR UPDATE",
        read.url,
    )
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    let mut summary = PageSummary {
        changed: prev_hash.as_deref() != Some(read.content_hash),
        ..PageSummary::default()
    };

    let mut kept: Vec<String> = Vec::with_capacity(read.files.len());
    for (position, f) in read.files.iter().enumerate() {
        let position = i32::try_from(position).unwrap_or(i32::MAX);
        let row = sqlx::query!(
            r#"WITH prev AS (
                   SELECT delisted_at FROM rulebook_files WHERE page_url = $1 AND url = $2
               ),
               known AS (
                   SELECT content_hash, size_bytes, text, fetched_at
                   FROM rulebook_files
                   WHERE url = $2 AND fetched_at IS NOT NULL
                   LIMIT 1
               )
               INSERT INTO rulebook_files
                   (page_url, url, position, title, content_hash, size_bytes, text, fetched_at)
               SELECT $1, $2, $3, $4, k.content_hash, k.size_bytes, k.text, k.fetched_at
               FROM (SELECT 1) one LEFT JOIN known k ON TRUE
               ON CONFLICT (page_url, url) DO UPDATE SET
                   position = EXCLUDED.position,
                   title = EXCLUDED.title,
                   delisted_at = NULL
               RETURNING (SELECT TRUE FROM prev) AS "existed?",
                         (SELECT delisted_at FROM prev) AS "prev_delisted_at?""#,
            read.url,
            f.url,
            position,
            f.title,
        )
        .fetch_one(&mut *tx)
        .await?;
        if row.existed.is_none() || row.prev_delisted_at.is_some() {
            summary.files_added += 1;
        }
        kept.push(f.url.to_owned());
    }
    summary.files_delisted = sqlx::query!(
        r#"UPDATE rulebook_files SET delisted_at = NOW()
           WHERE page_url = $1 AND delisted_at IS NULL AND NOT (url = ANY($2))"#,
        read.url,
        &kept,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    let reselect = summary.changed || summary.files_added > 0 || summary.files_delisted > 0;
    sqlx::query!(
        r#"UPDATE rulebook_pages SET
               title = $2,
               body = $3,
               content_hash = $4,
               updated_on = $5,
               content_changed_at = CASE WHEN $6 THEN NOW() ELSE content_changed_at END,
               needs_selection = needs_selection OR $7,
               last_checked_at = NOW(),
               next_check_at = $8,
               fetch_error = NULL,
               fetch_failures = 0
           WHERE url = $1"#,
        read.url,
        read.title,
        read.body,
        read.content_hash,
        read.updated_on,
        summary.changed,
        reselect,
        read.next_check_at,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(summary)
}

/// Record a failed page read and back off like a section.
pub async fn record_page_failure(
    db: &PgPool,
    url: &str,
    error: &str,
    max_backoff_hours: i32,
) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"UPDATE rulebook_pages SET
               fetch_error = $2,
               fetch_failures = fetch_failures + 1,
               last_checked_at = NOW(),
               next_check_at = NOW() + make_interval(
                   hours => LEAST(power(2, LEAST(fetch_failures, 16))::int, $3)
               )
           WHERE url = $1
           RETURNING fetch_failures"#,
        url,
        error,
        max_backoff_hours,
    )
    .fetch_one(db)
    .await
}

/// Listed files not downloaded yet whose next attempt is due, each URL
/// once however many pages link it.
pub async fn due_files(db: &PgPool, limit: i64) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT f.url
           FROM rulebook_files f
           JOIN rulebook_pages p ON p.url = f.page_url
           WHERE f.delisted_at IS NULL AND p.delisted_at IS NULL
             AND f.fetched_at IS NULL AND f.next_attempt_at <= NOW()
           GROUP BY f.url
           ORDER BY MIN(f.next_attempt_at)
           LIMIT $1"#,
        limit,
    )
    .fetch_all(db)
    .await
}

/// Record a downloaded file on every page that links it.
pub async fn record_file_success(
    db: &PgPool,
    url: &str,
    content_hash: &str,
    size_bytes: i64,
    text: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query!(
        r#"UPDATE rulebook_files SET
               content_hash = $2,
               size_bytes = $3,
               text = $4,
               fetched_at = NOW(),
               fetch_error = NULL
           WHERE url = $1"#,
        url,
        content_hash,
        size_bytes,
        text,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"UPDATE rulebook_pages SET needs_selection = TRUE
           WHERE url IN (SELECT page_url FROM rulebook_files WHERE url = $1)"#,
        url,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Record a failed download on every page that links the file, and back
/// off like a section. Its pages are reconsidered: a page waits for each
/// file's first attempt, not for a file that keeps failing. Returns the
/// new failure count.
pub async fn record_file_failure(
    db: &PgPool,
    url: &str,
    error: &str,
    max_backoff_hours: i32,
) -> Result<i32, sqlx::Error> {
    let mut tx = db.begin().await?;
    let failures = sqlx::query_scalar!(
        r#"UPDATE rulebook_files SET
               fetch_error = $2,
               fetch_failures = fetch_failures + 1,
               next_attempt_at = NOW() + make_interval(
                   hours => LEAST(power(2, LEAST(fetch_failures, 16))::int, $3)
               )
           WHERE url = $1
           RETURNING fetch_failures"#,
        url,
        error,
        max_backoff_hours,
    )
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query!(
        r#"UPDATE rulebook_pages SET needs_selection = TRUE
           WHERE url IN (SELECT page_url FROM rulebook_files WHERE url = $1)"#,
        url,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(failures.into_iter().max().unwrap_or(0))
}

pub struct SelectionPage {
    pub url: String,
    pub body: Option<String>,
}

/// Pages whose mirrored documents are to be decided again, once each of
/// their listed files has been tried at least once.
pub async fn pages_to_select(db: &PgPool) -> Result<Vec<SelectionPage>, sqlx::Error> {
    sqlx::query_as!(
        SelectionPage,
        r#"SELECT p.url, p.body
           FROM rulebook_pages p
           WHERE p.needs_selection AND p.delisted_at IS NULL
             AND (p.is_file OR p.body IS NOT NULL)
             AND NOT EXISTS (
                 SELECT 1 FROM rulebook_files f
                 WHERE f.page_url = p.url AND f.delisted_at IS NULL
                   AND f.fetched_at IS NULL AND f.fetch_failures = 0
             )"#,
    )
    .fetch_all(db)
    .await
}

pub struct SelectionFile {
    pub page_url: String,
    pub url: String,
    pub text: Option<String>,
}

/// The downloaded, listed files of the given pages, in page order.
pub async fn selection_files(
    db: &PgPool,
    page_urls: &[String],
) -> Result<Vec<SelectionFile>, sqlx::Error> {
    sqlx::query_as!(
        SelectionFile,
        r#"SELECT page_url, url, text
           FROM rulebook_files
           WHERE page_url = ANY($1) AND delisted_at IS NULL AND fetched_at IS NOT NULL
           ORDER BY page_url, position"#,
        page_urls,
    )
    .fetch_all(db)
    .await
}

/// Store what to mirror of one page. The scheduler is the only writer and
/// runs its steps in sequence, so a read cannot land between the decision
/// and this write.
pub async fn record_selection(
    db: &PgPool,
    page_url: &str,
    mirror_text: bool,
    mirrored_files: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query!(
        "UPDATE rulebook_pages SET mirror_text = $2, needs_selection = FALSE WHERE url = $1",
        page_url,
        mirror_text,
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "UPDATE rulebook_files SET mirror = (url = ANY($2)) WHERE page_url = $1",
        page_url,
        mirrored_files,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// A document to keep in every credit transfer workspace: a page's text
/// or a file, keyed by its URL (the mirrored document's `source_ref`).
pub struct MirrorDocument {
    pub url: String,
    pub title: String,
    pub category: String,
    pub is_page: bool,
    pub content_hash: String,
}

/// Everything currently chosen to be mirrored, each URL once.
pub async fn mirror_documents(db: &PgPool) -> Result<Vec<MirrorDocument>, sqlx::Error> {
    sqlx::query_as!(
        MirrorDocument,
        r#"SELECT url AS "url!", title AS "title!", category AS "category!",
                  is_page AS "is_page!", content_hash AS "content_hash!"
           FROM (
               SELECT p.url, p.title, p.category, TRUE AS is_page, p.content_hash,
                      p.section_url, p.position, 0 AS file_position
               FROM rulebook_pages p
               WHERE p.delisted_at IS NULL AND p.mirror_text
                 AND p.body IS NOT NULL AND p.content_hash IS NOT NULL
               UNION ALL
               (
                   SELECT DISTINCT ON (f.url)
                          f.url, f.title, p.category, FALSE, f.content_hash,
                          p.section_url, p.position, f.position + 1
                   FROM rulebook_files f
                   JOIN rulebook_pages p ON p.url = f.page_url
                   WHERE f.delisted_at IS NULL AND p.delisted_at IS NULL
                     AND f.mirror AND f.content_hash IS NOT NULL
                   ORDER BY f.url, p.position
               )
           ) d
           ORDER BY section_url, position, file_position"#,
    )
    .fetch_all(db)
    .await
}

pub async fn page_body(db: &PgPool, url: &str) -> Result<Option<String>, sqlx::Error> {
    Ok(
        sqlx::query_scalar!("SELECT body FROM rulebook_pages WHERE url = $1", url)
            .fetch_optional(db)
            .await?
            .flatten(),
    )
}

/// When the section indexes were last read.
pub async fn last_checked_at(db: &PgPool) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    sqlx::query_scalar!("SELECT MAX(last_checked_at) FROM rulebook_sections")
        .fetch_one(db)
        .await
}

pub struct PageListing {
    pub url: String,
    pub category: String,
    pub title: String,
    pub is_file: bool,
    pub updated_on: Option<NaiveDate>,
    pub read: bool,
    pub fetch_error: Option<String>,
    pub needs_selection: bool,
    pub mirror_text: bool,
}

/// Every listed entry, in index order.
pub async fn list_pages(db: &PgPool) -> Result<Vec<PageListing>, sqlx::Error> {
    sqlx::query_as!(
        PageListing,
        r#"SELECT url, category, title, is_file, updated_on,
                  body IS NOT NULL AS "read!", fetch_error, needs_selection, mirror_text
           FROM rulebook_pages
           WHERE delisted_at IS NULL
           ORDER BY section_url, position"#,
    )
    .fetch_all(db)
    .await
}

pub struct FileListing {
    pub page_url: String,
    pub url: String,
    pub title: String,
    pub downloaded: bool,
    pub fetch_error: Option<String>,
    pub mirror: bool,
}

/// The listed files of every listed entry, in page order.
pub async fn list_files(db: &PgPool) -> Result<Vec<FileListing>, sqlx::Error> {
    sqlx::query_as!(
        FileListing,
        r#"SELECT f.page_url, f.url, f.title, f.fetched_at IS NOT NULL AS "downloaded!",
                  f.fetch_error, f.mirror
           FROM rulebook_files f
           JOIN rulebook_pages p ON p.url = f.page_url
           WHERE f.delisted_at IS NULL AND p.delisted_at IS NULL
           ORDER BY f.page_url, f.position"#,
    )
    .fetch_all(db)
    .await
}
