//! Mirrors SU's rulebook (Regelboken) into every credit transfer workspace;
//! see migration `20261105000001_rulebook.sql`, and `crate::rulebook_page`
//! for how its pages are read.
//!
//! Each scheduler tick:
//!
//! 1. reads the section indexes that are due (daily) and reconciles their
//!    entries,
//! 2. reads the pages that are due (daily): text, "Senast uppdaterad" and
//!    the files they link,
//! 3. downloads files not downloaded yet. Download URLs carry the upload
//!    time and are served as immutable, so a revised file arrives under a
//!    new URL and nothing is downloaded twice,
//! 4. decides, for each changed page whose files have all been tried,
//!    whether its own text or only its files are mirrored
//!    (`rulebook_page::select`),
//! 5. mirrors the chosen documents as ordinary documents, so the ingest
//!    worker chunks and embeds them like any upload. A changed page or a
//!    replaced file swaps its document; one that is dropped is orphaned
//!    out of retrieval.

use crate::rulebook_page;
use crate::state::AppState;

/// `documents.source_system` of the mirrored documents; `source_ref` is the
/// page or file URL.
pub const SOURCE_SYSTEM: &str = "su_rulebook";

/// How often section indexes and pages are read again.
pub const RECHECK_HOURS: i64 = 24;

/// Ceiling for the exponential retry backoff after failed reads.
const MAX_BACKOFF_HOURS: i32 = 24 * 7;

/// Pages and files fetched per tick, one after the other, so the first
/// sync of a section (about 30 pages and as many PDFs) spreads over a few
/// minutes instead of a burst against SU's web.
const PAGE_BATCH: i64 = 5;
const FILE_BATCH: i64 = 5;

/// Pages are about 400 KB of HTML, mostly menus.
const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;

/// The largest current PDF (a local qualifications ordinance) is under
/// 3 MB; this only guards against a broken host.
const MAX_PDF_BYTES: usize = 30 * 1024 * 1024;

const USER_AGENT: &str = "minerva-rulebook/1.0 (+https://minerva.dsv.su.se)";

#[derive(Debug, Default)]
pub struct TickStats {
    pub entries_added: u64,
    pub entries_delisted: u64,
    pub pages_read: usize,
    pub pages_changed: usize,
    pub files_fetched: usize,
    pub failed: usize,
    /// Pages whose mirrored documents were decided again.
    pub selected: usize,
    /// Documents added or replaced across credit transfer workspaces.
    pub mirrored: usize,
    /// Documents orphaned because they are no longer mirrored.
    pub retired: usize,
}

impl TickStats {
    pub fn is_empty(&self) -> bool {
        self.entries_added
            + self.entries_delisted
            + (self.pages_read + self.files_fetched + self.failed) as u64
            + (self.selected + self.mirrored + self.retired) as u64
            == 0
    }
}

pub fn rulebook_dir(docs_path: &str) -> String {
    format!("{}/rulebook", docs_path)
}

pub fn rulebook_pdf_path(docs_path: &str, content_hash: &str) -> String {
    format!("{}/{}.pdf", rulebook_dir(docs_path), content_hash)
}

/// One scheduler tick; see the module docs.
pub async fn run_tick(state: &AppState) -> Result<TickStats, crate::error::AppError> {
    let mut stats = TickStats::default();
    let sections = minerva_db::queries::rulebook::due_sections(&state.db).await?;
    let pages = minerva_db::queries::rulebook::due_pages(&state.db, PAGE_BATCH).await?;
    let files = minerva_db::queries::rulebook::due_files(&state.db, FILE_BATCH).await?;
    if !(sections.is_empty() && pages.is_empty() && files.is_empty()) {
        match reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(std::time::Duration::from_secs(60))
            .build()
        {
            Ok(client) => {
                read_sections(state, &client, &sections, &mut stats).await?;
                read_pages(state, &client, &pages, &mut stats).await?;
                fetch_files(state, &client, &files, &mut stats).await?;
            }
            Err(e) => tracing::error!("rulebook: http client init failed: {}", e),
        }
    }
    stats.selected = select_pending(state).await?;
    let docs = minerva_db::queries::rulebook::mirror_documents(&state.db).await?;
    for ws in minerva_db::queries::credit_transfer::list_active(&state.db).await? {
        mirror_into(state, ws.workspace_id, &docs, &mut stats).await?;
    }
    Ok(stats)
}

fn next_check() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() + chrono::Duration::hours(RECHECK_HOURS)
}

async fn read_sections(
    state: &AppState,
    client: &reqwest::Client,
    sections: &[String],
    stats: &mut TickStats,
) -> Result<(), sqlx::Error> {
    use minerva_db::queries::rulebook as q;
    for url in sections {
        let entries = read_index(client, url).await;
        match entries {
            Ok(entries) => {
                let inputs: Vec<q::EntryInput<'_>> = entries
                    .iter()
                    .map(|e| q::EntryInput {
                        url: &e.url,
                        title: &e.title,
                        category: &e.category,
                        is_file: e.is_file,
                    })
                    .collect();
                let summary = q::reconcile_section(&state.db, url, &inputs).await?;
                q::record_section_success(&state.db, url, next_check()).await?;
                stats.entries_added += summary.added + summary.relisted;
                stats.entries_delisted += summary.delisted;
            }
            Err(e) => {
                let failures =
                    q::record_section_failure(&state.db, url, &e, MAX_BACKOFF_HOURS).await?;
                stats.failed += 1;
                tracing::warn!(
                    "rulebook: index {} failed ({} in a row): {}",
                    url,
                    failures,
                    e
                );
            }
        }
    }
    Ok(())
}

/// An index that parses to nothing is an error, not an empty section: a
/// layout change must not delist every document.
async fn read_index(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<rulebook_page::Entry>, String> {
    let base = reqwest::Url::parse(url).map_err(|e| format!("bad url: {}", e))?;
    let html = fetch_html(client, url).await?;
    match rulebook_page::parse_index(&html, &base) {
        None => Err("no content area (page layout changed?)".to_string()),
        Some(entries) if entries.is_empty() => Err("index lists no documents".to_string()),
        Some(entries) => Ok(entries),
    }
}

async fn read_pages(
    state: &AppState,
    client: &reqwest::Client,
    pages: &[minerva_db::queries::rulebook::DuePage],
    stats: &mut TickStats,
) -> Result<(), sqlx::Error> {
    use minerva_db::queries::rulebook as q;
    for page in pages {
        match read_page(client, &page.url).await {
            Ok(read) => {
                let title = read.title.unwrap_or_else(|| page.title.clone());
                let document =
                    rulebook_page::render_document(&title, &page.category, &page.url, &read.body);
                let hash = minerva_pipeline::pipeline::compute_content_hash(document.as_bytes());
                let files: Vec<q::FileInput<'_>> = read
                    .files
                    .iter()
                    .map(|f| q::FileInput {
                        url: &f.url,
                        title: &f.title,
                    })
                    .collect();
                let summary = q::record_page_success(
                    &state.db,
                    &q::PageRead {
                        url: &page.url,
                        title: &title,
                        body: &read.body,
                        content_hash: &hash,
                        updated_on: read.updated_on,
                        files: &files,
                        next_check_at: next_check(),
                    },
                )
                .await?;
                stats.pages_read += 1;
                if summary.changed && page.content_hash.is_some() {
                    tracing::info!("rulebook: {} changed upstream", page.url);
                }
                if summary.changed {
                    stats.pages_changed += 1;
                }
            }
            Err(e) => {
                let failures =
                    q::record_page_failure(&state.db, &page.url, &e, MAX_BACKOFF_HOURS).await?;
                stats.failed += 1;
                tracing::warn!(
                    "rulebook: page {} failed ({} in a row): {}",
                    page.url,
                    failures,
                    e
                );
            }
        }
    }
    Ok(())
}

async fn read_page(client: &reqwest::Client, url: &str) -> Result<rulebook_page::Page, String> {
    let base = reqwest::Url::parse(url).map_err(|e| format!("bad url: {}", e))?;
    let html = fetch_html(client, url).await?;
    rulebook_page::parse_page(&html, &base)
        .ok_or_else(|| "no content area (page layout changed?)".to_string())
}

/// GET an HTML page, capped at `MAX_PAGE_BYTES`.
async fn fetch_html(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let mut resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "text/html")
        .send()
        .await
        .map_err(|e| format!("request: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("http {}", status.as_u16()));
    }
    let html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    if !html {
        return Err("response is not an HTML page".to_string());
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("body read: {}", e))?
    {
        if buf.len() + chunk.len() > MAX_PAGE_BYTES {
            return Err(format!("response exceeds {} byte cap", MAX_PAGE_BYTES));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

async fn fetch_files(
    state: &AppState,
    client: &reqwest::Client,
    urls: &[String],
    stats: &mut TickStats,
) -> Result<(), sqlx::Error> {
    use minerva_db::queries::rulebook as q;
    for url in urls {
        match fetch_and_store(client, &state.config.docs_path, url).await {
            Ok((hash, size, text)) => {
                q::record_file_success(&state.db, url, &hash, size, &text).await?;
                stats.files_fetched += 1;
            }
            Err(e) => {
                let failures =
                    q::record_file_failure(&state.db, url, &e, MAX_BACKOFF_HOURS).await?;
                stats.failed += 1;
                tracing::warn!(
                    "rulebook: file {} failed ({} in a row): {}",
                    url,
                    failures,
                    e
                );
            }
        }
    }
    Ok(())
}

/// Download a PDF to `{docs_path}/rulebook/{sha256}.pdf` and read its text
/// with page markers. A PDF without a text layer still counts as
/// downloaded, with empty text: the page it belongs to is then mirrored
/// as well.
async fn fetch_and_store(
    client: &reqwest::Client,
    docs_path: &str,
    url: &str,
) -> Result<(String, i64, String), String> {
    let bytes = crate::pdf_fetch::fetch_pdf(client, url, MAX_PDF_BYTES, USER_AGENT).await?;
    let hash = minerva_pipeline::pipeline::compute_content_hash(&bytes);
    let path = rulebook_pdf_path(docs_path, &hash);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        tokio::fs::create_dir_all(rulebook_dir(docs_path))
            .await
            .map_err(|e| format!("create dir: {}", e))?;
        // Write-then-rename so a reader never sees a half-written file
        // under its final content-hash name.
        let tmp = format!("{}.{}.tmp", path, uuid::Uuid::new_v4());
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|e| format!("write pdf: {}", e))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|e| format!("rename pdf: {}", e))?;
    }
    let pdf = std::path::PathBuf::from(&path);
    let text = match tokio::task::spawn_blocking(move || minerva_pipeline::pdf::extract_pages(&pdf))
        .await
    {
        Ok(Ok(pages)) => minerva_pipeline::pdf::mark_pages(&pages),
        Ok(Err(e)) => {
            tracing::warn!("rulebook: no text in {}: {}", url, e);
            String::new()
        }
        Err(e) => {
            tracing::warn!("rulebook: reading {} failed: {}", url, e);
            String::new()
        }
    };
    Ok((hash, bytes.len() as i64, text))
}

/// Decide what to mirror for every page that asks for it. Returns how many
/// pages were decided.
async fn select_pending(state: &AppState) -> Result<usize, sqlx::Error> {
    use minerva_db::queries::rulebook as q;
    let pages = q::pages_to_select(&state.db).await?;
    if pages.is_empty() {
        return Ok(0);
    }
    let urls: Vec<String> = pages.iter().map(|p| p.url.clone()).collect();
    let mut files: std::collections::HashMap<String, Vec<q::SelectionFile>> =
        std::collections::HashMap::new();
    for f in q::selection_files(&state.db, &urls).await? {
        files.entry(f.page_url.clone()).or_default().push(f);
    }
    for page in &pages {
        let page_files: Vec<(&str, &str)> = files
            .get(&page.url)
            .map(|fs| {
                fs.iter()
                    .map(|f| (f.url.as_str(), f.text.as_deref().unwrap_or("")))
                    .collect()
            })
            .unwrap_or_default();
        let selection = rulebook_page::select(page.body.as_deref(), &page_files);
        q::record_selection(
            &state.db,
            &page.url,
            selection.mirror_text,
            &selection.files,
        )
        .await?;
    }
    Ok(pages.len())
}

/// Make one workspace's rulebook documents match `docs`. Unchanged
/// documents cost one map lookup.
async fn mirror_into(
    state: &AppState,
    workspace_id: uuid::Uuid,
    docs: &[minerva_db::queries::rulebook::MirrorDocument],
    stats: &mut TickStats,
) -> Result<(), crate::error::AppError> {
    let Some(ws) = minerva_db::queries::workspaces::find_by_id(&state.db, workspace_id).await?
    else {
        return Ok(());
    };
    let existing = minerva_db::queries::documents::active_source_hashes(
        &state.db,
        workspace_id,
        SOURCE_SYSTEM,
    )
    .await?;
    let mut keep = Vec::with_capacity(docs.len());
    for d in docs {
        keep.push(d.url.clone());
        let current = existing.get(&d.url).and_then(|h| h.as_deref());
        if current == Some(d.content_hash.as_str()) {
            continue;
        }
        let (bytes, filename, mime) = if d.is_page {
            let Some(body) = minerva_db::queries::rulebook::page_body(&state.db, &d.url).await?
            else {
                continue;
            };
            let text = rulebook_page::render_document(&d.title, &d.category, &d.url, &body);
            (
                text.into_bytes(),
                document_filename(&d.title, "txt"),
                "text/plain",
            )
        } else {
            let path = rulebook_pdf_path(&state.config.docs_path, &d.content_hash);
            match tokio::fs::read(&path).await {
                Ok(b) => (b, document_filename(&d.title, "pdf"), "application/pdf"),
                Err(e) => {
                    tracing::warn!("rulebook: {} missing on disk ({}): {}", d.url, path, e);
                    continue;
                }
            }
        };
        // A page whose stored hash predates a change to how documents are
        // rendered: what it would be mirrored as is already there.
        let hash = minerva_pipeline::pipeline::compute_content_hash(&bytes);
        if current == Some(hash.as_str()) {
            continue;
        }
        let doc = crate::documents::upload_or_dedup(
            &state.db,
            &state.config.docs_path,
            workspace_id,
            &filename,
            mime,
            &bytes,
            ws.owner_id,
            None,
            Some(SOURCE_SYSTEM),
            Some(&d.url),
        )
        .await?;
        // Reference material for retrieval, like the course plans: no
        // classifier call, no risk of an `unknown` kind.
        minerva_db::queries::documents::set_kind_locked(&state.db, doc.id, "syllabus").await?;
        stats.mirrored += 1;
    }
    let retired = minerva_db::queries::documents::reconcile_active_source_refs(
        &state.db,
        workspace_id,
        SOURCE_SYSTEM,
        &keep,
    )
    .await?;
    stats.retired += retired.len();
    Ok(())
}

/// `Regler för salstentamen.pdf`, with anything that is not safe in a file
/// name replaced.
fn document_filename(title: &str, extension: &str) -> String {
    let clean: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | ',' | '.' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let clean: String = clean.chars().take(150).collect();
    format!("{clean}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::document_filename;

    #[test]
    fn filename_keeps_the_title_readable() {
        assert_eq!(
            document_filename("Regler för salstentamen", "pdf"),
            "Regler för salstentamen.pdf"
        );
        assert_eq!(
            document_filename("Mall för kursrapport (dnr SU FV-1-1-2-3424-20)", "pdf"),
            "Mall för kursrapport (dnr SU FV-1-1-2-3424-20).pdf"
        );
        let odd = document_filename("a/b: c?", "txt");
        assert!(
            !odd.contains('/') && !odd.contains(':') && !odd.contains('?'),
            "{odd}"
        );
    }
}
