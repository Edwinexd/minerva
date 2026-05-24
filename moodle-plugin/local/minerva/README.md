# Minerva AI Assistant (local_minerva)

Local plugin that connects Moodle to a [Minerva](https://github.com/Edwinexd/minerva) instance for AI-assisted learning.

## Features

- **Course linking**: two modes, picked automatically from settings.
  - **Legacy** (default): teacher pastes a per-course API key; the scoped course is resolved automatically from the key.
  - **Site integration** (when admins set `site_api_key`): teacher picks from a dropdown of Minerva courses they own or teach; the plugin mints the per-course key on their behalf via `/api/integration/site/provision` and stores it like any other per-course key.
- **Site-wide Minerva URL**: admins can lock the Minerva URL so teachers only need to enter the API key. Non-https URLs are rejected (with a carve-out for loopback, `host.docker.internal`, and bare single-label hostnames used inside container networks).
- **Material sync**: uploads course content (stored files, mod_url targets, mod_page, mod_book chapters, mod_label intros, mod_resource intros, and section summaries) to Minerva for RAG processing. Runs on demand and twice an hour (at 15 and 45 past).
- **Source-identity tracking**: every uploaded resource is tagged with a stable `source_ref` (e.g. `page:cm:42`, `mod_file:cm:7:/lecture3.pdf`). When the underlying Moodle object changes, the previous Minerva doc is soft-orphaned and the new one supersedes it; when the object is deleted, the doc is orphaned too. Orphaned docs are excluded from new chat retrievals but kept so existing chat-history citations still resolve.
- **Delete mirroring (two layers)**:
  - **Event observer**: `course_module_deleted` fires immediately, posting the affected `source_ref`s to the Minerva `/orphan` endpoint. Low-latency, so students stop seeing stale material on the next chat turn rather than waiting for the next cron tick.
  - **Periodic reconcile sweep**: every sync run posts the current set of `source_ref`s; Minerva orphans anything no longer in the list. Safety net for whatever the observer missed (bulk delete, restore-from-backup gaps, plugin-disabled windows, feature-toggle-off).
- **Forum conversations (opt-in)**: teacher-answered discussions can be uploaded as one document per forum. Two-level opt-in:
  - Admin enables it site-wide in plugin settings (default ON; flip OFF as a kill switch).
  - Each teacher then opts their own course in via a button on the manage page (default OFF).
  - Only threads where a teacher (`editingteacher` or `teacher` role) has posted at least once are included. Student-only threads are skipped.
  - Student names from the course roster (firstname, lastname, and username local-part for `ext:`-style accounts) are stripped from post bodies before upload, replaced with `[student]`. Length floor (3 characters) prevents false positives on very short names. Teacher names are preserved.
  - The per-course toggle is gated on the site setting: if the admin disables it site-wide, the per-course control vanishes from the manage page and the sync task skips forums regardless of any per-course value.

> **Enrolment / membership is not handled by this plugin.** Course membership is provisioned by Minerva itself on LTI launch (and reconciled via NRPS), so the plugin only pushes materials.
- **Housekeeping**:
  - Unlinking a course also clears the per-course sync log; the next sync re-discovers every Moodle object and re-POSTs it. The Minerva server's `(course, content_hash)` dedup index decides what's actually new, so unchanged bytes are no-ops on the server side.
  - "Reset sync log" button does the same without unlinking. Useful when the local optimisation cache and Minerva's view have drifted (e.g. after a manual delete on the Minerva side).
  - `course_deleted` observer cleans up link and sync-log rows automatically.

## Requirements

- Moodle 4.5 LTS
- A running Minerva instance with an integration API key (per-course) or a site integration key (multi-course provisioning).

## Installation

1. Download and extract the plugin into `local/minerva/`.
2. Visit *Site administration -> Notifications* to finish the install.
3. (Optional) Lock the Minerva URL in *Site administration -> Plugins -> Local plugins -> Minerva AI Assistant* so teachers don't have to enter it per course.
4. (Optional) Flip the **Enable forum sync** admin setting OFF if you don't want teachers in your installation to be able to opt-in to forum-conversation indexing at all. Default ON.

## Assumptions

- Moodle usernames are the user's eppn (e.g. `abcd1234@su.se`). At SU this is how the Shibboleth auth plugin is configured; for local / test Moodle installs, the bare username is used as-is and nothing breaks (no synthetic `@domain` suffix is applied any more).
- The Minerva API key is per-course; the plugin calls `/api/integration/*` with it as a bearer token.

## Scheduled tasks

| Task | Schedule | Purpose |
| --- | --- | --- |
| `sync_materials` | 15 and 45 past the hour | Discover, upload, and reconcile course resources (and, when opted in, forum conversations) against Minerva. |

The task respects the `autosync_materials` admin toggle. The reconcile sweep runs at the end of every invocation regardless of whether new items were uploaded.

## Settings

| Setting | Default | Effect |
| --- | --- | --- |
| `minerva_url` | empty | Lock the Minerva URL site-wide. When set, teachers can't edit it per course. |
| `site_api_key` | empty | Optional site-integration key. When set, teachers pick from a course dropdown instead of pasting an API key. |
| `autosync_materials` | ON | Disable to pause the scheduled task without removing it. |
| `enable_forum_sync` | ON | Kill switch for forum syncing. When OFF, the per-course toggle is hidden and forums are never read. |

## Capabilities

- `local/minerva:manage`: required to view the manage page at all and to link / unlink the course (default: editing teacher, manager).
- `local/minerva:syncmaterials`: required for the per-course controls rendered on the manage page ; "Sync materials", "Reset sync log", and the "Sync conversations" forum toggle. The manage page only renders these for users who hold this capability (default: editing teacher, manager).

## Source-identity reference

The plugin tags every uploaded item with a `source_ref` Minerva uses for per-object versioning and delete tracking. The schema:

| Item kind | `source_ref` format |
| --- | --- |
| Stored module file | `mod_file:cm:<cmid>:<filepath><filename>` |
| `mod_url` external URL | `url:cm:<cmid>` |
| `mod_page` | `page:cm:<cmid>` |
| `mod_book` chapter | `book_chapter:<chapterid>` |
| `mod_label` | `label:cm:<cmid>` |
| `mod_resource` intro | `resource_intro:cm:<cmid>` |
| Section summary | `section:<sectionid>` |
| Forum (opt-in) | `forum:<forumid>` |

Minerva keeps at most one *active* document per `(course, source_system='moodle', source_ref)`. A re-upload with the same ref but different bytes orphans the previous doc and supersedes it.

## Surfacing the assistant to students

This plugin only links a course and pushes materials. Students reach the Minerva assistant via an **LTI launch**, configured as an external tool in Moodle, not via this plugin.

## License

This plugin is licensed under the [GNU GPL v3 or later](https://www.gnu.org/copyleft/gpl.html).
