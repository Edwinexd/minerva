//! The `query!` macros only PREPARE, which never reaches the planner. Plan
//! every cached statement so errors raised there, such as an `ON CONFLICT`
//! target with no matching index, fail here instead of in production.

use sqlx::PgPool;

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn every_cached_query_plans_against_the_migrated_schema(db: PgPool) {
    let cache = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.sqlx");
    let mut planned = 0;
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(cache).unwrap() {
        let path = entry.unwrap().path();
        let cached: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let query = cached["query"].as_str().unwrap();
        let explain = format!("EXPLAIN (GENERIC_PLAN) {query}");
        if let Err(error) = sqlx::raw_sql(sqlx::AssertSqlSafe(explain))
            .execute(&db)
            .await
        {
            failures.push(format!("{}: {error}", path.display()));
        }
        planned += 1;
    }
    assert!(planned > 0, "no cached queries found in {cache}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
