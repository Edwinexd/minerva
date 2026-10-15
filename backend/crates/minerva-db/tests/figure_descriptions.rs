use minerva_db::queries::{figure_descriptions as queue, visual_extraction as figures};
use uuid::Uuid;

async fn fixture(db: &sqlx::PgPool) -> (Uuid, Uuid, Uuid) {
    let user = Uuid::new_v4();
    let course = Uuid::new_v4();
    let doc = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,eppn,display_name) VALUES ($1,$2,'Teacher')")
        .bind(user)
        .bind(format!("{user}@example.com"))
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO courses (id,name,owner_id) VALUES ($1,'Figures',$2)")
        .bind(course)
        .bind(user)
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO documents (id,course_id,filename,mime_type,size_bytes,status,uploaded_by) VALUES ($1,$2,'slides.pdf','application/pdf',100,'ready',$3)")
        .bind(doc).bind(course).bind(user).execute(db).await.unwrap();
    for position in 0..2 {
        sqlx::query("INSERT INTO document_figures (document_id,course_id,page_position,box,image_path,context,visual_model,visual_vector,indexed_at) VALUES ($1,$2,$3,ARRAY[0,0,1,1]::real[],'crop.jpg','Whole page text','clip',ARRAY[0.1,0.2]::real[],NOW())")
            .bind(doc).bind(course).bind(position).execute(db).await.unwrap();
    }
    (user, course, doc)
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn descriptions_have_independent_leases_and_filter_decorations(db: sqlx::PgPool) {
    let (_, course, doc) = fixture(&db).await;
    queue::enqueue(&db, "vision", 1, 64).await.unwrap();
    let a = queue::claim(&db, "vision", 1).await.unwrap().unwrap();
    let b = queue::claim(&db, "vision", 1).await.unwrap().unwrap();
    assert_ne!(a.figure_id, b.figure_id);
    assert!(queue::claim(&db, "vision", 1).await.unwrap().is_none());
    assert!(
        figures::find_chat_figures(&db, course, &[a.figure_id, b.figure_id], false)
            .await
            .unwrap()
            .is_empty()
    );
    // Expiry allows a new attempt and rejects the previous lease's result.
    sqlx::query("UPDATE figure_descriptions SET lease_expires_at=NOW()-INTERVAL '1 second' WHERE figure_id=$1")
        .bind(a.figure_id).execute(&db).await.unwrap();
    let new_a = queue::claim(&db, "vision", 1).await.unwrap().unwrap();
    assert!(!queue::complete(&db, &a, "Old", "Old", false).await.unwrap());
    assert!(queue::complete(
        &db,
        &new_a,
        "Attention blocks connected by residual arrows",
        "Transformer architecture",
        false
    )
    .await
    .unwrap());
    assert!(
        queue::complete(&db, &b, "Generic question mark", "Question mark", true)
            .await
            .unwrap()
    );
    let found = figures::find_chat_figures(&db, course, &[b.figure_id, a.figure_id], false)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].caption.as_deref(),
        Some("Transformer architecture")
    );
    assert_eq!(
        found[0].context,
        "Attention blocks connected by residual arrows"
    );
    let pending = figures::list_unindexed_figures(&db, 10).await.unwrap();
    assert_eq!(pending.len(), 2);
    let substantive = pending.iter().find(|f| !f.is_decorative).unwrap();
    assert_eq!(substantive.id, a.figure_id);
    let old_description_time = substantive.described_at;
    // Re-running enrichment leaves extraction intact; deleting the source
    // cascades through both crops and their cache/queue records.
    queue::enqueue(&db, "new-vision", 2, 64).await.unwrap();
    assert!(queue::claim(&db, "vision", 1).await.unwrap().is_none());
    let first = queue::claim(&db, "new-vision", 2).await.unwrap().unwrap();
    let second = queue::claim(&db, "new-vision", 2).await.unwrap().unwrap();
    let updated = if first.figure_id == a.figure_id {
        first
    } else {
        second
    };
    queue::complete(
        &db,
        &updated,
        "A more precise description",
        "Updated diagram",
        false,
    )
    .await
    .unwrap();
    // An old indexing pass must not mark the new description as indexed.
    figures::mark_figures_indexed(&db, &[a.figure_id], &[old_description_time])
        .await
        .unwrap();
    let still_pending = figures::list_unindexed_figures(&db, 10).await.unwrap();
    assert_eq!(still_pending.len(), 1);
    assert_eq!(still_pending[0].context, "A more precise description");
    let crops: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM document_figures")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(crops, 2);
    sqlx::query("DELETE FROM documents WHERE id=$1")
        .bind(doc)
        .execute(&db)
        .await
        .unwrap();
    let cached: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM figure_descriptions")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(cached, 0);
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn capacity_and_spending_caps_defer_without_exhausting_retries(db: sqlx::PgPool) {
    fixture(&db).await;
    queue::enqueue(&db, "vision", 1, 64).await.unwrap();
    let a = queue::claim(&db, "vision", 1).await.unwrap().unwrap();
    queue::defer_for_capacity(&db, &a).await.unwrap();
    let b = queue::claim(&db, "vision", 1).await.unwrap().unwrap();
    queue::defer_for_budget(&db, &b).await.unwrap();
    assert!(queue::claim(&db, "vision", 1).await.unwrap().is_none());
    let attempts: i64 = sqlx::query_scalar("SELECT SUM(attempts)::bigint FROM figure_descriptions")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(attempts, 0);
    // A malformed result consumes a retry and retries later, up to five.
    sqlx::query("UPDATE figure_descriptions SET retry_at=NOW(),attempts=4 WHERE figure_id=$1")
        .bind(a.figure_id)
        .execute(&db)
        .await
        .unwrap();
    let a = queue::claim(&db, "vision", 1).await.unwrap().unwrap();
    queue::fail(&db, &a, "Invalid result").await.unwrap();
    sqlx::query("UPDATE figure_descriptions SET retry_at=NOW() WHERE figure_id=$1")
        .bind(a.figure_id)
        .execute(&db)
        .await
        .unwrap();
    assert!(queue::claim(&db, "vision", 1).await.unwrap().is_none());
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn flex_variant_prices_drive_owner_cost_without_discount_logic(db: sqlx::PgPool) {
    let (owner, course, _) = fixture(&db).await;
    sqlx::query("INSERT INTO chat_models(model,provider,display_name,input_usd_per_mtok,output_usd_per_mtok) VALUES ('vision','openai','Vision',0.10,0.50),('vision@flex','openai','Vision (Flex)',0.05,0.25)").execute(&db).await.unwrap();
    minerva_db::queries::course_token_usage::record(
        &db,
        course,
        "figure_description",
        "vision@flex",
        1000,
        200,
    )
    .await
    .unwrap();
    let cost = minerva_db::queries::usage::get_owner_daily_cost(&db, owner)
        .await
        .unwrap();
    assert_eq!(cost, rust_decimal::Decimal::new(1, 4));
    let rows = minerva_db::queries::usage::get_owner_pipeline_usage(&db, owner, 30)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cost_usd, cost);
}
