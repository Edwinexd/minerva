//! Pages of a credit transfer workspace beyond the chat: the summary its
//! home card and header show, the viewer's own cases, the DSV plans it
//! compares against and SU's rules it draws on. Mounted under
//! `/api/workspaces/{workspace_id}`; any other workspace kind 404s.

use std::collections::HashMap;

use axum::extract::{Extension, Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use minerva_core::models::{User, WorkspaceKind};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/credit-transfer", get(summary))
        .route("/credit-transfer/cases", get(list_cases))
        .route("/credit-transfer/plans", get(list_plans))
        .route("/credit-transfer/rules", get(list_rules))
}

/// Same access as the chat (`workspace_access`), and only this kind.
async fn credit_transfer_workspace(
    state: &AppState,
    workspace_id: Uuid,
    user: &User,
) -> Result<minerva_db::queries::credit_transfer::CreditTransferRow, AppError> {
    let workspace = super::chat::verify_course_access_pub(state, workspace_id, user.id).await?;
    if WorkspaceKind::parse(&workspace.kind) != Some(WorkspaceKind::CreditTransfer) {
        return Err(AppError::NotFound);
    }
    minerva_db::queries::credit_transfer::find(&state.db, workspace_id)
        .await?
        .ok_or(AppError::NotFound)
}

#[derive(Serialize)]
struct SummaryResponse {
    plan_count: i64,
    plans_changed_this_week: i64,
    plans_checked_at: Option<DateTime<Utc>>,
    my_case_count: i64,
    case_retention_days: i32,
}

async fn summary(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<SummaryResponse>, AppError> {
    let ct = credit_transfer_workspace(&state, workspace_id, &user).await?;
    let s = minerva_db::queries::credit_transfer::summary(&state.db, workspace_id, user.id).await?;
    Ok(Json(SummaryResponse {
        plan_count: s.plan_count,
        plans_changed_this_week: s.plans_changed_this_week,
        plans_checked_at: s.plans_checked_at,
        my_case_count: s.my_case_count,
        case_retention_days: ct.case_retention_days,
    }))
}

#[derive(Serialize)]
struct CaseResponse {
    id: Uuid,
    title: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    /// When the retention sweep deletes the case unless it is used again.
    deleted_on: DateTime<Utc>,
    attachments: Vec<String>,
    best_match: Option<BestMatch>,
    candidate_count: Option<i32>,
}

#[derive(Serialize)]
struct BestMatch {
    code: String,
    verdict: String,
}

/// The caller's own cases. Nobody else's are listed, admins included:
/// cases are private to their author.
async fn list_cases(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<Vec<CaseResponse>>, AppError> {
    let ct = credit_transfer_workspace(&state, workspace_id, &user).await?;
    let retention = chrono::Duration::days(ct.case_retention_days.into());
    let rows =
        minerva_db::queries::credit_transfer::list_cases(&state.db, workspace_id, user.id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| CaseResponse {
                id: r.id,
                title: r.title,
                created_at: r.created_at,
                updated_at: r.updated_at,
                deleted_on: r.updated_at + retention,
                attachments: r.attachment_names,
                best_match: r
                    .best_code
                    .zip(r.best_verdict)
                    .map(|(code, verdict)| BestMatch { code, verdict }),
                candidate_count: r.candidate_count,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
struct PlanResponse {
    code: String,
    name_sv: String,
    name_en: Option<String>,
    ects: Option<f32>,
    level: Option<String>,
    /// The version assessments compare against, or `None` when the plan
    /// has no listed version.
    latest: Option<LatestVersion>,
    /// Older versions, newest first, by the term they took effect.
    older_versions: Vec<String>,
    /// `indexed`, `indexing`, `pending` (not downloaded or mirrored yet),
    /// `failed` or `discontinued`.
    status: &'static str,
    /// The current version arrived or changed in the last seven days.
    changed_this_week: bool,
}

#[derive(Serialize)]
struct LatestVersion {
    valid_from: String,
    files: Vec<PlanFile>,
}

#[derive(Serialize)]
struct PlanFile {
    language: String,
    pdf_url: String,
}

#[derive(Deserialize)]
struct PlansQuery {
    /// Keep plans whose code, name or text contains this.
    q: Option<String>,
}

/// Every DSV plan the archive lists, with the state of its current
/// version in this workspace.
async fn list_plans(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<PlansQuery>,
) -> Result<Json<Vec<PlanResponse>>, AppError> {
    credit_transfer_workspace(&state, workspace_id, &user).await?;
    let mut plans = minerva_db::queries::syllabus_archive::list_plans(&state.db).await?;
    if let Some(needle) = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty()) {
        let by_text: std::collections::HashSet<String> =
            minerva_db::queries::syllabus_archive::codes_with_text(&state.db, needle)
                .await?
                .into_iter()
                .collect();
        let lower = needle.to_lowercase();
        plans.retain(|p| {
            by_text.contains(&p.code)
                || p.code.to_lowercase().contains(&lower)
                || p.name_sv.to_lowercase().contains(&lower)
                || p.name_en
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&lower))
        });
    }
    let mut versions: HashMap<String, Vec<minerva_db::queries::syllabus_archive::VersionListing>> =
        HashMap::new();
    for v in minerva_db::queries::syllabus_archive::list_versions(&state.db).await? {
        versions.entry(v.code.clone()).or_default().push(v);
    }
    let doc_status: HashMap<String, String> =
        minerva_db::queries::credit_transfer::plan_document_statuses(&state.db, workspace_id)
            .await?
            .into_iter()
            .collect();
    let week_ago = Utc::now() - chrono::Duration::days(7);

    let out = plans
        .into_iter()
        .map(|p| {
            // Listed newest term first, so the first rows are the current
            // version in each of its languages.
            let vs = versions.remove(&p.code).unwrap_or_default();
            let latest_term = vs.first().map(|v| v.valid_from_term_id);
            let (current, older): (Vec<_>, Vec<_>) = vs
                .into_iter()
                .partition(|v| Some(v.valid_from_term_id) == latest_term);
            let mut older_versions: Vec<String> = Vec::new();
            for v in older {
                if !older_versions.contains(&v.valid_from) {
                    older_versions.push(v.valid_from);
                }
            }
            let level = current
                .iter()
                .find(|v| v.language == "sv" && v.level.is_some())
                .or_else(|| current.iter().find(|v| v.level.is_some()))
                .and_then(|v| v.level.clone());
            let changed_this_week = current.iter().any(|v| {
                v.downloaded
                    && (v.first_seen_at > week_ago
                        || v.content_changed_at.is_some_and(|t| t > week_ago))
            });
            let status = plan_status(p.is_discontinued, &current, |lang| {
                doc_status
                    .get(&format!("{}/{}", p.code, lang))
                    .map(String::as_str)
            });
            PlanResponse {
                latest: current.first().map(|v| LatestVersion {
                    valid_from: v.valid_from.clone(),
                    files: current
                        .iter()
                        .map(|v| PlanFile {
                            language: v.language.clone(),
                            pdf_url: v.pdf_url.clone(),
                        })
                        .collect(),
                }),
                code: p.code,
                name_sv: p.name_sv,
                name_en: p.name_en,
                ects: p.ects,
                level,
                older_versions,
                status,
                changed_this_week,
            }
        })
        .collect();
    Ok(Json(out))
}

fn plan_status<'a>(
    discontinued: bool,
    current: &[minerva_db::queries::syllabus_archive::VersionListing],
    doc_status: impl Fn(&str) -> Option<&'a str>,
) -> &'static str {
    if discontinued {
        return "discontinued";
    }
    let statuses: Vec<Option<&str>> = current
        .iter()
        .filter(|v| v.downloaded)
        .map(|v| doc_status(&v.language))
        .collect();
    if statuses.contains(&Some("ready")) {
        "indexed"
    } else if statuses
        .iter()
        .any(|s| matches!(s, Some("pending" | "processing")))
    {
        "indexing"
    } else if statuses.contains(&Some("failed"))
        || (statuses.is_empty() && current.iter().any(|v| v.fetch_error.is_some()))
    {
        "failed"
    } else {
        "pending"
    }
}

#[derive(Serialize)]
struct RulesResponse {
    /// When SU's rulebook index was last read.
    checked_at: Option<DateTime<Utc>>,
    documents: Vec<RuleDocument>,
}

#[derive(Serialize)]
struct RuleDocument {
    /// The page on SU's staff web, or the PDF for a file the index links
    /// directly.
    url: String,
    title: String,
    /// The index heading it is listed under ('Regler', 'Stöddokument').
    category: String,
    /// "Senast uppdaterad" on the page.
    updated_on: Option<chrono::NaiveDate>,
    files: Vec<RuleFile>,
    /// `indexed`, `indexing`, `pending` (not read or mirrored yet) or
    /// `failed`.
    status: &'static str,
}

#[derive(Serialize)]
struct RuleFile {
    url: String,
    title: String,
}

/// Every document of SU's rulebook the workspace mirrors, in the index's
/// order, with the state of its documents in this workspace.
async fn list_rules(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<RulesResponse>, AppError> {
    credit_transfer_workspace(&state, workspace_id, &user).await?;
    let pages = minerva_db::queries::rulebook::list_pages(&state.db).await?;
    let mut files: HashMap<String, Vec<minerva_db::queries::rulebook::FileListing>> =
        HashMap::new();
    for f in minerva_db::queries::rulebook::list_files(&state.db).await? {
        files.entry(f.page_url.clone()).or_default().push(f);
    }
    let doc_status: HashMap<String, String> =
        minerva_db::queries::credit_transfer::rulebook_document_statuses(&state.db, workspace_id)
            .await?
            .into_iter()
            .collect();
    let documents = pages
        .into_iter()
        .map(|p| {
            let page_files = files.remove(&p.url).unwrap_or_default();
            let status = rule_status(&p, &page_files, |url| {
                doc_status.get(url).map(String::as_str)
            });
            RuleDocument {
                // A file entry's own file is the entry; listing it again
                // would only repeat the title.
                files: if p.is_file {
                    Vec::new()
                } else {
                    page_files
                        .into_iter()
                        .map(|f| RuleFile {
                            url: f.url,
                            title: f.title,
                        })
                        .collect()
                },
                url: p.url,
                title: p.title,
                category: p.category,
                updated_on: p.updated_on,
                status,
            }
        })
        .collect();
    Ok(Json(RulesResponse {
        checked_at: minerva_db::queries::rulebook::last_checked_at(&state.db).await?,
        documents,
    }))
}

/// One rulebook entry's state in a workspace: what it is mirrored as (its
/// text, its files or both), and how far indexing those has come.
fn rule_status<'a>(
    page: &minerva_db::queries::rulebook::PageListing,
    files: &[minerva_db::queries::rulebook::FileListing],
    doc_status: impl Fn(&str) -> Option<&'a str>,
) -> &'static str {
    let mut mirrored: Vec<&str> = files
        .iter()
        .filter(|f| f.mirror && f.downloaded)
        .map(|f| f.url.as_str())
        .collect();
    if page.mirror_text && page.read {
        mirrored.push(page.url.as_str());
    }
    if mirrored.is_empty() {
        let never_read = !page.is_file && !page.read && page.fetch_error.is_some();
        let never_downloaded = page.is_file && files.iter().all(|f| f.fetch_error.is_some());
        return if never_read || (never_downloaded && !files.is_empty()) {
            "failed"
        } else {
            "pending"
        };
    }
    let statuses: Vec<Option<&str>> = mirrored.into_iter().map(&doc_status).collect();
    if statuses.contains(&Some("failed")) {
        "failed"
    } else if statuses.iter().all(|s| *s == Some("ready")) {
        "indexed"
    } else {
        "indexing"
    }
}
