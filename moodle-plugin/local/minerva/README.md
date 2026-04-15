# Minerva AI Assistant (local_minerva)

Local plugin that connects Moodle to a [Minerva](https://github.com/Edwinexd/minerva) instance for AI-assisted learning.

## Features

- **Course linking**: teachers link a Moodle course to a Minerva course by pasting a per-course API key. The scoped course is resolved automatically from the key.
- **Site-wide Minerva URL**: admins can lock the Minerva URL so teachers only need to enter the API key. Non-https URLs are rejected (with a carve-out for loopback, `host.docker.internal`, and bare single-label hostnames used inside container networks).
- **Enrolment sync (event-driven)**: enrols / unenrols students in Minerva as Moodle enrolments change. Multi-instance enrolments are handled correctly -- a user is only removed from Minerva when no active enrolment remains.
- **Enrolment sync (scheduled + manual)**: every 30 minutes (and on demand from the course manage page) the plugin reconciles both directions -- missing users are added, and Minerva students who are no longer enrolled in Moodle are removed. Teachers and TAs on the Minerva side are never touched.
- **Material sync**: uploads course content (stored files, mod_url targets, mod_page, mod_book chapters, mod_label intros, mod_resource intros, and section summaries) to Minerva for RAG processing. Runs on demand and on a 30-minute schedule (offset 15 minutes from enrolment sync).
- **Housekeeping**:
  - Unlinking a course also clears the per-course sync log so a re-link does a full re-upload.
  - "Reset sync log" button for the same effect without unlinking.
  - `course_deleted` observer cleans up link and sync-log rows automatically.

## Requirements

- Moodle 4.1 or later
- A running Minerva instance with an integration API key

## Installation

1. Download and extract the plugin into `local/minerva/`.
2. Visit *Site administration → Notifications* to finish the install.
3. (Optional) Lock the Minerva URL in *Site administration → Plugins → Local plugins → Minerva AI Assistant* so teachers don't have to enter it per course.

## Assumptions

- Moodle usernames are the user's eppn (e.g. `abcd1234@su.se`). At SU this is how the Shibboleth auth plugin is configured; for local / test Moodle installs, the bare username is used as-is and nothing breaks (no synthetic `@domain` suffix is applied any more).
- The Minerva API key is per-course; the plugin calls `/api/integration/*` with it as a bearer token.

## Scheduled tasks

| Task | Schedule | Purpose |
| --- | --- | --- |
| `sync_enrolments` | every 30 min (`*/30`) | Reconcile Moodle enrolments against Minerva membership. |
| `sync_materials`  | 15 and 45 past the hour | Upload new / changed course resources to Minerva. |

Both tasks respect the `autosync_enrolment` / `autosync_materials` admin toggles.

## Capabilities

- `local/minerva:manage` -- configure the Minerva link for a course (default: editing teacher, manager).
- `local/minerva:view` -- see the AI assistant in a course (default: student, teacher, editing teacher, manager).
- `local/minerva:syncmaterials` -- trigger a material sync or reset the sync log (default: editing teacher, manager).

## Related plugin

This plugin works together with [mod_minerva](https://moodle.org/plugins/mod_minerva), which provides the activity module that teachers add to their course page.

## License

This plugin is licensed under the [GNU GPL v3 or later](https://www.gnu.org/copyleft/gpl.html).
