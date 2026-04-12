use sqlx::PgPool;

#[derive(Debug, sqlx::FromRow)]
pub struct PlayCourseCatalogRow {
    pub code: String,
    pub name: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

pub async fn list_all(db: &PgPool) -> Result<Vec<PlayCourseCatalogRow>, sqlx::Error> {
    sqlx::query_as::<_, PlayCourseCatalogRow>(
        "SELECT code, name, updated_at FROM play_course_catalog ORDER BY code",
    )
    .fetch_all(db)
    .await
}

/// Upsert a batch of (code, name) pairs. Existing entries are updated,
/// new ones are inserted. Returns the number of rows touched.
pub async fn upsert_many(db: &PgPool, entries: &[(String, String)]) -> Result<u64, sqlx::Error> {
    if entries.is_empty() {
        return Ok(0);
    }
    let codes: Vec<&str> = entries.iter().map(|(c, _)| c.as_str()).collect();
    let names: Vec<&str> = entries.iter().map(|(_, n)| n.as_str()).collect();
    let result = sqlx::query(
        r#"INSERT INTO play_course_catalog (code, name)
        SELECT * FROM UNNEST($1::text[], $2::text[])
        ON CONFLICT (code) DO UPDATE
            SET name = EXCLUDED.name,
                updated_at = NOW()"#,
    )
    .bind(&codes)
    .bind(&names)
    .execute(db)
    .await?;
    Ok(result.rows_affected())
}
