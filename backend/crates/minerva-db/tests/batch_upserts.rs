use minerva_db::queries::{play_course_catalog, role_rule_attribute_observations};
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn repeated_attribute_value_in_one_login_is_observed_once(db: PgPool) {
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,eppn) VALUES ($1,$2)")
        .bind(user)
        .bind(format!("observe-{user}@example.invalid"))
        .execute(&db)
        .await
        .unwrap();
    let pair = ("affiliation".to_string(), "staff@su.se".to_string());
    role_rule_attribute_observations::observe_for_user(&db, user, &[pair.clone(), pair])
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM role_rule_attribute_observations WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(rows, 1);
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn repeated_code_in_one_catalog_push_is_stored_once(db: PgPool) {
    let entries = [
        ("PROG1".to_string(), "Programmering 1".to_string()),
        ("PROG1".to_string(), "Programming 1".to_string()),
    ];
    assert_eq!(
        play_course_catalog::upsert_many(&db, &entries)
            .await
            .unwrap(),
        1
    );
}
