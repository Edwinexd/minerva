-- Daisy course schedules are snapshots, not an append-only feed.  Each
-- VEVENT's stable RFC 5545 UID is the external identity; a sync upserts all
-- events in the new snapshot and removes UIDs which disappeared.
CREATE TABLE course_schedule_events (
    momenttillf_id TEXT NOT NULL,
    uid TEXT NOT NULL,
    event_ical TEXT NOT NULL,
    last_modified TEXT,
    synced_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (momenttillf_id, uid),
    FOREIGN KEY (momenttillf_id)
        REFERENCES course_daisy_offerings(momenttillf_id) ON DELETE CASCADE
);

CREATE INDEX idx_course_schedule_events_offering
    ON course_schedule_events(momenttillf_id);
