//! Figures (slide images, document diagrams) extracted by DeepSeek-OCR
//! and linked to their source document. Schema: see migration
//! `20260506000002_document_figures.sql`.

use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, serde::Serialize)]
pub struct FigureRow {
    pub id: Uuid,
    pub document_id: Uuid,
    /// 1-based PDF page; NULL for video-derived figures.
    pub page: Option<i32>,
    /// Video timeline span start in seconds; NULL for PDF-derived figures.
    pub t_start_seconds: Option<f32>,
    pub t_end_seconds: Option<f32>,
    /// {x,y,w,h} normalized 0..1 within the OCRed (post-crop) image.
    pub bbox: Option<serde_json::Value>,
    pub caption: Option<String>,
    pub storage_path: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
pub struct NewFigure<'a> {
    pub id: Uuid,
    pub document_id: Uuid,
    pub page: Option<i32>,
    pub t_start_seconds: Option<f32>,
    pub t_end_seconds: Option<f32>,
    pub bbox: Option<&'a serde_json::Value>,
    pub caption: Option<&'a str>,
    pub storage_path: &'a str,
}

/// Insert a single figure row. Caller is expected to have written the PNG
/// bytes to `storage_path` already (or to do so right after, since we don't
/// want a doc transaction blocking on filesystem latency).
pub async fn insert(db: &PgPool, fig: NewFigure<'_>) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO document_figures
               (id, document_id, page, t_start_seconds, t_end_seconds,
                bbox, caption, storage_path)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        fig.id,
        fig.document_id,
        fig.page,
        fig.t_start_seconds,
        fig.t_end_seconds,
        fig.bbox,
        fig.caption,
        fig.storage_path,
    )
    .execute(db)
    .await?;
    Ok(())
}

pub async fn list_by_document(
    db: &PgPool,
    document_id: Uuid,
) -> Result<Vec<FigureRow>, sqlx::Error> {
    sqlx::query_as!(
        FigureRow,
        r#"SELECT id, document_id, page, t_start_seconds, t_end_seconds,
                  bbox, caption, storage_path, created_at
           FROM document_figures
           WHERE document_id = $1
           ORDER BY page NULLS LAST, t_start_seconds NULLS LAST, created_at"#,
        document_id,
    )
    .fetch_all(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<FigureRow>, sqlx::Error> {
    sqlx::query_as!(
        FigureRow,
        r#"SELECT id, document_id, page, t_start_seconds, t_end_seconds,
                  bbox, caption, storage_path, created_at
           FROM document_figures
           WHERE id = $1"#,
        id,
    )
    .fetch_optional(db)
    .await
}

/// Wipe all figure rows for a document. Used when re-OCRing so old figure
/// rows don't accumulate alongside new ones. Caller is responsible for
/// removing the on-disk PNGs (or letting the janitor do it later).
pub async fn delete_by_document(db: &PgPool, document_id: Uuid) -> Result<u64, sqlx::Error> {
    let result = sqlx::query!(
        "DELETE FROM document_figures WHERE document_id = $1",
        document_id,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
