use minerva_db::queries::{user_eppn_aliases, users};
use rust_decimal::Decimal;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use tokio::sync::Barrier;
use uuid::Uuid;

/// Database-backed identity regression coverage.  It is ignored by default
/// because it intentionally migrates and writes to DATABASE_URL; CI or local
/// verification runs it explicitly against a disposable database.
#[tokio::test]
#[ignore = "requires a disposable PostgreSQL DATABASE_URL"]
async fn aliases_and_primaries_share_one_concurrent_namespace() {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let db = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await
        .unwrap();
    sqlx::migrate!("../../migrations").run(&db).await.unwrap();

    let suffix = Uuid::new_v4();
    let primary = format!("identity-primary-{suffix}@example.invalid");
    let alias = format!("identity-alias-{suffix}@example.invalid");
    let raced = format!("identity-race-{suffix}@example.invalid");

    let (owner, created) = users::find_or_create_by_eppn(
        &db,
        &primary,
        Some("Identity Test"),
        "student",
        Decimal::ZERO,
    )
    .await
    .unwrap();
    assert!(created);
    assert!(user_eppn_aliases::register(&db, owner.id, &alias)
        .await
        .unwrap());

    let (resolved_alias, created) =
        users::find_or_create_by_eppn(&db, &alias, None, "student", Decimal::ZERO)
            .await
            .unwrap();
    assert!(!created);
    assert_eq!(resolved_alias.id, owner.id);

    let direct_duplicate = sqlx::query(
        "INSERT INTO users (id, eppn, role, owner_daily_cost_limit_usd) \
         VALUES ($1, $2, 'student', 0)",
    )
    .bind(Uuid::new_v4())
    .bind(&alias)
    .execute(&db)
    .await
    .unwrap_err();
    assert_eq!(
        direct_duplicate
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("user_eppn_registry_pkey")
    );

    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let db = db.clone();
        let raced = raced.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            users::find_or_create_by_eppn(
                &db,
                &raced,
                Some("Concurrent Identity"),
                "student",
                Decimal::ZERO,
            )
            .await
            .unwrap()
        }));
    }
    barrier.wait().await;
    let left = tasks.remove(0).await.unwrap();
    let right = tasks.remove(0).await.unwrap();
    assert_eq!(left.0.id, right.0.id);
    assert_ne!(left.1, right.1);

    user_eppn_aliases::swap_primary_with_alias(&db, owner.id, &alias)
        .await
        .unwrap();
    let promoted = users::find_by_id(&db, owner.id).await.unwrap().unwrap();
    assert_eq!(promoted.eppn, alias);
    let (old_primary_owner, via_alias) = users::find_by_eppn_or_alias(&db, &primary)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_primary_owner.id, owner.id);
    assert!(via_alias);

    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(&[owner.id, left.0.id][..])
        .execute(&db)
        .await
        .unwrap();
}
