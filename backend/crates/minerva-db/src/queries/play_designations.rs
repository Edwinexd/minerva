use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub struct PlayDesignationRow {
    pub id: Uuid,
    pub course_id: Uuid,
    pub designation: String,
    pub added_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_synced_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_error: Option<String>,
}

pub async fn insert(
    db: &PgPool,
    id: Uuid,
    course_id: Uuid,
    designation: &str,
    added_by: Uuid,
) -> Result<PlayDesignationRow, sqlx::Error> {
    sqlx::query_as::<_, PlayDesignationRow>(
        r#"INSERT INTO play_designations (id, course_id, designation, added_by)
        VALUES ($1, $2, $3, $4)
        RETURNING id, course_id, designation, added_by, created_at, last_synced_at, last_error"#,
    )
    .bind(id)
    .bind(course_id)
    .bind(designation)
    .bind(added_by)
    .fetch_one(db)
    .await
}

pub async fn list_by_course(
    db: &PgPool,
    course_id: Uuid,
) -> Result<Vec<PlayDesignationRow>, sqlx::Error> {
    sqlx::query_as::<_, PlayDesignationRow>(
        r#"SELECT id, course_id, designation, added_by, created_at, last_synced_at, last_error
        FROM play_designations WHERE course_id = $1 ORDER BY designation ASC"#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await
}

pub async fn list_all(db: &PgPool) -> Result<Vec<PlayDesignationRow>, sqlx::Error> {
    sqlx::query_as::<_, PlayDesignationRow>(
        r#"SELECT id, course_id, designation, added_by, created_at, last_synced_at, last_error
        FROM play_designations ORDER BY course_id, designation"#,
    )
    .fetch_all(db)
    .await
}

pub async fn find_by_id(db: &PgPool, id: Uuid) -> Result<Option<PlayDesignationRow>, sqlx::Error> {
    sqlx::query_as::<_, PlayDesignationRow>(
        r#"SELECT id, course_id, designation, added_by, created_at, last_synced_at, last_error
        FROM play_designations WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

pub async fn delete(db: &PgPool, id: Uuid, course_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM play_designations WHERE id = $1 AND course_id = $2")
        .bind(id)
        .bind(course_id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn mark_synced_ok(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE play_designations SET last_synced_at = NOW(), last_error = NULL WHERE id = $1",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn mark_synced_error(db: &PgPool, id: Uuid, error: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE play_designations SET last_synced_at = NOW(), last_error = $1 WHERE id = $2",
    )
    .bind(error)
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}
