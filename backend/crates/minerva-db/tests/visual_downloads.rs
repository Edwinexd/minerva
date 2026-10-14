use chrono::{Duration, Utc};
use minerva_db::queries::visual_extraction::{self as queue, OfferOutcome};
use uuid::Uuid;

/// Offers with an unbounded staging window: these tests are about claims
/// and deadlines; `probed_candidates_shrink_to_the_chosen_source` covers
/// the window.
async fn offer(
    db: &sqlx::PgPool,
    id: Uuid,
    url: &str,
    deadline: chrono::DateTime<Utc>,
    cues: &serde_json::Value,
    size_bytes: Option<i64>,
) -> Result<bool, sqlx::Error> {
    Ok(
        queue::offer_download(db, id, url, deadline, cues, size_bytes, i64::MAX).await?
            == OfferOutcome::Offered,
    )
}

async fn offer_many(
    db: &sqlx::PgPool,
    id: Uuid,
    candidates: &serde_json::Value,
    deadline: chrono::DateTime<Utc>,
    cues: &serde_json::Value,
    reserve_bytes: Option<i64>,
) -> Result<bool, sqlx::Error> {
    Ok(
        queue::offer_candidates(db, id, candidates, deadline, cues, reserve_bytes, i64::MAX)
            .await?
            == OfferOutcome::Offered,
    )
}

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
    assert!(offer(
        &db,
        jobs[0],
        "late",
        now + Duration::minutes(30),
        &cues,
        None
    )
    .await
    .unwrap());
    assert!(offer(
        &db,
        jobs[1],
        "early",
        now + Duration::minutes(10),
        &cues,
        None
    )
    .await
    .unwrap());
    assert!(!offer(
        &db,
        jobs[2],
        "expired",
        now - Duration::seconds(1),
        &cues,
        None
    )
    .await
    .unwrap());
    assert!(!offer(
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
        offer(&db, a.id, "fresh", now + Duration::minutes(15), &cues, None)
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
        offer(&db, b.id, "soon", now + Duration::minutes(5), &cues, None)
            .await
            .unwrap()
    );
    assert!(offer(
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
        offer(&db, attempted[0], "sized", deadline, &cues, Some(GIB / 4))
            .await
            .unwrap()
    );
    assert!(offer(&db, attempted[1], "unknown", deadline, &cues, None)
        .await
        .unwrap());
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
            assert!(
                offer(&db, slot.job_id, "sized", deadline, &cues, Some(GIB / 4))
                    .await
                    .unwrap()
            );
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

#[sqlx::test(migrations = "../../migrations")]
#[ignore = "requires an isolated PostgreSQL DATABASE_URL"]
async fn probed_candidates_shrink_to_the_chosen_source(db: sqlx::PgPool) {
    const GIB: i64 = 1024 * 1024 * 1024;
    let user = Uuid::new_v4();
    let course = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id,eppn) VALUES ($1,$2)")
        .bind(user)
        .bind(format!("probe-test-{user}@example.invalid"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO courses (id,name,owner_id) VALUES ($1,'probe-test',$2)")
        .bind(course)
        .bind(user)
        .execute(&db)
        .await
        .unwrap();
    let mut jobs = Vec::new();
    for _ in 0..2 {
        let doc = Uuid::new_v4();
        let job = Uuid::new_v4();
        sqlx::query("INSERT INTO documents (id,course_id,filename,mime_type,size_bytes,uploaded_by) VALUES ($1,$2,'test.url','text/x-url',0,$3)")
            .bind(doc).bind(course).bind(user).execute(&db).await.unwrap();
        sqlx::query("INSERT INTO visual_extraction_jobs (id,document_id,course_id,kind,status,pipeline_version,fetch_reserved_at,staged_bytes) VALUES ($1,$2,$3,'play_lecture','fetching',1,NOW(),$4)")
            .bind(job).bind(doc).bind(course).bind(GIB).execute(&db).await.unwrap();
        jobs.push(job);
    }
    let cues = serde_json::json!([]);
    let candidates = serde_json::json!([{"source": "a"}, {"source": "b"}]);
    let now = Utc::now();
    // Growing past the 1 GiB placeholder needs room in the window: with
    // both placeholders pending (2 GiB) and 2.5 GiB to spare, 1.5 GiB fits
    // but 2 GiB does not.
    let deadline = now + Duration::minutes(30);
    assert_eq!(
        queue::offer_candidates(
            &db,
            jobs[0],
            &candidates,
            deadline,
            &cues,
            Some(2 * GIB),
            5 * GIB / 2
        )
        .await
        .unwrap(),
        OfferOutcome::NoSpace
    );
    assert_eq!(
        queue::offer_candidates(
            &db,
            jobs[0],
            &candidates,
            deadline,
            &cues,
            Some(3 * GIB / 2),
            5 * GIB / 2
        )
        .await
        .unwrap(),
        OfferOutcome::Offered
    );
    // Reset for the deadline-ordering checks below.
    sqlx::query("UPDATE visual_extraction_jobs SET download_candidates=NULL, download_deadline=NULL, staged_bytes=$2 WHERE id=$1")
        .bind(jobs[0])
        .bind(GIB)
        .execute(&db)
        .await
        .unwrap();
    assert!(offer_many(
        &db,
        jobs[0],
        &candidates,
        now + Duration::minutes(30),
        &cues,
        Some(GIB / 2)
    )
    .await
    .unwrap());
    assert!(offer_many(
        &db,
        jobs[1],
        &candidates,
        now + Duration::minutes(10),
        &cues,
        None
    )
    .await
    .unwrap());
    let staged = |id| {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT staged_bytes FROM visual_extraction_jobs WHERE id=$1",
        )
        .bind(id)
        .fetch_one(&db)
    };
    assert_eq!(staged(jobs[0]).await.unwrap(), Some(GIB / 2));
    // Undecided lectures are probed, never downloaded, earliest expiry first.
    assert!(queue::claim_download(&db).await.unwrap().is_none());
    let early = queue::claim_probe(&db, 900).await.unwrap().unwrap();
    let late = queue::claim_probe(&db, 900).await.unwrap().unwrap();
    assert_eq!((early.id, late.id), (jobs[1], jobs[0]));
    assert!(queue::claim_probe(&db, 900).await.unwrap().is_none());
    // A restarted fetcher takes over its own claims at once.
    assert_eq!(queue::reset_fetch_claims(&db).await.unwrap(), 2);
    assert!(!queue::choose_download(&db, &late, "stale", Some(GIB / 4))
        .await
        .unwrap());
    let early = queue::claim_probe(&db, 900).await.unwrap().unwrap();
    let late = queue::claim_probe(&db, 900).await.unwrap().unwrap();
    assert_eq!((early.id, late.id), (jobs[1], jobs[0]));
    // The winner cannot be larger than what was reserved for the lecture.
    assert!(!queue::choose_download(&db, &late, "slides", Some(GIB))
        .await
        .unwrap());
    assert!(queue::choose_download(&db, &late, "slides", Some(GIB / 4))
        .await
        .unwrap());
    assert_eq!(staged(jobs[0]).await.unwrap(), Some(GIB / 4));
    let download = queue::claim_download(&db).await.unwrap().unwrap();
    assert_eq!((download.id, download.url.as_str()), (jobs[0], "slides"));
    assert_eq!(download.expected_bytes, Some(GIB / 4));
    // A failed probe returns the lecture for a fresh authorization.
    assert!(
        queue::fail_probe(&db, &early, "slide source probe failed (x)", true)
            .await
            .unwrap()
    );
    let failed = queue::find_job(&db, jobs[1]).await.unwrap().unwrap();
    assert_eq!(failed.status, "needs_source");
    let leftover: (Option<serde_json::Value>, Option<chrono::DateTime<Utc>>) = sqlx::query_as(
        "SELECT download_candidates, download_deadline FROM visual_extraction_jobs WHERE id=$1",
    )
    .bind(jobs[1])
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(leftover, (None, None));
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
