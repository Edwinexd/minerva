//! Canvas LMS sync engine: discovery (Files API + Modules) and the
//! per-item materialisation of files / pages / external URLs into Minerva
//! documents.
//!
//! This is the axum-free core. The HTTP route handlers (list/create/delete
//! connections, preview, manual "Sync now" trigger) live in
//! `minerva-server`'s `routes::canvas` and call into this module; the
//! periodic auto-sync loop in `crate::schedulers` calls the same
//! [`run_sync`]. Keeping the engine here lets the `minerva-scheduler`
//! binary run Canvas auto-sync without linking the api's route tree.
//!
//! Identity & dedup. `canvas_sync_log.canvas_file_id` is the per-connection
//! idempotency key. We prefix it to keep namespaces disjoint:
//! `file:{canvas_file_id}`, `page:{canvas_page_id}`, `url:{absolute_url}`.
//! Re-sync triggers when Canvas's `updated_at` advances past the stored
//! `canvas_updated_at` (Files and Pages). ExternalUrls carry no timestamp
//! and stay skip-once unless the underlying `source_url` changes.

use chrono::{DateTime, Utc};
use qdrant_client::qdrant::DeletePointsBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

use crate::error::{AppError, LocalizedMessage};
use crate::state::AppState;
use minerva_pipeline::pipeline::{compute_content_hash, extension_from_filename};

// ---------------------------------------------------------------------------
// Canvas API: raw wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Clone)]
struct CanvasFile {
    id: i64,
    display_name: String,
    filename: String,
    #[serde(rename = "content-type")]
    content_type: Option<String>,
    size: i64,
    url: Option<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    locked: bool,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct CanvasModule {
    #[serde(default)]
    items: Vec<CanvasModuleItem>,
}

#[derive(Debug, Deserialize)]
struct CanvasModuleItem {
    #[serde(rename = "type")]
    item_type: String,
    title: Option<String>,
    /// File ID for File items, Page id (numeric) for Page items.
    content_id: Option<i64>,
    /// Page slug for Page items.
    page_url: Option<String>,
    /// Absolute URL for ExternalUrl items.
    external_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CanvasPage {
    title: String,
    body: Option<String>,
    updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    published: bool,
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn canvas_get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    api_token: &str,
) -> Result<(T, Option<String>), AppError> {
    let resp = http
        .get(url)
        .header("Authorization", format!("Bearer {}", api_token))
        .send()
        .await
        .map_err(|e| {
            AppError::bad_request_with("canvas.api_request_failed", [("detail", e.to_string())])
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(AppError::bad_request_with(
            "canvas.api_error",
            [("status", status.to_string()), ("body", body)],
        ));
    }

    let link_header = resp
        .headers()
        .get("link")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let parsed: T = resp.json().await.map_err(|e| {
        AppError::bad_request_with("canvas.api_parse_failed", [("detail", e.to_string())])
    })?;

    Ok((parsed, next_page_url(&link_header)))
}

fn next_page_url(link_header: &Option<String>) -> Option<String> {
    let header = link_header.as_ref()?;
    for part in header.split(',') {
        let part = part.trim();
        if part.contains("rel=\"next\"") {
            let start = part.find('<')? + 1;
            let end = part.find('>')?;
            return Some(part[start..end].to_string());
        }
    }
    None
}

/// Paginated GET that follows Canvas's `Link: rel="next"` header until
/// exhausted. Public so the api's `lookup_courses` route can list a
/// teacher's Canvas courses with the same pagination machinery.
pub async fn paginate_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    first_url: String,
    api_token: &str,
) -> Result<Vec<T>, AppError> {
    let mut out = Vec::new();
    let mut url = first_url;
    loop {
        let (page, next): (Vec<T>, _) = canvas_get_json(http, &url, api_token).await?;
        out.extend(page);
        match next {
            Some(n) => url = n,
            None => break,
        }
    }
    Ok(out)
}

async fn fetch_canvas_files(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
) -> Result<Vec<CanvasFile>, AppError> {
    let base = base_url.trim_end_matches('/');
    let url = format!(
        "{}/api/v1/courses/{}/files?per_page=100",
        base, canvas_course_id
    );
    paginate_json(http, url, api_token).await
}

async fn fetch_canvas_modules(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
) -> Result<Vec<CanvasModule>, AppError> {
    let base = base_url.trim_end_matches('/');
    let url = format!(
        "{}/api/v1/courses/{}/modules?include[]=items&per_page=100",
        base, canvas_course_id
    );
    paginate_json(http, url, api_token).await
}

async fn fetch_file_by_id(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    file_id: i64,
) -> Result<CanvasFile, AppError> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{}/api/v1/files/{}", base, file_id);
    let (file, _) = canvas_get_json::<CanvasFile>(http, &url, api_token).await?;
    Ok(file)
}

async fn fetch_canvas_page(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
    page_url_slug: &str,
) -> Result<CanvasPage, AppError> {
    let base = base_url.trim_end_matches('/');
    let url = format!(
        "{}/api/v1/courses/{}/pages/{}",
        base, canvas_course_id, page_url_slug
    );
    let (page, _) = canvas_get_json::<CanvasPage>(http, &url, api_token).await?;
    Ok(page)
}

async fn download_canvas_file(
    http: &reqwest::Client,
    api_token: &str,
    download_url: &str,
) -> Result<Vec<u8>, AppError> {
    let resp = http
        .get(download_url)
        .header("Authorization", format!("Bearer {}", api_token))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Canvas file download failed: {}", e)))?;

    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "Canvas file download error: {}",
            resp.status()
        )));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::Internal(format!("Canvas file read error: {}", e)))?;
    Ok(bytes.to_vec())
}

// ---------------------------------------------------------------------------
// Discovery: merge Files API + Modules into a single item list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    File,
    Page,
    Url,
}

/// One resource discovered from Canvas (Files API and/or Modules tree),
/// keyed by its prefixed identity. Fields are public so the api's preview
/// route can render them; the engine populates them during [`discover_items`].
#[derive(Debug, Clone)]
pub struct DiscoveredItem {
    /// canvas_sync_log.canvas_file_id; prefixed identity key.
    pub key: String,
    pub kind: ItemKind,
    pub display_name: String,
    pub content_type: Option<String>,
    pub size_hint: i64,
    pub updated_at: Option<DateTime<Utc>>,
    /// Seen in {"files_api","modules"}; drives the preview "origin" chips.
    pub sources: BTreeSet<&'static str>,
    // File-only
    file: Option<CanvasFile>,
    // Page-only
    page_slug: Option<String>,
    // Url-only
    external_url: Option<String>,
}

/// Result of [`discover_items`]: the merged item list plus any per-source
/// warnings (e.g. the Files tab 403'd but Modules succeeded).
pub struct Discovery {
    pub items: Vec<DiscoveredItem>,
    pub warnings: Vec<LocalizedMessage>,
}

/// Query both sources concurrently; per-source failures become warnings.
/// If BOTH sources fail, `items` is empty but `warnings` describes why so
/// the teacher sees something useful instead of an opaque error.
pub async fn discover_items(
    http: &reqwest::Client,
    base_url: &str,
    api_token: &str,
    canvas_course_id: &str,
) -> Discovery {
    let files_fut = fetch_canvas_files(http, base_url, api_token, canvas_course_id);
    let modules_fut = fetch_canvas_modules(http, base_url, api_token, canvas_course_id);
    let (files_res, modules_res) = tokio::join!(files_fut, modules_fut);

    let mut warnings = Vec::new();
    let mut by_key: HashMap<String, DiscoveredItem> = HashMap::new();

    match files_res {
        Ok(files) => {
            for f in files {
                if f.hidden || f.locked {
                    continue;
                }
                let key = format!("file:{}", f.id);
                let entry = by_key.entry(key.clone()).or_insert_with(|| DiscoveredItem {
                    key: key.clone(),
                    kind: ItemKind::File,
                    display_name: f.display_name.clone(),
                    content_type: f.content_type.clone(),
                    size_hint: f.size,
                    updated_at: f.updated_at,
                    sources: BTreeSet::new(),
                    file: Some(f.clone()),
                    page_slug: None,
                    external_url: None,
                });
                entry.sources.insert("files_api");
                if entry.file.is_none() {
                    entry.file = Some(f);
                }
            }
        }
        Err(e) => {
            warnings.push(LocalizedMessage::with(
                "canvas.warning.files_unavailable",
                [("detail", e.to_string())],
            ));
        }
    }

    let modules_ok = modules_res.is_ok();
    match modules_res {
        Ok(modules) => {
            for module in modules {
                for item in module.items {
                    match item.item_type.as_str() {
                        "File" => {
                            let Some(fid) = item.content_id else {
                                continue;
                            };
                            let key = format!("file:{}", fid);
                            let entry =
                                by_key.entry(key.clone()).or_insert_with(|| DiscoveredItem {
                                    key: key.clone(),
                                    kind: ItemKind::File,
                                    display_name: item
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| format!("Canvas file {}", fid)),
                                    content_type: None,
                                    size_hint: 0,
                                    updated_at: None,
                                    sources: BTreeSet::new(),
                                    file: None,
                                    page_slug: None,
                                    external_url: None,
                                });
                            entry.sources.insert("modules");
                        }
                        "Page" => {
                            let Some(slug) = item.page_url else {
                                continue;
                            };
                            let key = format!("page:{}", slug);
                            let entry =
                                by_key.entry(key.clone()).or_insert_with(|| DiscoveredItem {
                                    key: key.clone(),
                                    kind: ItemKind::Page,
                                    display_name: item
                                        .title
                                        .clone()
                                        .unwrap_or_else(|| slug.clone()),
                                    content_type: Some("text/html".into()),
                                    size_hint: 0,
                                    updated_at: None,
                                    sources: BTreeSet::new(),
                                    file: None,
                                    page_slug: Some(slug.clone()),
                                    external_url: None,
                                });
                            entry.sources.insert("modules");
                        }
                        "ExternalUrl" => {
                            let Some(url) = item.external_url else {
                                continue;
                            };
                            let key = format!("url:{}", url);
                            let entry =
                                by_key.entry(key.clone()).or_insert_with(|| DiscoveredItem {
                                    key: key.clone(),
                                    kind: ItemKind::Url,
                                    display_name: item.title.clone().unwrap_or_else(|| url.clone()),
                                    content_type: Some("text/x-url".into()),
                                    size_hint: url.len() as i64,
                                    updated_at: None,
                                    sources: BTreeSet::new(),
                                    file: None,
                                    page_slug: None,
                                    external_url: Some(url.clone()),
                                });
                            entry.sources.insert("modules");
                        }
                        _ => {} // SubHeader / Assignment / Quiz / Discussion / ExternalTool; ignored
                    }
                }
            }
        }
        Err(e) => {
            warnings.push(LocalizedMessage::with(
                "canvas.warning.modules_unavailable",
                [("detail", e.to_string())],
            ));
        }
    }

    if !modules_ok && by_key.is_empty() {
        // Both sources failed. warnings already carries the reasons.
    }

    let mut items: Vec<DiscoveredItem> = by_key.into_values().collect();
    items.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
    });

    Discovery { items, warnings }
}

// ---------------------------------------------------------------------------
// Sync
// ---------------------------------------------------------------------------

/// Outcome of a single Canvas sync run, surfaced to the api's "Sync now"
/// response and logged by the periodic auto-sync loop. Per-item failures
/// are collected in `errors`; only infra failures bubble up as `AppError`.
#[derive(Serialize)]
pub struct SyncResult {
    pub synced: usize,
    pub resynced: usize,
    pub skipped: usize,
    pub errors: Vec<LocalizedMessage>,
    pub warnings: Vec<LocalizedMessage>,
}

/// Core sync routine, callable from both the manual HTTP trigger and the
/// background auto-sync loop. Performs discovery, iterates items, updates
/// `last_synced_at`. Errors are per-item (returned in `SyncResult.errors`);
/// only infra failures (e.g. course row missing) bubble up.
pub async fn run_sync(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
) -> Result<SyncResult, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, conn.course_id)
        .await?
        .ok_or(AppError::NotFound)?;

    let disc = discover_items(
        &state.http_client,
        &conn.canvas_base_url,
        &conn.canvas_api_token,
        &conn.canvas_course_id,
    )
    .await;

    let existing = minerva_db::queries::canvas::synced_log_by_canvas_id(&state.db, conn.id).await?;

    let mut result = SyncResult {
        synced: 0,
        resynced: 0,
        skipped: 0,
        errors: Vec::new(),
        warnings: disc.warnings,
    };

    for item in disc.items {
        let prev = existing.get(&item.key);
        let outcome = match item.kind {
            ItemKind::File => sync_file(state, conn, course.owner_id, &item, prev).await,
            ItemKind::Page => sync_page(state, conn, course.owner_id, &item, prev).await,
            ItemKind::Url => {
                sync_url(state, conn, course.owner_id, conn.course_id, &item, prev).await
            }
        };

        match outcome {
            Ok(Outcome::Created) => result.synced += 1,
            Ok(Outcome::Resynced) => result.resynced += 1,
            Ok(Outcome::Skipped) => result.skipped += 1,
            Err(e) => result.errors.push(LocalizedMessage::with(
                "canvas.sync.item_failed",
                [
                    ("item", item.display_name.clone()),
                    ("detail", e.to_string()),
                ],
            )),
        }
    }

    let _ = minerva_db::queries::canvas::update_last_synced(&state.db, conn.id).await;

    Ok(result)
}

enum Outcome {
    Created,
    Resynced,
    Skipped,
}

fn needs_resync(
    prev: Option<&minerva_db::queries::canvas::SyncLogRow>,
    latest: Option<DateTime<Utc>>,
) -> bool {
    let Some(row) = prev else {
        return false;
    };
    match (row.canvas_updated_at, latest) {
        (Some(p), Some(l)) => l > p,
        (None, Some(_)) => true,
        _ => false,
    }
}

async fn purge_chunks(state: &AppState, course_id: Uuid, doc_id: Uuid) -> Result<(), AppError> {
    // Look up the live collection name from the course's
    // embedding_version. Canvas resync runs on a sweeper, not the
    // hot chat path, so the extra DB roundtrip is fine; and it's
    // the only safe option since `purge_chunks` is invoked from
    // multiple call sites (file vs page replace) without a course
    // row in scope.
    let collection = minerva_pipeline::pipeline::collection_name_for_course(&state.db, course_id)
        .await
        .map_err(|e| AppError::Internal(format!("course lookup failed: {}", e)))?;
    let exists = state
        .qdrant
        .collection_exists(&collection)
        .await
        .unwrap_or(false);
    if !exists {
        return Ok(());
    }
    let filter = qdrant_client::qdrant::Filter::must([qdrant_client::qdrant::Condition::matches(
        "document_id",
        doc_id.to_string(),
    )]);
    state
        .qdrant
        .delete_points(
            DeletePointsBuilder::new(&collection)
                .points(filter)
                .wait(true),
        )
        .await
        .map_err(|e| AppError::Internal(format!("qdrant delete failed: {}", e)))?;
    Ok(())
}

async fn sync_file(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
    owner_id: Uuid,
    item: &DiscoveredItem,
    prev: Option<&minerva_db::queries::canvas::SyncLogRow>,
) -> Result<Outcome, AppError> {
    // Canvas file id from the prefixed key.
    let file_id: i64 = item
        .key
        .strip_prefix("file:")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| AppError::Internal(format!("bad canvas file key {}", item.key)))?;

    // If only Modules listed it, pull the full file record now.
    let file = match &item.file {
        Some(f) => f.clone(),
        None => fetch_file_by_id(
            &state.http_client,
            &conn.canvas_base_url,
            &conn.canvas_api_token,
            file_id,
        )
        .await
        .map_err(|e| {
            AppError::bad_request_with(
                "canvas.fetch_file_metadata_failed",
                [("detail", e.to_string())],
            )
        })?,
    };

    if file.hidden || file.locked {
        return Ok(Outcome::Skipped);
    }
    let Some(download_url) = &file.url else {
        return Ok(Outcome::Skipped);
    };

    let resync = needs_resync(prev, file.updated_at);
    if prev.is_some() && !resync {
        return Ok(Outcome::Skipped);
    }

    let data =
        download_canvas_file(&state.http_client, &conn.canvas_api_token, download_url).await?;
    let size_bytes = data.len() as i64;
    if size_bytes > crate::system_defaults::max_upload_bytes(&state.db).await {
        return Err(AppError::bad_request_with(
            "canvas.file_too_large",
            [("size_bytes", size_bytes.to_string())],
        ));
    }

    let content_type = file
        .content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    let dir = format!("{}/{}", state.config.docs_path, conn.course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {}", e)))?;
    let ext = extension_from_filename(&file.filename);

    if resync {
        let prev = prev.unwrap();
        let Some(doc_id) = prev.minerva_document_id else {
            // Log says synced but the doc row is gone; fall through to create.
            return create_file_doc(state, conn, owner_id, item, &file, &data, content_type, ext)
                .await;
        };
        purge_chunks(state, conn.course_id, doc_id).await?;
        let file_path = format!("{}/{}.{}", dir, doc_id, ext);
        tokio::fs::write(&file_path, &data)
            .await
            .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;
        let content_hash = compute_content_hash(&data);
        minerva_db::queries::documents::reset_for_resync(
            &state.db,
            doc_id,
            &file.display_name,
            content_type,
            size_bytes,
            Some(&content_hash),
        )
        .await?;
        minerva_db::queries::canvas::upsert_sync_log(
            &state.db,
            Uuid::new_v4(),
            conn.id,
            &item.key,
            &file.display_name,
            Some(content_type),
            Some(doc_id),
            file.updated_at,
        )
        .await?;
        return Ok(Outcome::Resynced);
    }

    create_file_doc(state, conn, owner_id, item, &file, &data, content_type, ext).await
}

#[allow(clippy::too_many_arguments)]
async fn create_file_doc(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
    owner_id: Uuid,
    item: &DiscoveredItem,
    file: &CanvasFile,
    data: &[u8],
    content_type: &str,
    ext: &str,
) -> Result<Outcome, AppError> {
    let doc_id = Uuid::new_v4();
    let dir = format!("{}/{}", state.config.docs_path, conn.course_id);
    let file_path = format!("{}/{}.{}", dir, doc_id, ext);
    tokio::fs::write(&file_path, data)
        .await
        .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;

    let content_hash = compute_content_hash(data);
    minerva_db::queries::documents::insert(
        &state.db,
        minerva_db::queries::documents::NewDocument {
            id: doc_id,
            course_id: conn.course_id,
            filename: &file.display_name,
            mime_type: content_type,
            size_bytes: data.len() as i64,
            uploaded_by: owner_id,
            source_url: None,
            content_hash: Some(&content_hash),
            source_system: None,
            source_ref: None,
            parent_document_id: None,
        },
    )
    .await?;

    minerva_db::queries::canvas::upsert_sync_log(
        &state.db,
        Uuid::new_v4(),
        conn.id,
        &item.key,
        &file.display_name,
        Some(content_type),
        Some(doc_id),
        file.updated_at,
    )
    .await?;

    Ok(Outcome::Created)
}

async fn sync_page(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
    owner_id: Uuid,
    item: &DiscoveredItem,
    prev: Option<&minerva_db::queries::canvas::SyncLogRow>,
) -> Result<Outcome, AppError> {
    let Some(slug) = &item.page_slug else {
        return Err(AppError::Internal("page item missing slug".into()));
    };

    let page = fetch_canvas_page(
        &state.http_client,
        &conn.canvas_base_url,
        &conn.canvas_api_token,
        &conn.canvas_course_id,
        slug,
    )
    .await?;

    if !page.published {
        return Ok(Outcome::Skipped);
    }

    let resync = needs_resync(prev, page.updated_at);
    if prev.is_some() && !resync {
        return Ok(Outcome::Skipped);
    }

    let body = page.body.clone().unwrap_or_default();
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body><h1>{}</h1>{}</body></html>",
        html_escape(&page.title),
        html_escape(&page.title),
        body
    );
    let data = html.into_bytes();
    let size_bytes = data.len() as i64;
    if size_bytes > crate::system_defaults::max_upload_bytes(&state.db).await {
        return Err(AppError::bad_request_with(
            "canvas.page_too_large",
            [("size_bytes", size_bytes.to_string())],
        ));
    }

    let filename = format!("{}.html", safe_filename(&page.title));
    let content_type = "text/html";
    let dir = format!("{}/{}", state.config.docs_path, conn.course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {}", e)))?;

    if resync {
        let prev = prev.unwrap();
        if let Some(doc_id) = prev.minerva_document_id {
            purge_chunks(state, conn.course_id, doc_id).await?;
            let file_path = format!("{}/{}.html", dir, doc_id);
            tokio::fs::write(&file_path, &data)
                .await
                .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;
            let content_hash = compute_content_hash(&data);
            minerva_db::queries::documents::reset_for_resync(
                &state.db,
                doc_id,
                &filename,
                content_type,
                size_bytes,
                Some(&content_hash),
            )
            .await?;
            minerva_db::queries::canvas::upsert_sync_log(
                &state.db,
                Uuid::new_v4(),
                conn.id,
                &item.key,
                &filename,
                Some(content_type),
                Some(doc_id),
                page.updated_at,
            )
            .await?;
            return Ok(Outcome::Resynced);
        }
    }

    let doc_id = Uuid::new_v4();
    let file_path = format!("{}/{}.html", dir, doc_id);
    tokio::fs::write(&file_path, &data)
        .await
        .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;

    let content_hash = compute_content_hash(&data);
    minerva_db::queries::documents::insert(
        &state.db,
        minerva_db::queries::documents::NewDocument {
            id: doc_id,
            course_id: conn.course_id,
            filename: &filename,
            mime_type: content_type,
            size_bytes,
            uploaded_by: owner_id,
            source_url: None,
            content_hash: Some(&content_hash),
            source_system: None,
            source_ref: None,
            parent_document_id: None,
        },
    )
    .await?;

    minerva_db::queries::canvas::upsert_sync_log(
        &state.db,
        Uuid::new_v4(),
        conn.id,
        &item.key,
        &filename,
        Some(content_type),
        Some(doc_id),
        page.updated_at,
    )
    .await?;

    Ok(Outcome::Created)
}

async fn sync_url(
    state: &AppState,
    conn: &minerva_db::queries::canvas::ConnectionRow,
    owner_id: Uuid,
    course_id: Uuid,
    item: &DiscoveredItem,
    prev: Option<&minerva_db::queries::canvas::SyncLogRow>,
) -> Result<Outcome, AppError> {
    let Some(url) = &item.external_url else {
        return Err(AppError::Internal("url item missing url".into()));
    };

    // The sync log already covers per-connection idempotency. The
    // documents.source_url unique index additionally protects against
    // a manually-added URL doc colliding with the Canvas-sourced one.
    if prev.is_some() {
        return Ok(Outcome::Skipped);
    }

    if let Some(existing) =
        minerva_db::queries::documents::find_by_course_source_url(&state.db, course_id, url).await?
    {
        minerva_db::queries::canvas::upsert_sync_log(
            &state.db,
            Uuid::new_v4(),
            conn.id,
            &item.key,
            &existing.filename,
            Some("text/x-url"),
            Some(existing.id),
            None,
        )
        .await?;
        return Ok(Outcome::Skipped);
    }

    let doc_id = Uuid::new_v4();
    let dir = format!("{}/{}", state.config.docs_path, course_id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Internal(format!("mkdir failed: {}", e)))?;
    let file_path = format!("{}/{}.url", dir, doc_id);
    tokio::fs::write(&file_path, url.as_bytes())
        .await
        .map_err(|e| AppError::Internal(format!("write failed: {}", e)))?;

    let filename = format!("{}.url", safe_filename(&item.display_name));
    let content_hash = compute_content_hash(url.as_bytes());
    let insert = minerva_db::queries::documents::insert(
        &state.db,
        minerva_db::queries::documents::NewDocument {
            id: doc_id,
            course_id,
            filename: &filename,
            mime_type: "text/x-url",
            size_bytes: url.len() as i64,
            uploaded_by: owner_id,
            source_url: Some(url.as_str()),
            content_hash: Some(&content_hash),
            source_system: None,
            source_ref: None,
            parent_document_id: None,
        },
    )
    .await;

    let row = match insert {
        Ok(row) => row,
        Err(sqlx::Error::Database(db_err)) if db_err.is_unique_violation() => {
            let _ = tokio::fs::remove_file(&file_path).await;
            let existing = minerva_db::queries::documents::find_by_course_source_url(
                &state.db, course_id, url,
            )
            .await?
            .ok_or_else(|| {
                AppError::Internal("unique violation on source_url but no match".into())
            })?;
            minerva_db::queries::canvas::upsert_sync_log(
                &state.db,
                Uuid::new_v4(),
                conn.id,
                &item.key,
                &existing.filename,
                Some("text/x-url"),
                Some(existing.id),
                None,
            )
            .await?;
            return Ok(Outcome::Skipped);
        }
        Err(e) => return Err(e.into()),
    };

    minerva_db::queries::canvas::upsert_sync_log(
        &state.db,
        Uuid::new_v4(),
        conn.id,
        &item.key,
        &row.filename,
        Some("text/x-url"),
        Some(row.id),
        None,
    )
    .await?;

    Ok(Outcome::Created)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn safe_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "untitled".into()
    } else if cleaned.len() > 180 {
        cleaned[..180].to_string()
    } else {
        cleaned.to_string()
    }
}
