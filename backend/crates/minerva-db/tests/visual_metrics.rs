use minerva_db::queries::visual_extraction as queue;
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn metrics_distinguish_backlogs_expiry_and_recent_failures(db: sqlx::PgPool) {
    let empty = queue::download_metrics(&db).await.unwrap();
    assert_eq!(
        (
            empty.reserved,
            empty.queued,
            empty.downloading,
            empty.expired
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(empty.earliest_deadline, 0.0);
    let figures = queue::figure_metrics(&db).await.unwrap();
    assert_eq!((figures.pending, figures.indexed), (0, 0));

    let user = Uuid::new_v4();
    let course = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,eppn) VALUES ($1,$2)")
        .bind(user)
        .bind(format!("metrics-{user}@example.invalid"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO courses (id,name,owner_id) VALUES ($1,'metrics-test',$2)")
        .bind(course)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    let mut documents = Vec::new();
    for (kind, status) in [
        ("play_lecture", "fetching"),
        ("play_lecture", "fetching"),
        ("play_lecture", "fetching"),
        ("pdf", "ready"),
        ("pdf", "leased"),
        ("pdf", "done"),
        ("pdf", "done"),
        ("play_lecture", "needs_source"),
        ("play_lecture", "needs_source"),
    ] {
        let doc = Uuid::new_v4();
        sqlx::query("INSERT INTO documents (id,course_id,filename,mime_type,size_bytes,uploaded_by) VALUES ($1,$2,'test.pdf','application/pdf',0,$3)")
            .bind(doc).bind(course).bind(user).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO visual_extraction_jobs (document_id,course_id,kind,status,pipeline_version,updated_at) VALUES ($1,$2,$3,$4,1,NOW()-INTERVAL '10 minutes')")
            .bind(doc).bind(course).bind(kind).bind(status).execute(&db).await.unwrap();
        documents.push(doc);
    }
    sqlx::query("UPDATE visual_extraction_jobs SET staged_bytes=100 WHERE status='fetching'")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE visual_extraction_jobs SET download_url='test',download_deadline=NOW()+INTERVAL '5 minutes' WHERE document_id=$1")
        .bind(documents[1]).execute(&db).await.unwrap();
    sqlx::query("UPDATE visual_extraction_jobs SET download_url='test',download_deadline=NOW()-INTERVAL '1 minute',download_started_at=NOW()-INTERVAL '2 minutes' WHERE document_id=$1")
        .bind(documents[2]).execute(&db).await.unwrap();
    sqlx::query(
        "UPDATE visual_extraction_jobs SET error_msg='retry',attempts=1 WHERE status='ready'",
    )
    .execute(&db)
    .await
    .unwrap();
    // Both lectures were released with an error; the older one is backing
    // off until Play serves it, so only the other is a stuck backlog.
    sqlx::query("UPDATE visual_extraction_jobs SET error_msg='transcript not generated yet' WHERE status='needs_source'")
        .execute(&db).await.unwrap();
    sqlx::query("UPDATE visual_extraction_jobs SET retry_after=NOW()+INTERVAL '1 hour',updated_at=NOW()-INTERVAL '2 days' WHERE document_id=$1")
        .bind(documents[7]).execute(&db).await.unwrap();
    sqlx::query("UPDATE visual_extraction_jobs SET lease_expires_at=NOW()-INTERVAL '1 minute' WHERE status='leased'").execute(&db).await.unwrap();
    for (doc, age) in [(documents[5], 5), (documents[6], 30)] {
        sqlx::query("UPDATE visual_extraction_jobs SET completed_at=NOW()-make_interval(mins=>$2) WHERE document_id=$1")
            .bind(doc).bind(age).execute(&db).await.unwrap();
    }
    for (state, age) in [("queued", 0), ("running", 0), ("failed", 5), ("failed", 30)] {
        sqlx::query("INSERT INTO visual_extraction_workers (id,state,expires_at,finished_at) VALUES ($1,$2,NOW()+INTERVAL '1 day',CASE WHEN $2='failed' THEN NOW()-make_interval(mins=>$3) ELSE NULL END)")
            .bind(Uuid::new_v4()).bind(state).bind(age).execute(&db).await.unwrap();
    }
    for indexed in [false, true] {
        sqlx::query("INSERT INTO document_figures (document_id,course_id,page_position,box,image_path,context,visual_model,visual_vector,created_at,indexed_at) VALUES ($1,$2,0,ARRAY[0,0,1,1]::REAL[],'test.png','test','test',ARRAY[0]::REAL[],NOW()-INTERVAL '3 minutes',CASE WHEN $3 THEN NOW() ELSE NULL END)")
            .bind(documents[5]).bind(course).bind(indexed).execute(&db).await.unwrap();
    }

    let downloads = queue::download_metrics(&db).await.unwrap();
    assert_eq!(
        (
            downloads.reserved,
            downloads.queued,
            downloads.downloading,
            downloads.expired
        ),
        (1, 1, 1, 1)
    );
    assert_eq!(downloads.staging_bytes, 300);
    assert!(downloads.earliest_deadline < chrono::Utc::now().timestamp() as f64);
    assert!((120.0..130.0).contains(&downloads.oldest_started_seconds));
    let jobs = queue::job_metrics(&db).await.unwrap();
    let ready = jobs.iter().find(|j| j.status == "ready").unwrap();
    assert_eq!((ready.count, ready.with_errors), (1, 1));
    assert!((600.0..610.0).contains(&ready.oldest_age_seconds));
    assert_eq!(ready.waiting_on_play, 0);
    let needs = jobs.iter().find(|j| j.status == "needs_source").unwrap();
    assert_eq!(
        (needs.count, needs.waiting_on_play, needs.with_errors),
        (2, 1, 1)
    );
    assert!((600.0..610.0).contains(&needs.oldest_age_seconds));
    assert_eq!(jobs.iter().map(|j| j.completed_last_15m).sum::<i64>(), 1);
    assert_eq!(jobs.iter().map(|j| j.expired_leases).sum::<i64>(), 1);
    let workers = queue::worker_metrics(&db).await.unwrap();
    let failed = workers.iter().find(|w| w.state == "failed").unwrap();
    assert_eq!((failed.count, failed.failed_last_15m), (2, 1));
    let figures = queue::figure_metrics(&db).await.unwrap();
    assert_eq!((figures.pending, figures.indexed), (1, 1));
    assert!((180.0..190.0).contains(&figures.oldest_pending_seconds));

    // Empty queues must reset to zero after draining, including deadlines.
    sqlx::query("DELETE FROM courses WHERE id=$1")
        .bind(course)
        .execute(&db)
        .await
        .unwrap();
    let cleared = queue::download_metrics(&db).await.unwrap();
    assert_eq!(
        (cleared.queued, cleared.downloading, cleared.staging_bytes),
        (0, 0, 0)
    );
    assert_eq!(cleared.earliest_deadline, 0.0);
    assert_eq!(
        queue::figure_metrics(&db)
            .await
            .unwrap()
            .oldest_pending_seconds,
        0.0
    );
}
