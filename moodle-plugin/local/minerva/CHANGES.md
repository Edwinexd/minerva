# Changelog

## v0.6.1 (2026-04-15)

- Clear `local_minerva_sync_log` rows when a course is unlinked (previously re-linking silently skipped everything)
- New "Reset sync log" button on the manage page to force a full re-upload without unlinking

## v0.6.0 (2026-04-15)

- Reject non-https Minerva URLs (http only accepted for localhost)
- Enrolment sync now reconciles both directions: stale Minerva student members are removed, not just new ones added
- Event observer: don't remove from Minerva when a user still has another active enrolment in the course
- New `course_deleted` observer cleans up link + sync log rows
- Convert unlink / sync buttons from GET to POST
- Guard empty course list from the API (no more fatal on `reset(false)`)
- Cache the scoped course from form validation to avoid a duplicate API call
- Scrub uploaded HTML via HTML Purifier before shipping to Minerva
- `safe_slug` now preserves Swedish (and other UTF-8) characters in filenames
- De-dupe sync items within a batch to avoid UNIQUE(courseid, contenthash) crashes
- Guard `tempnam()` failure in the material sync loop
- Offset the materials scheduled task by 15 minutes from the enrolment task
- Truncate API response bodies included in exception debuginfo

## v0.5.1 (2026-03-30)

- Simplify course linking: API key auto-resolves to its scoped course
- Site admin can lock Minerva URL globally
- Add public URL setting for embed iframe (local development)
- Add material sync scheduled task
- Remove old nav-based view in favour of mod_minerva activity module

## v0.1.0 (2026-03-30)

- Initial release
- Course linking with API key
- Enrolment sync (event-driven and scheduled)
- Manual material sync
- Navigation integration
