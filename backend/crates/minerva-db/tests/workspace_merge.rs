use minerva_db::queries::workspaces::merge_courses;
use sqlx::PgPool;
use uuid::Uuid;

async fn seed_course(db: &PgPool, owner: Uuid) -> Uuid {
    let course = Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id,name,owner_id) VALUES ($1,'merge-test',$2)")
        .bind(course)
        .bind(owner)
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO courses (workspace_id) VALUES ($1)")
        .bind(course)
        .execute(db)
        .await
        .unwrap();
    course
}

async fn seed_usage(db: &PgPool, user: Uuid, course: Uuid, model: &str, prompt_tokens: i64) {
    sqlx::query("INSERT INTO usage_daily (user_id,workspace_id,date,model,provider,prompt_tokens,request_count) VALUES ($1,$2,CURRENT_DATE,$3,'cerebras',$4,1)")
        .bind(user).bind(course).bind(model).bind(prompt_tokens).execute(db).await.unwrap();
}

async fn seed_figure(db: &PgPool, course: Uuid, doc: Uuid, position: i32, person: bool) -> Uuid {
    sqlx::query_scalar("INSERT INTO document_figures (document_id,workspace_id,page_position,box,image_path,context,visual_model,visual_vector,indexed_at,shows_person) VALUES ($1,$2,$3,ARRAY[0,0,1,1]::real[],'crop.jpg','context','clip',ARRAY[0.1,0.2]::real[],NOW(),$4) RETURNING id")
        .bind(doc).bind(course).bind(position).bind(person).fetch_one(db).await.unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn merge_carries_usage_and_visual_rows(db: PgPool) {
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,eppn) VALUES ($1,$2)")
        .bind(user)
        .bind(format!("merge-{user}@example.invalid"))
        .execute(&db)
        .await
        .unwrap();
    let survivor = seed_course(&db, user).await;
    let source = seed_course(&db, user).await;

    seed_usage(&db, user, survivor, "gpt-oss-120b", 10).await;
    seed_usage(&db, user, source, "gpt-oss-120b", 5).await;
    seed_usage(&db, user, source, "other-model", 7).await;

    let doc = Uuid::new_v4();
    sqlx::query("INSERT INTO documents (id,workspace_id,filename,mime_type,size_bytes,status,uploaded_by) VALUES ($1,$2,'slides.pdf','application/pdf',100,'ready',$3)")
        .bind(doc).bind(source).bind(user).execute(&db).await.unwrap();
    sqlx::query("INSERT INTO visual_extraction_jobs (document_id,workspace_id,kind,status,pipeline_version) VALUES ($1,$2,'pdf','done',1)")
        .bind(doc).bind(source).execute(&db).await.unwrap();
    let figure = seed_figure(&db, source, doc, 0, false).await;
    let person = seed_figure(&db, source, doc, 1, true).await;

    let outcome = merge_courses(&db, survivor, source).await.unwrap();
    assert_eq!(outcome.documents_moved, 1);
    assert_eq!(outcome.documents_requeued, 1);

    let usage: Vec<(String, i64, i32)> = sqlx::query_as(
        "SELECT model,prompt_tokens,request_count FROM usage_daily WHERE workspace_id=$1 ORDER BY model",
    )
    .bind(survivor)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(
        usage,
        vec![
            ("gpt-oss-120b".to_string(), 15, 2),
            ("other-model".to_string(), 7, 1),
        ]
    );

    let job_course: Uuid =
        sqlx::query_scalar("SELECT workspace_id FROM visual_extraction_jobs WHERE document_id=$1")
            .bind(doc)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(job_course, survivor);

    let figure_state = |id: Uuid| {
        sqlx::query_as::<_, (Uuid, bool)>(
            "SELECT workspace_id, indexed_at IS NOT NULL FROM document_figures WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&db)
    };
    // A figure goes back through indexing to reach the survivor's search; a
    // person crop must not, or it would be treated as awaiting removal.
    assert_eq!(figure_state(figure).await.unwrap(), (survivor, false));
    assert_eq!(figure_state(person).await.unwrap(), (survivor, true));

    let left_on_source: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM usage_daily WHERE workspace_id=$1)
              + (SELECT COUNT(*) FROM document_figures WHERE workspace_id=$1)
              + (SELECT COUNT(*) FROM visual_extraction_jobs WHERE workspace_id=$1)",
    )
    .bind(source)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(left_on_source, 0);
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn merge_clears_a_colliding_url_on_an_already_orphaned_document(db: PgPool) {
    let user = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,eppn) VALUES ($1,$2)")
        .bind(user)
        .bind(format!("merge-{user}@example.invalid"))
        .execute(&db)
        .await
        .unwrap();
    let survivor = seed_course(&db, user).await;
    let source = seed_course(&db, user).await;
    let orphan = Uuid::new_v4();
    for (id, course, orphaned) in [(Uuid::new_v4(), survivor, false), (orphan, source, true)] {
        sqlx::query("INSERT INTO documents (id,workspace_id,filename,mime_type,size_bytes,uploaded_by,source_url,orphaned_at) VALUES ($1,$2,'lecture.url','text/x-url',0,$3,'https://play.dsv.su.se/x',CASE WHEN $4 THEN NOW() END)")
            .bind(id).bind(course).bind(user).bind(orphaned).execute(&db).await.unwrap();
    }

    merge_courses(&db, survivor, source).await.unwrap();

    let (course, url): (Uuid, Option<String>) =
        sqlx::query_as("SELECT workspace_id,source_url FROM documents WHERE id=$1")
            .bind(orphan)
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!((course, url), (survivor, None));
}
