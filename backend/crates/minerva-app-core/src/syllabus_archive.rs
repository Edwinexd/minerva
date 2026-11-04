//! Downloads course syllabus PDFs listed by the nightly Daisy sync (see
//! migration `20261104000001_syllabus_archive.sql`).
//!
//! Each scheduler tick fetches the versions that are due: never-downloaded
//! ones first, then periodic re-checks. The archive sends no ETag or
//! Last-Modified, so the only way to notice a PDF re-published under the
//! same URL is to download it again and compare hashes.
//!
//! Files are content-addressed at `{docs_path}/syllabi/{sha256}.pdf`: a
//! revision writes a new file and leaves the old bytes in place.
//!
//! Each tick then mirrors the current plans into every credit transfer
//! workspace as ordinary documents (one per plan and language), so the
//! ingest worker chunks and embeds them like any upload. A new version
//! replaces its predecessor through source identity; a plan that drops
//! out (delisted, discontinued) is orphaned out of retrieval.

use crate::state::AppState;

/// Versions handled per tick. Run sequentially, so a full first sync of
/// ~200 DSV plans spreads over a few minutes instead of a burst.
const BATCH_SIZE: i64 = 10;

/// How often an already-downloaded version is fetched again to catch silent
/// revisions.
pub const RECHECK_INTERVAL_DAYS: i64 = 7;

/// Ceiling for the exponential retry backoff after failed downloads.
const MAX_BACKOFF_HOURS: i32 = 24 * RECHECK_INTERVAL_DAYS as i32;

/// Syllabi are a few hundred KB; this only guards against a broken host
/// streaming something enormous.
const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;

const USER_AGENT: &str = "minerva-syllabus/1.0 (+https://minerva.dsv.su.se)";

/// `documents.source_system` of the mirrored plans; `source_ref` is
/// `{code}/{language}`, stable across versions.
pub const SOURCE_SYSTEM: &str = "syllabus_archive";

#[derive(Debug, Default)]
pub struct TickStats {
    pub fetched: usize,
    pub changed: usize,
    pub failed: usize,
    /// Plan documents added or replaced across credit transfer workspaces.
    pub mirrored: usize,
    /// Plan documents orphaned because the plan is no longer current.
    pub retired: usize,
}

pub fn syllabus_dir(docs_path: &str) -> String {
    format!("{}/syllabi", docs_path)
}

pub fn syllabus_pdf_path(docs_path: &str, content_hash: &str) -> String {
    format!("{}/{}.pdf", syllabus_dir(docs_path), content_hash)
}

/// One scheduler tick: download due versions, then mirror the current
/// plans into every credit transfer workspace.
pub async fn run_tick(state: &AppState) -> Result<TickStats, crate::error::AppError> {
    let mut stats = run_due(state).await?;
    let current = minerva_db::queries::syllabus_archive::current_versions(&state.db).await?;
    for ws in minerva_db::queries::credit_transfer::list_active(&state.db).await? {
        mirror_into(state, ws.workspace_id, &current, &mut stats).await?;
    }
    Ok(stats)
}

/// Fetch every due version once. Per-version failures are recorded on the
/// row and do not abort the batch; only a failing due-query is an error.
pub async fn run_due(state: &AppState) -> Result<TickStats, sqlx::Error> {
    let due =
        minerva_db::queries::syllabus_archive::find_due_versions(&state.db, BATCH_SIZE).await?;
    let mut stats = TickStats::default();
    if due.is_empty() {
        return Ok(stats);
    }

    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(60))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("syllabus archive: http client init failed: {}", e);
            return Ok(stats);
        }
    };

    for version in due {
        let label = format!(
            "{} {} ({})",
            version.code, version.valid_from, version.language
        );
        match fetch_and_store(&client, &state.config.docs_path, &version).await {
            Ok((hash, size)) => {
                let changed = version.content_hash.as_deref() != Some(hash.as_str());
                let next = chrono::Utc::now() + chrono::Duration::days(RECHECK_INTERVAL_DAYS);
                minerva_db::queries::syllabus_archive::record_fetch_success(
                    &state.db, version.id, &hash, size, changed, next,
                )
                .await?;
                stats.fetched += 1;
                if changed {
                    stats.changed += 1;
                    if version.content_hash.is_some() {
                        tracing::info!("syllabus archive: {} changed upstream", label);
                    }
                }
            }
            Err(e) => {
                let failures = minerva_db::queries::syllabus_archive::record_fetch_failure(
                    &state.db,
                    version.id,
                    &e,
                    MAX_BACKOFF_HOURS,
                )
                .await?;
                stats.failed += 1;
                tracing::warn!(
                    "syllabus archive: {} download failed ({} in a row): {}",
                    label,
                    failures,
                    e
                );
            }
        }
    }
    Ok(stats)
}

async fn fetch_and_store(
    client: &reqwest::Client,
    docs_path: &str,
    version: &minerva_db::queries::syllabus_archive::DueVersion,
) -> Result<(String, i64), String> {
    let bytes =
        crate::pdf_fetch::fetch_pdf(client, &version.pdf_url, MAX_PDF_BYTES, USER_AGENT).await?;
    let hash = minerva_pipeline::pipeline::compute_content_hash(&bytes);
    let path = syllabus_pdf_path(docs_path, &hash);
    if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
        tokio::fs::create_dir_all(syllabus_dir(docs_path))
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
    Ok((hash, bytes.len() as i64))
}

/// Make one workspace's plan documents match `current`. Unchanged plans
/// cost one map lookup; only new or revised PDFs are read from disk.
async fn mirror_into(
    state: &AppState,
    workspace_id: uuid::Uuid,
    current: &[minerva_db::queries::syllabus_archive::CurrentVersion],
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
    let mut keep = Vec::with_capacity(current.len());
    for v in current {
        let source_ref = format!("{}/{}", v.code, v.language);
        keep.push(source_ref.clone());
        if existing.get(&source_ref).and_then(|h| h.as_deref()) == Some(v.content_hash.as_str()) {
            continue;
        }
        let path = syllabus_pdf_path(&state.config.docs_path, &v.content_hash);
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    "syllabus archive: {} missing on disk ({}): {}",
                    source_ref,
                    path,
                    e
                );
                continue;
            }
        };
        let name = match (v.language.as_str(), v.name_en.as_deref()) {
            ("en", Some(en)) => en,
            _ => v.name_sv.as_str(),
        };
        let filename = document_filename(&v.code, name, &v.valid_from, &v.language);
        let doc = crate::documents::upload_or_dedup(
            &state.db,
            &state.config.docs_path,
            workspace_id,
            &filename,
            "application/pdf",
            &bytes,
            ws.owner_id,
            None,
            Some(SOURCE_SYSTEM),
            Some(&source_ref),
        )
        .await?;
        // Course plans are reference material for retrieval; no reason to
        // spend a classifier call or risk an `unknown` kind.
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

/// `IB440C Programmering 2 (HT2020, sv).pdf`, with anything that is not
/// safe in a filename replaced.
fn document_filename(code: &str, name: &str, valid_from: &str, language: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | ',' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let clean: String = clean.chars().take(100).collect();
    format!("{code} {clean} ({valid_from}, {language}).pdf")
}

#[cfg(test)]
mod tests {
    use super::document_filename;

    #[test]
    fn filename_names_the_plan_version_and_language() {
        assert_eq!(
            document_filename("IB440C", "Programmering 2", "HT2020", "sv"),
            "IB440C Programmering 2 (HT2020, sv).pdf"
        );
        let odd = document_filename("X1", "a/b: c?", "VT2015", "en");
        assert!(
            !odd.contains('/') && !odd.contains(':') && !odd.contains('?'),
            "{odd}"
        );
    }
}
