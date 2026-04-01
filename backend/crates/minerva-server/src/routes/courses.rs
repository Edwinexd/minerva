use axum::extract::{Extension, Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use minerva_core::models::User;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_courses).post(create_course))
        .route(
            "/{id}",
            get(get_course).put(update_course).delete(archive_course),
        )
        .route("/{id}/members", get(list_members).post(add_member))
        .route("/{id}/members/{user_id}", delete(remove_member))
}

#[derive(Deserialize)]
struct CreateCourseRequest {
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct UpdateCourseRequest {
    name: Option<String>,
    description: Option<String>,
    context_ratio: Option<f64>,
    temperature: Option<f64>,
    model: Option<String>,
    system_prompt: Option<String>,
    max_chunks: Option<i32>,
    strategy: Option<String>,
    embedding_provider: Option<String>,
    embedding_model: Option<String>,
    daily_token_limit: Option<i64>,
}

#[derive(Serialize)]
struct CourseResponse {
    id: Uuid,
    name: String,
    description: Option<String>,
    owner_id: Uuid,
    context_ratio: f64,
    temperature: f64,
    model: String,
    system_prompt: Option<String>,
    max_chunks: i32,
    strategy: String,
    embedding_provider: String,
    embedding_model: String,
    daily_token_limit: i64,
    active: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<minerva_db::queries::courses::CourseRow> for CourseResponse {
    fn from(row: minerva_db::queries::courses::CourseRow) -> Self {
        Self {
            id: row.id,
            name: row.name,
            description: row.description,
            owner_id: row.owner_id,
            context_ratio: row.context_ratio,
            temperature: row.temperature,
            model: row.model,
            system_prompt: row.system_prompt,
            max_chunks: row.max_chunks,
            strategy: row.strategy,
            embedding_provider: row.embedding_provider,
            embedding_model: row.embedding_model,
            daily_token_limit: row.daily_token_limit,
            active: row.active,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

async fn list_courses(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
) -> Result<Json<Vec<CourseResponse>>, AppError> {
    let rows = if user.role.is_admin() {
        minerva_db::queries::courses::list_all(&state.db).await?
    } else if user.role.is_teacher_or_above() {
        minerva_db::queries::courses::list_by_owner(&state.db, user.id).await?
    } else {
        // Students see courses they're a member of
        minerva_db::queries::courses::list_by_member(&state.db, user.id).await?
    };
    Ok(Json(rows.into_iter().map(CourseResponse::from).collect()))
}

async fn create_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Json(body): Json<CreateCourseRequest>,
) -> Result<Json<CourseResponse>, AppError> {
    if !user.role.is_teacher_or_above() {
        return Err(AppError::Forbidden);
    }

    let id = Uuid::new_v4();
    let input = minerva_db::queries::courses::CreateCourse {
        name: body.name,
        description: body.description,
        owner_id: user.id,
    };

    let row = minerva_db::queries::courses::create(&state.db, id, &input).await?;

    // Auto-add owner as teacher member
    minerva_db::queries::courses::add_member(&state.db, id, user.id, "teacher").await?;

    Ok(Json(CourseResponse::from(row)))
}

async fn get_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<CourseResponse>, AppError> {
    let row = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    // Check access: owner, admin, or member
    if row.owner_id != user.id
        && !user.role.is_admin()
        && !minerva_db::queries::courses::is_member(&state.db, id, user.id).await?
    {
        return Err(AppError::Forbidden);
    }

    Ok(Json(CourseResponse::from(row)))
}

async fn update_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCourseRequest>,
) -> Result<Json<CourseResponse>, AppError> {
    let existing = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if existing.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    // Validate embedding_provider
    if let Some(ref provider) = body.embedding_provider {
        if !minerva_ingest::pipeline::VALID_EMBEDDING_PROVIDERS.contains(&provider.as_str()) {
            return Err(AppError::BadRequest(format!(
                "invalid embedding_provider: {}",
                provider
            )));
        }
    }

    // Validate embedding_model based on the effective provider
    let effective_provider = body
        .embedding_provider
        .as_deref()
        .unwrap_or(&existing.embedding_provider);

    if effective_provider == "local" {
        if let Some(ref model) = body.embedding_model {
            let valid = minerva_ingest::pipeline::VALID_LOCAL_MODELS
                .iter()
                .any(|(name, _)| *name == model.as_str());
            if !valid {
                return Err(AppError::BadRequest(format!(
                    "invalid local embedding_model: {}",
                    model
                )));
            }
        }
    }

    // For openai provider, force the embedding_model to the canonical value
    let embedding_model = if effective_provider == "openai" {
        body.embedding_model
            .as_ref()
            .map(|_| minerva_ingest::pipeline::OPENAI_EMBEDDING_MODEL.to_string())
            .or_else(|| {
                // If switching to openai, ensure the model column is updated
                if body.embedding_provider.is_some() {
                    Some(minerva_ingest::pipeline::OPENAI_EMBEDDING_MODEL.to_string())
                } else {
                    None
                }
            })
    } else {
        body.embedding_model
    };

    let input = minerva_db::queries::courses::UpdateCourse {
        name: body.name,
        description: body.description,
        context_ratio: body.context_ratio,
        temperature: body.temperature,
        model: body.model,
        system_prompt: body.system_prompt,
        max_chunks: body.max_chunks,
        strategy: body.strategy,
        embedding_provider: body.embedding_provider,
        embedding_model,
        daily_token_limit: body.daily_token_limit,
    };

    let row = minerva_db::queries::courses::update(&state.db, id, &input)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(CourseResponse::from(row)))
}

async fn archive_course(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let existing = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if existing.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    minerva_db::queries::courses::archive(&state.db, id).await?;
    Ok(Json(serde_json::json!({ "archived": true })))
}

#[derive(Serialize)]
struct MemberResponse {
    user_id: Uuid,
    eppn: Option<String>,
    display_name: Option<String>,
    role: String,
    added_at: chrono::DateTime<chrono::Utc>,
}

async fn list_members(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<MemberResponse>>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let rows = minerva_db::queries::courses::list_members(&state.db, id).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| MemberResponse {
                user_id: r.user_id,
                eppn: r.eppn,
                display_name: r.display_name,
                role: r.role,
                added_at: r.added_at,
            })
            .collect(),
    ))
}

#[derive(Deserialize)]
struct AddMemberRequest {
    eppn: String,
    role: Option<String>,
}

async fn add_member(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMemberRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    // Find or create the user by eppn
    let target_user = minerva_db::queries::users::find_by_eppn(&state.db, &body.eppn).await?;
    let target_id = match target_user {
        Some(u) => u.id,
        None => {
            let new_id = Uuid::new_v4();
            minerva_db::queries::users::insert(&state.db, new_id, &body.eppn, None, "student")
                .await?;
            new_id
        }
    };

    let role = body.role.as_deref().unwrap_or("student");
    minerva_db::queries::courses::add_member(&state.db, id, target_id, role).await?;

    Ok(Json(
        serde_json::json!({ "added": true, "user_id": target_id }),
    ))
}

async fn remove_member(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let course = minerva_db::queries::courses::find_by_id(&state.db, id)
        .await?
        .ok_or(AppError::NotFound)?;

    if course.owner_id != user.id && !user.role.is_admin() {
        return Err(AppError::Forbidden);
    }

    let removed = minerva_db::queries::courses::remove_member(&state.db, id, user_id).await?;
    Ok(Json(serde_json::json!({ "removed": removed })))
}
