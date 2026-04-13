use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct CourseRow {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Uuid,
    pub context_ratio: f64,
    pub temperature: f64,
    pub model: String,
    pub system_prompt: Option<String>,
    pub max_chunks: i32,
    pub min_score: f32,
    pub strategy: String,
    pub embedding_provider: String,
    pub embedding_model: String,
    pub daily_token_limit: i64,
    pub active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub struct CreateCourse {
    pub name: String,
    pub description: Option<String>,
    pub owner_id: Uuid,
}

pub struct UpdateCourse {
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_ratio: Option<f64>,
    pub temperature: Option<f64>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub max_chunks: Option<i32>,
    pub min_score: Option<f32>,
    pub strategy: Option<String>,
    pub embedding_provider: Option<String>,
    pub embedding_model: Option<String>,
    pub daily_token_limit: Option<i64>,
}

pub async fn create(db: &PgPool, id: Uuid, input: &CreateCourse) -> Result<CourseRow, sqlx::Error> {
    sqlx::query_as::<_, CourseRow>(
        r#"INSERT INTO courses (id, name, description, owner_id)
        VALUES ($1, $2, $3, $4)
        RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, embedding_provider, embedding_model, daily_token_limit, active, created_at, updated_at"#,
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.description)
    .bind(input.owner_id)
    .fetch_one(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as::<_, CourseRow>(
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, embedding_provider, embedding_model, daily_token_limit, active, created_at, updated_at FROM courses WHERE id = $1 AND active = true",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn list_by_owner(db: &PgPool, owner_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as::<_, CourseRow>(
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, embedding_provider, embedding_model, daily_token_limit, active, created_at, updated_at FROM courses WHERE owner_id = $1 AND active = true ORDER BY updated_at DESC",
    )
    .bind(owner_id)
    .fetch_all(db)
    .await
}

pub async fn list_by_member(db: &PgPool, user_id: Uuid) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as::<_, CourseRow>(
        r#"SELECT c.id, c.name, c.description, c.owner_id, c.context_ratio, c.temperature, c.model, c.system_prompt, c.max_chunks, c.strategy, c.embedding_provider, c.embedding_model, c.daily_token_limit, c.active, c.created_at, c.updated_at
        FROM courses c
        JOIN course_members cm ON cm.course_id = c.id
        WHERE cm.user_id = $1 AND c.active = true
        ORDER BY c.updated_at DESC"#,
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn list_all(db: &PgPool) -> Result<Vec<CourseRow>, sqlx::Error> {
    sqlx::query_as::<_, CourseRow>(
        "SELECT id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, embedding_provider, embedding_model, daily_token_limit, active, created_at, updated_at FROM courses WHERE active = true ORDER BY updated_at DESC",
    )
    .fetch_all(db)
    .await
}

pub async fn update(
    db: &PgPool,
    id: Uuid,
    input: &UpdateCourse,
) -> Result<Option<CourseRow>, sqlx::Error> {
    sqlx::query_as::<_, CourseRow>(
        r#"UPDATE courses SET
            name = COALESCE($2, name),
            description = COALESCE($3, description),
            context_ratio = COALESCE($4, context_ratio),
            temperature = COALESCE($5, temperature),
            model = COALESCE($6, model),
            system_prompt = COALESCE($7, system_prompt),
            max_chunks = COALESCE($8, max_chunks),
            strategy = COALESCE($9, strategy),
            daily_token_limit = COALESCE($10, daily_token_limit),
            embedding_provider = COALESCE($11, embedding_provider),
            embedding_model = COALESCE($12, embedding_model),
            min_score = COALESCE($13, min_score),
            updated_at = NOW()
        WHERE id = $1 AND active = true
        RETURNING id, name, description, owner_id, context_ratio, temperature, model, system_prompt, max_chunks, min_score, strategy, embedding_provider, embedding_model, daily_token_limit, active, created_at, updated_at"#,
    )
    .bind(id)
    .bind(&input.name)
    .bind(&input.description)
    .bind(input.context_ratio)
    .bind(input.temperature)
    .bind(&input.model)
    .bind(&input.system_prompt)
    .bind(input.max_chunks)
    .bind(&input.strategy)
    .bind(input.daily_token_limit)
    .bind(&input.embedding_provider)
    .bind(&input.embedding_model)
    .bind(input.min_score)
    .fetch_optional(db)
    .await
}

pub async fn archive(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE courses SET active = false, updated_at = NOW() WHERE id = $1 AND active = true",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[derive(Debug, sqlx::FromRow)]
pub struct MemberRow {
    pub course_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub added_at: chrono::DateTime<chrono::Utc>,
    pub eppn: Option<String>,
    pub display_name: Option<String>,
}

pub async fn add_member(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
    role: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO course_members (course_id, user_id, role) VALUES ($1, $2, $3) ON CONFLICT (course_id, user_id) DO UPDATE SET role = $3",
    )
    .bind(course_id)
    .bind(user_id)
    .bind(role)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn remove_member(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM course_members WHERE course_id = $1 AND user_id = $2")
        .bind(course_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_members(db: &PgPool, course_id: Uuid) -> Result<Vec<MemberRow>, sqlx::Error> {
    sqlx::query_as::<_, MemberRow>(
        r#"SELECT cm.course_id, cm.user_id, cm.role, cm.added_at, u.eppn, u.display_name
        FROM course_members cm
        JOIN users u ON u.id = cm.user_id
        WHERE cm.course_id = $1
        ORDER BY cm.added_at"#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await
}

pub async fn is_member(db: &PgPool, course_id: Uuid, user_id: Uuid) -> Result<bool, sqlx::Error> {
    let row = sqlx::query("SELECT 1 FROM course_members WHERE course_id = $1 AND user_id = $2")
        .bind(course_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    Ok(row.is_some())
}

pub async fn is_course_teacher(
    db: &PgPool,
    course_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        "SELECT 1 FROM course_members WHERE course_id = $1 AND user_id = $2 AND role IN ('teacher', 'ta')",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}
