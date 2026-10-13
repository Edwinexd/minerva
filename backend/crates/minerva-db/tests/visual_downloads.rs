use chrono::{Duration, Utc};
use minerva_db::queries::visual_extraction as queue;
use uuid::Uuid;

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn downloads_follow_deadlines_and_reject_stale_claims(db: sqlx::PgPool) {
    let user = Uuid::new_v4();
    let course = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,eppn) VALUES ($1,$2)")
        .bind(user)
        .bind(format!("download-test-{user}@example.invalid"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO courses (id,name,owner_id) VALUES ($1,'download-test',$2)")
        .bind(course)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    let mut jobs = Vec::new();
    for _ in 0..3 {
        let doc = Uuid::new_v4();
        let job = Uuid::new_v4();
        sqlx::query("INSERT INTO documents (id,course_id,filename,mime_type,size_bytes,uploaded_by) VALUES ($1,$2,'test.url','text/x-url',0,$3)")
            .bind(doc).bind(course).bind(user).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO visual_extraction_jobs (id,document_id,course_id,kind,status,pipeline_version,fetch_reserved_at,staged_bytes) VALUES ($1,$2,$3,'play_lecture','fetching',1,NOW(),1073741824)")
            .bind(job).bind(doc).bind(course).execute(&db).await.unwrap();
        jobs.push(job);
    }
    let cues = serde_json::json!([]);
    let now = Utc::now();
    assert!(queue::offer_download(
        &db,
        jobs[0],
        "late",
        now + Duration::minutes(30),
        &cues,
        None
    )
    .await
    .unwrap());
    assert!(queue::offer_download(
        &db,
        jobs[1],
        "early",
        now + Duration::minutes(10),
        &cues,
        None
    )
    .await
    .unwrap());
    assert!(!queue::offer_download(
        &db,
        jobs[2],
        "expired",
        now - Duration::seconds(1),
        &cues,
        None
    )
    .await
    .unwrap());
    assert!(!queue::offer_download(
        &db,
        jobs[1],
        "duplicate",
        now + Duration::minutes(20),
        &cues,
        None,
    )
    .await
    .unwrap());
    let (a, b) = tokio::join!(queue::claim_download(&db), queue::claim_download(&db));
    let a = a.unwrap().unwrap();
    let b = b.unwrap().unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(
        [a.id, b.id]
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
        [jobs[0], jobs[1]].into_iter().collect()
    );
    assert!(queue::claim_download(&db).await.unwrap().is_none());
    assert!(!queue::release_fetch(&db, a.id, "late Actions retry", true)
        .await
        .unwrap());
    // Simulate a downloader crash, recover it, and authorize a fresh URL.
    sqlx::query("UPDATE visual_extraction_jobs SET download_started_at=NOW()-INTERVAL '40 minutes' WHERE id=$1")
        .bind(a.id).execute(&db).await.unwrap();
    queue::release_stale_fetches(&db, 7200).await.unwrap();
    sqlx::query(
        "UPDATE visual_extraction_jobs SET status='fetching',fetch_reserved_at=NOW() WHERE id=$1",
    )
    .bind(a.id)
    .execute(&db)
    .await
    .unwrap();
    assert!(
        queue::offer_download(&db, a.id, "fresh", now + Duration::minutes(15), &cues, None)
            .await
            .unwrap()
    );
    let fresh = queue::claim_download(&db).await.unwrap().unwrap();
    assert_eq!(fresh.id, a.id);
    assert!(!queue::finish_download(&db, &a, Some("stale.mp4"), 100)
        .await
        .unwrap());
    assert!(queue::finish_download(&db, &fresh, Some("fresh.mp4"), 100)
        .await
        .unwrap());
    assert!(queue::finish_download(&db, &b, None, 0).await.unwrap());
    // Only the released job is eligible for authorization again.
    sqlx::query(
        "UPDATE visual_extraction_jobs SET status='fetching',fetch_reserved_at=NOW() WHERE id=$1",
    )
    .bind(b.id)
    .execute(&db)
    .await
    .unwrap();
    assert!(
        queue::offer_download(&db, b.id, "soon", now + Duration::minutes(5), &cues, None)
            .await
            .unwrap()
    );
    assert!(queue::offer_download(
        &db,
        jobs[2],
        "later",
        now + Duration::minutes(20),
        &cues,
        None
    )
    .await
    .unwrap());
    assert_eq!(queue::claim_download(&db).await.unwrap().unwrap().id, b.id);
    // Expired authorizations return for renewal, never become worker-ready.
    sqlx::query(
        "UPDATE visual_extraction_jobs SET download_deadline=NOW()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(jobs[2])
    .execute(&db)
    .await
    .unwrap();
    queue::release_stale_fetches(&db, 7200).await.unwrap();
    assert_eq!(
        queue::find_job(&db, jobs[2]).await.unwrap().unwrap().status,
        "needs_source"
    );
    sqlx::query("DELETE FROM courses WHERE id=$1")
        .bind(course)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn sized_reservations_refill_space_and_exclude_failed_sources(db: sqlx::PgPool) {
    const GIB: i64 = 1024 * 1024 * 1024;
    let user = Uuid::new_v4();
    let course = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,eppn) VALUES ($1,$2)")
        .bind(user)
        .bind(format!("sizing-test-{user}@example.invalid"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO courses (id,name,owner_id) VALUES ($1,'sizing-test',$2)")
        .bind(course)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    for _ in 0..12 {
        let doc = Uuid::new_v4();
        sqlx::query("INSERT INTO documents (id,course_id,filename,mime_type,size_bytes,uploaded_by) VALUES ($1,$2,'test.url','text/x-url',0,$3)")
            .bind(doc).bind(course).bind(user).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO visual_extraction_jobs (id,document_id,course_id,kind,status,pipeline_version) VALUES ($1,$2,$3,'play_lecture','needs_source',1)")
            .bind(Uuid::new_v4()).bind(doc).bind(course).execute(&db).await.unwrap();
    }
    // Thirteen GiB free, ten GiB guard: only three unknown videos fit.
    let (a, b) = tokio::join!(
        queue::reserve_fetch_slots(&db, 13 * GIB, 10 * GIB, GIB, 2, &[]),
        queue::reserve_fetch_slots(&db, 13 * GIB, 10 * GIB, GIB, 2, &[]),
    );
    let slots: Vec<_> = a.unwrap().into_iter().chain(b.unwrap()).collect();
    assert_eq!(slots.len(), 3);
    let mut attempted: Vec<_> = slots.iter().map(|s| s.job_id).collect();
    let deadline = Utc::now() + Duration::minutes(30);
    let cues = serde_json::json!([]);
    assert!(
        queue::offer_download(&db, attempted[0], "sized", deadline, &cues, Some(GIB / 4))
            .await
            .unwrap()
    );
    assert!(
        queue::offer_download(&db, attempted[1], "unknown", deadline, &cues, None)
            .await
            .unwrap()
    );
    assert!(
        queue::release_fetch(&db, attempted[2], "probe failed", true)
            .await
            .unwrap()
    );
    let mut accepted = 2;
    loop {
        let slots = queue::reserve_fetch_slots(&db, 13 * GIB, 10 * GIB, GIB, 4, &attempted)
            .await
            .unwrap();
        if slots.is_empty() {
            break;
        }
        for slot in slots {
            assert!(!attempted.contains(&slot.job_id));
            attempted.push(slot.job_id);
            assert!(queue::offer_download(
                &db,
                slot.job_id,
                "sized",
                deadline,
                &cues,
                Some(GIB / 4)
            )
            .await
            .unwrap());
            accepted += 1;
        }
    }
    assert_eq!(accepted, 6);
    let pending: i64 = sqlx::query_scalar(
        "SELECT SUM(staged_bytes)::BIGINT FROM visual_extraction_jobs WHERE status='fetching'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(pending, GIB + 5 * GIB / 4);
    let mut sized_claims = 0;
    let mut unknown_claims = 0;
    while let Some(claim) = queue::claim_download(&db).await.unwrap() {
        match claim.expected_bytes {
            Some(size) => {
                assert_eq!(size, GIB / 4);
                sized_claims += 1;
            }
            None => {
                unknown_claims += 1;
            }
        }
    }
    assert_eq!((sized_claims, unknown_claims), (5, 1));
}
