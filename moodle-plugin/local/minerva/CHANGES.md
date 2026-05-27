# Changelog

## v1.2.0 (2026-05-24)

- **Server-side dedup**: every upload now carries a stable Moodle source_ref so Minerva can keep at most one active document per Moodle object. Re-uploading the same bytes (same hash, same ref) is now idempotent against the server and no longer relies on the client-side `local_minerva_sync_log` as the source of truth.
- **Orphan-on-replace**: editing a Moodle page / label / file / etc. orphans the previous Minerva doc and uploads a new one under the same source_ref. Orphaned docs stay around so old chat-history citations still resolve, but are excluded from new retrievals.
- **Low-latency delete mirroring**: `course_module_deleted` observer now POSTs the affected source_refs to Minerva's `/orphan` endpoint, so deleting an activity in Moodle stops it surfacing in chat answers immediately (not on the next cron tick).
- **Reconcile sweep**: every sync run posts the current set of source_refs to Minerva; anything not in the list gets orphaned. Catches what the observer missed (bulk delete, restore-from-backup gaps, plugin-disabled windows, opt-in toggle flips). Reconcile is discovery-only ; the local sync_log is no longer consulted as a keep-list fallback, so toggling a feature off does orphan its prior uploads.
- **Forum sync (two-level opt-in)**: new admin setting `enable_forum_sync` (default ON) gates a per-course teacher toggle (default OFF). When both are on, teacher-answered discussions are serialised as one HTML doc per forum (source_ref `forum:<id>`).
  - Only discussions where a teacher (`editingteacher` or `teacher` role) has posted at least once are included. Student-only threads are skipped.
  - Student names from the course roster (firstname, lastname, ext: username local-part) are stripped from post bodies before upload, replaced with `[student]`. Teacher names are preserved. Length floor of 3 characters prevents false positives on very short names.
  - Site-level kill switch wins: flipping `enable_forum_sync` OFF removes the per-course toggle from the UI and the sync task skips forums regardless of per-course value. Any previously-uploaded forum docs are orphaned by the reconcile sweep on the next run.
- New `local_minerva_sync_log.sourceref` column (added via upgrade) so the plugin can observer-delete by source_ref without re-discovering the cm.
- New `local_minerva_links.sync_forums` column for the per-course toggle.

## v1.1.0 (2026-05-22)

- Drop enrolment/membership sync entirely. Course membership is now provisioned by Minerva on LTI launch (and reconciled via NRPS), so the plugin no longer adds or removes members.
- Removed the `sync_enrolments` scheduled task, the `user_enrolment_created` / `user_enrolment_deleted` event observers, the "Sync enrolment now" manage-page button, and the `autosync_enrolment` admin setting.
- Removed the now-unused member endpoints (`ensure_user`, `list_members`, `add_member`, `remove_member`) from the API client.
- Removed the deprecated embed/iframe surface: the `create_embed_token` API method, the `local/minerva:view` capability, and the unused `chat_*` / `minerva_assistant` / `open_in_new_tab` strings. The assistant is surfaced to students via LTI launch, not this plugin.
- The plugin is now materials-only: it links a course and pushes content to Minerva.

## v0.7.1 (2026-04-22)

- Target Moodle 4.5 LTS explicitly (`requires` 4.5, `supported = [405, 405]`). Older Moodles are no longer supported since DSV runs only 4.5 LTS.

## v0.7.0 (2026-04-22)

- New optional site-level integration key (`Site integration key` in the plugin settings). When set, teachers link courses by picking from a dropdown of Minerva courses they own or teach instead of pasting an API key; the plugin mints a per-course key on their behalf via `/api/integration/site/provision`.
- Legacy per-course paste flow still works when the site key is unset.

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
