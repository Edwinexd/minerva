# Minerva AI Assistant (local_minerva)

Local plugin that connects Moodle to a [Minerva](https://github.com/Edwinexd/minerva) instance for AI-assisted learning.

## Features

- **Course linking**: teachers link a Moodle course to a Minerva course by pasting a per-course API key. The scoped course is resolved automatically from the key.
- **Site-wide Minerva URL**: admins can lock the Minerva URL so teachers only need to enter the API key. Non-https URLs are rejected (with a carve-out for loopback, `host.docker.internal`, and bare single-label hostnames used inside container networks).
- **Material sync**: uploads course content (stored files, mod_url targets, mod_page, mod_book chapters, mod_label intros, mod_resource intros, and section summaries) to Minerva for RAG processing. Runs on demand and twice an hour (at 15 and 45 past).

> **Enrolment / membership is not handled by this plugin.** Course membership is provisioned by Minerva itself on LTI launch (and reconciled via NRPS), so the plugin only pushes materials.
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
| `sync_materials`  | 15 and 45 past the hour | Upload new / changed course resources to Minerva. |

The task respects the `autosync_materials` admin toggle.

## Capabilities

- `local/minerva:manage`: configure the Minerva link for a course (default: editing teacher, manager).
- `local/minerva:syncmaterials`: trigger a material sync or reset the sync log (default: editing teacher, manager).

## Surfacing the assistant to students

This plugin only links a course and pushes materials. Students reach the Minerva assistant via an **LTI launch**, configured as an external tool in Moodle, not via this plugin.

## License

This plugin is licensed under the [GNU GPL v3 or later](https://www.gnu.org/copyleft/gpl.html).
