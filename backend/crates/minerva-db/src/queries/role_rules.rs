use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub struct RoleRuleRow {
    pub id: Uuid,
    pub name: String,
    pub target_role: String,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug)]
pub struct RoleRuleConditionRow {
    pub id: Uuid,
    pub rule_id: Uuid,
    pub attribute: String,
    pub operator: String,
    pub value: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn list_all(db: &PgPool) -> Result<Vec<RoleRuleRow>, sqlx::Error> {
    sqlx::query_as!(
        RoleRuleRow,
        "SELECT id, name, target_role, enabled, created_at, updated_at FROM role_rules ORDER BY created_at",
    )
    .fetch_all(db)
    .await
}

pub async fn list_enabled(db: &PgPool) -> Result<Vec<RoleRuleRow>, sqlx::Error> {
    sqlx::query_as!(
        RoleRuleRow,
        "SELECT id, name, target_role, enabled, created_at, updated_at FROM role_rules WHERE enabled = TRUE ORDER BY created_at",
    )
    .fetch_all(db)
    .await
}

pub async fn list_conditions_for_rules(
    db: &PgPool,
    rule_ids: &[Uuid],
) -> Result<Vec<RoleRuleConditionRow>, sqlx::Error> {
    if rule_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as!(
        RoleRuleConditionRow,
        "SELECT id, rule_id, attribute, operator, value, created_at
         FROM role_rule_conditions WHERE rule_id = ANY($1) ORDER BY created_at",
        rule_ids,
    )
    .fetch_all(db)
    .await
}

pub async fn create_rule(
    db: &PgPool,
    id: Uuid,
    name: &str,
    target_role: &str,
    enabled: bool,
) -> Result<RoleRuleRow, sqlx::Error> {
    sqlx::query_as!(
        RoleRuleRow,
        "INSERT INTO role_rules (id, name, target_role, enabled)
         VALUES ($1, $2, $3, $4)
         RETURNING id, name, target_role, enabled, created_at, updated_at",
        id,
        name,
        target_role,
        enabled,
    )
    .fetch_one(db)
    .await
}

pub async fn update_rule(
    db: &PgPool,
    id: Uuid,
    name: &str,
    target_role: &str,
    enabled: bool,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!(
        "UPDATE role_rules SET name = $2, target_role = $3, enabled = $4, updated_at = NOW() WHERE id = $1",
        id,
        name,
        target_role,
        enabled,
    )
    .execute(db)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_rule(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM role_rules WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn create_condition(
    db: &PgPool,
    id: Uuid,
    rule_id: Uuid,
    attribute: &str,
    operator: &str,
    value: &str,
) -> Result<RoleRuleConditionRow, sqlx::Error> {
    sqlx::query_as!(
        RoleRuleConditionRow,
        "INSERT INTO role_rule_conditions (id, rule_id, attribute, operator, value)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, rule_id, attribute, operator, value, created_at",
        id,
        rule_id,
        attribute,
        operator,
        value,
    )
    .fetch_one(db)
    .await
}

pub async fn delete_condition(db: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query!("DELETE FROM role_rule_conditions WHERE id = $1", id)
        .execute(db)
        .await?;
    Ok(result.rows_affected() > 0)
}
