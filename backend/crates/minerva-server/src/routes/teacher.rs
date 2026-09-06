//! Owner-scoped AI spend for the teacher portal.
//!
//! The per-course usage tab answers "which of my students burned
//! tokens"; this answers "how much am I spending against my own daily
//! cap, and across which courses". The cap
//! (`users.owner_daily_cost_limit_usd`) is per owner and sums every
//! course the teacher owns, so the view is scoped the same way:
//! courses the caller only assists on bill to their owner and are
//! deliberately absent here.
//!
//! Both spend axes the cap enforces are reported: student chat
//! (`usage_daily`) and pipeline / classification work
//! (`course_token_usage`). Cost is derived on read from tokens x each
//! model's current rate, identical to `usage::get_owner_daily_cost`, so
//! the figure a teacher reads is the figure the cap tests.

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Extension, Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use minerva_core::models::User;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::guards::require_admin;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/usage", get(get_owner_usage))
}

/// Mounted under `/admin`: the same view for someone else's account.
/// Admins carry the cap dial (`/admin/users`), so they need to see the
/// spend it is being set against without asking the teacher to read
/// their own page out loud.
pub fn admin_router() -> Router<AppState> {
    Router::new().route("/users/{id}/usage", get(get_user_usage))
}

/// Default reporting window. Long enough to cover a course's ingest
/// burst plus steady chat traffic, short enough to stay "this term".
const DEFAULT_WINDOW_DAYS: i32 = 30;
const MAX_WINDOW_DAYS: i32 = 180;

#[derive(Deserialize)]
struct WindowQuery {
    days: Option<i32>,
}

#[derive(Serialize)]
struct OwnerUsageResponse {
    /// Whose spend this is. Populated for the self view too, so the
    /// admin drill-down needs no second request to name the account.
    owner_id: Uuid,
    owner_eppn: String,
    owner_display_name: Option<String>,
    /// The owner's aggregate daily cap in USD. 0 = unlimited.
    daily_cost_limit_usd: Decimal,
    /// Today's spend across every owned course, chat + pipeline. This is
    /// the number the cap is tested against.
    spend_today_usd: Decimal,
    window_days: i32,
    window_spend_usd: Decimal,
    window_chat_spend_usd: Decimal,
    window_pipeline_spend_usd: Decimal,
    window_requests: i64,
    courses: Vec<OwnerCourseUsage>,
    providers: Vec<ProviderUsage>,
    daily: Vec<DailyUsage>,
}

#[derive(Serialize)]
struct OwnerCourseUsage {
    id: Uuid,
    name: String,
    course_code: Option<String>,
    semester_label: Option<String>,
    /// False for an archived course that still carries usage in the
    /// window. Such a course is not in the owner's active list but its
    /// spend still counted against the cap while it ran.
    active: bool,
    /// Configured chat model and its provider. `None` once the course is
    /// archived (only its usage rows survive) or the model left the catalog.
    model: Option<String>,
    provider: Option<String>,
    /// Per-student-per-day cap for this course. 0 = unlimited.
    student_daily_cost_limit_usd: Decimal,
    student_count: i64,
    window_active_students: i64,
    spend_today_usd: Decimal,
    window_spend_usd: Decimal,
    window_chat_spend_usd: Decimal,
    window_pipeline_spend_usd: Decimal,
    window_requests: i64,
    window_prompt_tokens: i64,
    window_completion_tokens: i64,
}

/// Window spend grouped by LLM provider. Which provider a course runs on
/// decides what a limit-increase request needs (a unit's budget approval
/// for the paid hosted providers), so it is reported rather than left
/// for the teacher to infer from model ids.
#[derive(Serialize)]
struct ProviderUsage {
    provider: String,
    window_spend_usd: Decimal,
    window_prompt_tokens: i64,
    window_completion_tokens: i64,
}

#[derive(Serialize)]
struct DailyUsage {
    date: chrono::NaiveDate,
    chat_spend_usd: Decimal,
    pipeline_spend_usd: Decimal,
    requests: i64,
}

#[derive(Default)]
struct CourseAgg {
    name: String,
    course_code: Option<String>,
    semester_label: Option<String>,
    active: bool,
    model: Option<String>,
    student_daily_cost_limit_usd: Decimal,
    student_count: i64,
    spend_today: Decimal,
    chat_spend: Decimal,
    pipeline_spend: Decimal,
    requests: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
}

#[derive(Default)]
struct ProviderAgg {
    spend: Decimal,
    prompt_tokens: i64,
    completion_tokens: i64,
}

#[derive(Default)]
struct DailyAgg {
    chat_spend: Decimal,
    pipeline_spend: Decimal,
    requests: i64,
}

/// The signed-in teacher's own spend.
async fn get_owner_usage(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Query(params): Query<WindowQuery>,
) -> Result<Json<OwnerUsageResponse>, AppError> {
    if !user.role.is_teacher_or_above() {
        return Err(AppError::Forbidden);
    }
    Ok(Json(
        owner_usage(&state, &user, window_days(&params)).await?,
    ))
}

/// Any account's spend, for an admin. Not restricted to users who
/// currently hold a teacher role: a demoted or suspended account can
/// still own courses that spent money, and that spend is exactly what an
/// admin is looking for.
async fn get_user_usage(
    State(state): State<AppState>,
    Extension(caller): Extension<User>,
    Path(user_id): Path<Uuid>,
    Query(params): Query<WindowQuery>,
) -> Result<Json<OwnerUsageResponse>, AppError> {
    require_admin(&caller)?;
    let owner = minerva_db::queries::users::find_by_id(&state.db, user_id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(
        owner_usage(
            &state,
            &crate::auth::user_from_row(owner),
            window_days(&params),
        )
        .await?,
    ))
}

fn window_days(params: &WindowQuery) -> i32 {
    params
        .days
        .unwrap_or(DEFAULT_WINDOW_DAYS)
        .clamp(1, MAX_WINDOW_DAYS)
}

async fn owner_usage(
    state: &AppState,
    user: &User,
    days: i32,
) -> Result<OwnerUsageResponse, AppError> {
    let owned = minerva_db::queries::courses::list_by_owner(&state.db, user.id).await?;
    let student_counts = minerva_db::queries::courses::count_students_by_course(&state.db).await?;
    let chat_rows =
        minerva_db::queries::usage::get_owner_chat_usage(&state.db, user.id, days).await?;
    let pipeline_rows =
        minerva_db::queries::usage::get_owner_pipeline_usage(&state.db, user.id, days).await?;
    let active_students =
        minerva_db::queries::usage::get_owner_active_students(&state.db, user.id, days).await?;
    // One catalog read resolves every course's model to its provider;
    // the usage rows already carry theirs.
    let provider_of: HashMap<String, String> =
        minerva_db::queries::chat_models::list_all(&state.db)
            .await?
            .into_iter()
            .map(|m| (m.model, m.provider))
            .collect();

    // Seed from the owned courses so a course with no traffic still
    // shows its cap and enrolment rather than vanishing.
    let mut courses: HashMap<Uuid, CourseAgg> = owned
        .into_iter()
        .map(|c| {
            let student_count = student_counts.get(&c.id).copied().unwrap_or(0);
            (
                c.id,
                CourseAgg {
                    name: c.name,
                    course_code: c.course_code,
                    semester_label: c.semester_label,
                    active: c.active,
                    model: Some(c.model),
                    student_daily_cost_limit_usd: c.daily_cost_limit_usd,
                    student_count,
                    ..Default::default()
                },
            )
        })
        .collect();

    let mut providers: HashMap<String, ProviderAgg> = HashMap::new();
    let mut daily: BTreeMap<chrono::NaiveDate, DailyAgg> = BTreeMap::new();

    for row in chat_rows {
        let course = courses.entry(row.course_id).or_insert_with(|| CourseAgg {
            name: row.course_name.clone(),
            course_code: row.course_code.clone(),
            semester_label: row.course_semester_label.clone(),
            active: row.course_active,
            ..Default::default()
        });
        course.chat_spend += row.cost_usd;
        course.requests += row.request_count;
        course.prompt_tokens += row.prompt_tokens;
        course.completion_tokens += row.completion_tokens;
        if row.is_today {
            course.spend_today += row.cost_usd;
        }

        let provider = providers.entry(row.provider).or_default();
        provider.spend += row.cost_usd;
        provider.prompt_tokens += row.prompt_tokens;
        provider.completion_tokens += row.completion_tokens;

        let day = daily.entry(row.date).or_default();
        day.chat_spend += row.cost_usd;
        day.requests += row.request_count;
    }

    for row in pipeline_rows {
        let course = courses.entry(row.course_id).or_insert_with(|| CourseAgg {
            name: row.course_name.clone(),
            course_code: row.course_code.clone(),
            semester_label: row.course_semester_label.clone(),
            active: row.course_active,
            ..Default::default()
        });
        course.pipeline_spend += row.cost_usd;
        course.prompt_tokens += row.prompt_tokens;
        course.completion_tokens += row.completion_tokens;
        if row.is_today {
            course.spend_today += row.cost_usd;
        }

        let provider = providers.entry(row.provider).or_default();
        provider.spend += row.cost_usd;
        provider.prompt_tokens += row.prompt_tokens;
        provider.completion_tokens += row.completion_tokens;

        daily.entry(row.date).or_default().pipeline_spend += row.cost_usd;
    }

    let active_by_course: HashMap<Uuid, i64> = active_students
        .into_iter()
        .map(|r| (r.course_id, r.active_students))
        .collect();

    let mut course_rows: Vec<OwnerCourseUsage> = courses
        .into_iter()
        .map(|(id, agg)| OwnerCourseUsage {
            id,
            name: agg.name,
            course_code: agg.course_code,
            semester_label: agg.semester_label,
            active: agg.active,
            provider: agg.model.as_ref().and_then(|m| provider_of.get(m).cloned()),
            model: agg.model,
            student_daily_cost_limit_usd: agg.student_daily_cost_limit_usd,
            student_count: agg.student_count,
            window_active_students: active_by_course.get(&id).copied().unwrap_or(0),
            spend_today_usd: agg.spend_today,
            window_spend_usd: agg.chat_spend + agg.pipeline_spend,
            window_chat_spend_usd: agg.chat_spend,
            window_pipeline_spend_usd: agg.pipeline_spend,
            window_requests: agg.requests,
            window_prompt_tokens: agg.prompt_tokens,
            window_completion_tokens: agg.completion_tokens,
        })
        .collect();
    course_rows.sort_by(|a, b| {
        b.window_spend_usd
            .cmp(&a.window_spend_usd)
            .then_with(|| a.name.cmp(&b.name))
    });

    let mut provider_rows: Vec<ProviderUsage> = providers
        .into_iter()
        .map(|(provider, agg)| ProviderUsage {
            provider,
            window_spend_usd: agg.spend,
            window_prompt_tokens: agg.prompt_tokens,
            window_completion_tokens: agg.completion_tokens,
        })
        .collect();
    provider_rows.sort_by(|a, b| {
        b.window_spend_usd
            .cmp(&a.window_spend_usd)
            .then_with(|| a.provider.cmp(&b.provider))
    });

    let daily_rows: Vec<DailyUsage> = daily
        .into_iter()
        .map(|(date, agg)| DailyUsage {
            date,
            chat_spend_usd: agg.chat_spend,
            pipeline_spend_usd: agg.pipeline_spend,
            requests: agg.requests,
        })
        .collect();

    let window_chat: Decimal = course_rows.iter().map(|c| c.window_chat_spend_usd).sum();
    let window_pipeline: Decimal = course_rows
        .iter()
        .map(|c| c.window_pipeline_spend_usd)
        .sum();

    Ok(OwnerUsageResponse {
        owner_id: user.id,
        owner_eppn: user.eppn.clone(),
        owner_display_name: user.display_name.clone(),
        daily_cost_limit_usd: user.owner_daily_cost_limit_usd,
        spend_today_usd: course_rows.iter().map(|c| c.spend_today_usd).sum(),
        window_days: days,
        window_spend_usd: window_chat + window_pipeline,
        window_chat_spend_usd: window_chat,
        window_pipeline_spend_usd: window_pipeline,
        window_requests: course_rows.iter().map(|c| c.window_requests).sum(),
        courses: course_rows,
        providers: provider_rows,
        daily: daily_rows,
    })
}
