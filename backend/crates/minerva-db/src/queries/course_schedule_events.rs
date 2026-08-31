use sqlx::{PgPool, Postgres, QueryBuilder};

pub struct ScheduleEvent<'a> {
    pub uid: &'a str,
    pub event_ical: &'a str,
    pub last_modified: Option<&'a str>,
}

/// Replace one offering's schedule in a transaction. The temporary-table
/// shape makes an empty Daisy calendar meaningful: it deletes every old UID.
pub async fn reconcile(
    db: &PgPool,
    momenttillf_id: &str,
    events: &[ScheduleEvent<'_>],
) -> Result<(u64, u64), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("CREATE TEMP TABLE schedule_uids (uid TEXT PRIMARY KEY) ON COMMIT DROP")
        .execute(&mut *tx)
        .await?;

    let mut upserted = 0;
    if !events.is_empty() {
        let mut qb = QueryBuilder::<Postgres>::new(
            "INSERT INTO course_schedule_events (momenttillf_id, uid, event_ical, last_modified) ",
        );
        qb.push_values(events, |mut b, event| {
            b.push_bind(momenttillf_id)
                .push_bind(event.uid)
                .push_bind(event.event_ical)
                .push_bind(event.last_modified);
        });
        qb.push(" ON CONFLICT (momenttillf_id, uid) DO UPDATE SET event_ical = EXCLUDED.event_ical, last_modified = EXCLUDED.last_modified, synced_at = NOW()")
            .build()
            .execute(&mut *tx)
            .await?;
        upserted = events.len() as u64;

        let mut uid_qb = QueryBuilder::<Postgres>::new("INSERT INTO schedule_uids (uid) ");
        uid_qb.push_values(events, |mut b, event| {
            b.push_bind(event.uid);
        });
        uid_qb
            .push(" ON CONFLICT DO NOTHING")
            .build()
            .execute(&mut *tx)
            .await?;
    }

    let deleted = sqlx::query(
        "DELETE FROM course_schedule_events e WHERE e.momenttillf_id = $1 AND NOT EXISTS (SELECT 1 FROM schedule_uids k WHERE k.uid = e.uid)",
    )
    .bind(momenttillf_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok((upserted, deleted))
}
