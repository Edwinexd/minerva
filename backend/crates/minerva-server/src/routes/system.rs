use axum::extract::{Extension, State};
use axum::routing::get;
use axum::{Json, Router};
use minerva_core::models::User;
use serde::Serialize;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/system", get(system_metrics))
}

#[derive(Serialize)]
struct SystemMetrics {
    disk: Option<DiskInfo>,
    database: DatabaseInfo,
    documents: DocumentsInfo,
    qdrant: QdrantInfo,
}

#[derive(Serialize)]
struct DiskInfo {
    /// Path used to sample the mounted filesystem.
    path: String,
    total_bytes: u64,
    free_bytes: u64,
    used_bytes: u64,
}

#[derive(Serialize)]
struct DatabaseInfo {
    /// Size of the Postgres database in bytes (pg_database_size).
    size_bytes: Option<i64>,
    table_counts: Vec<TableCount>,
}

#[derive(Serialize)]
struct TableCount {
    name: String,
    rows: i64,
}

#[derive(Serialize)]
struct DocumentsInfo {
    /// Total rows in the documents table.
    count: i64,
    /// Sum of size_bytes across all documents.
    total_bytes: i64,
    /// Documents currently processing or pending.
    pending: i64,
    /// Documents in the failed state.
    failed: i64,
}

#[derive(Serialize)]
struct QdrantInfo {
    reachable: bool,
    collections: Vec<QdrantCollection>,
}

#[derive(Serialize)]
struct QdrantCollection {
    name: String,
    points_count: Option<u64>,
    indexed_vectors_count: Option<u64>,
    segments_count: Option<u64>,
}

async fn system_metrics(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<SystemMetrics>, AppError> {
    if !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let disk = disk_usage(&state.config.docs_path);
    let database = database_info(&state.db).await;
    let documents = documents_info(&state.db).await;
    let qdrant = qdrant_info(&state.qdrant).await;

    Ok(Json(SystemMetrics {
        disk,
        database,
        documents,
        qdrant,
    }))
}

#[cfg(unix)]
#[allow(clippy::unnecessary_cast)] // field widths vary by target (u32 vs u64); casts keep this portable
fn disk_usage(path: &str) -> Option<DiskInfo> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;

    let c_path = CString::new(path).ok()?;
    let mut stat: MaybeUninit<libc::statvfs> = MaybeUninit::uninit();
    // SAFETY: c_path is a valid C string; stat is a valid out-pointer.
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        tracing::warn!(
            "statvfs({}) failed: {}",
            path,
            std::io::Error::last_os_error()
        );
        return None;
    }
    // SAFETY: statvfs returned success, so stat is initialized.
    let stat = unsafe { stat.assume_init() };

    let frsize = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * frsize;
    // f_bavail: blocks available to unprivileged users; matches what df reports.
    let free = stat.f_bavail as u64 * frsize;
    let used = total.saturating_sub(free);

    Some(DiskInfo {
        path: path.to_string(),
        total_bytes: total,
        free_bytes: free,
        used_bytes: used,
    })
}

#[cfg(not(unix))]
fn disk_usage(_path: &str) -> Option<DiskInfo> {
    None
}

async fn database_info(db: &sqlx::PgPool) -> DatabaseInfo {
    let size_bytes: Option<i64> = sqlx::query_scalar("SELECT pg_database_size(current_database())")
        .fetch_optional(db)
        .await
        .ok()
        .flatten();

    // Fast approximate counts from pg_class.reltuples; accurate enough for an admin dashboard
    // and avoids a full scan on large tables.
    let table_counts = sqlx::query_as::<_, (String, f32)>(
        r#"SELECT relname::text, reltuples
        FROM pg_class
        WHERE relkind = 'r'
          AND relnamespace = 'public'::regnamespace
        ORDER BY relname"#,
    )
    .fetch_all(db)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(name, rows)| TableCount {
        name,
        rows: rows.max(0.0) as i64,
    })
    .collect();

    DatabaseInfo {
        size_bytes,
        table_counts,
    }
}

async fn documents_info(db: &sqlx::PgPool) -> DocumentsInfo {
    let row: Option<(i64, Option<i64>, i64, i64)> = sqlx::query_as(
        r#"SELECT
            COUNT(*)::bigint AS count,
            COALESCE(SUM(size_bytes), 0)::bigint AS total_bytes,
            COUNT(*) FILTER (WHERE status IN ('pending', 'processing', 'awaiting_transcript'))::bigint AS pending,
            COUNT(*) FILTER (WHERE status = 'failed')::bigint AS failed
        FROM documents"#,
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten();

    match row {
        Some((count, total_bytes, pending, failed)) => DocumentsInfo {
            count,
            total_bytes: total_bytes.unwrap_or(0),
            pending,
            failed,
        },
        None => DocumentsInfo {
            count: 0,
            total_bytes: 0,
            pending: 0,
            failed: 0,
        },
    }
}

async fn qdrant_info(qdrant: &qdrant_client::Qdrant) -> QdrantInfo {
    let list = match qdrant.list_collections().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("qdrant list_collections failed: {}", e);
            return QdrantInfo {
                reachable: false,
                collections: Vec::new(),
            };
        }
    };

    let mut collections = Vec::new();
    for desc in list.collections {
        let name = desc.name;
        match qdrant.collection_info(&name).await {
            Ok(info) => {
                let result = info.result;
                collections.push(QdrantCollection {
                    points_count: result.as_ref().and_then(|r| r.points_count),
                    indexed_vectors_count: result.as_ref().and_then(|r| r.indexed_vectors_count),
                    segments_count: result.as_ref().map(|r| r.segments_count),
                    name,
                });
            }
            Err(e) => {
                tracing::warn!("qdrant collection_info({}) failed: {}", name, e);
                collections.push(QdrantCollection {
                    name,
                    points_count: None,
                    indexed_vectors_count: None,
                    segments_count: None,
                });
            }
        }
    }

    QdrantInfo {
        reachable: true,
        collections,
    }
}
