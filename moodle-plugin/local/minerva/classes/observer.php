<?php
// This file is part of Moodle - http://moodle.org/
//
// Moodle is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Moodle is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Moodle.  If not, see <http://www.gnu.org/licenses/>.

namespace local_minerva;

/**
 * Event observer for local_minerva.
 *
 * Membership is now provisioned by Minerva on LTI launch, so the plugin no
 * longer syncs enrolments. The only event we still care about is course
 * deletion, where we clean up our own link + sync-log rows.
 *
 * @package    local_minerva
 * @copyright  2026 Edwin Sundberg
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class observer {
    /**
     * Handle course deleted event: clean up local link + sync log rows.
     *
     * @param \core\event\course_deleted $event
     */
    public static function course_deleted(\core\event\course_deleted $event): void {
        global $DB;

        $DB->delete_records('local_minerva_links', ['courseid' => $event->courseid]);
        $DB->delete_records('local_minerva_sync_log', ['courseid' => $event->courseid]);
    }

    /**
     * Handle course module deleted: orphan the matching Minerva docs
     * by source_ref so retrieval stops surfacing material the teacher
     * just removed. Best-effort: failures are logged at debug level
     * and the periodic reconcile sweep is the safety net.
     *
     * The event fires AFTER the cm row is deleted, so we can't
     * re-read the cm to compute its source_ref. Instead we look up
     * everything in the local sync log whose source_ref matches the
     * cm-derived prefix `*:cm:{cmid}` (covers `url:cm:N`,
     * `page:cm:N`, `label:cm:N`, `resource_intro:cm:N`,
     * `mod_file:cm:N:*`) and send those refs to Minerva to be
     * orphaned. Items without a source_ref (legacy pre-slice-2
     * uploads) are ignored ; they get reconciled out gradually as
     * upstream Moodle objects get re-discovered.
     *
     * @param \core\event\course_module_deleted $event
     */
    public static function course_module_deleted(\core\event\course_module_deleted $event): void {
        global $DB;

        $cmid = (int) $event->objectid;
        $courseid = (int) $event->courseid;

        $link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);
        if (!$link) {
            return;
        }

        // Match every sourceref that names this cm. Both the
        // `*:cm:{cmid}` exact form and the `mod_file:cm:{cmid}:*`
        // prefix form are covered by these two LIKE clauses.
        $likeexact = $DB->sql_like('sourceref', ':refexact');
        $likeprefix = $DB->sql_like('sourceref', ':refprefix');
        $params = [
            'courseid' => $courseid,
            'refexact' => '%:cm:' . $cmid,
            'refprefix' => 'mod_file:cm:' . $cmid . ':%',
        ];
        $refs = $DB->get_fieldset_sql(
            "SELECT DISTINCT sourceref
               FROM {local_minerva_sync_log}
              WHERE courseid = :courseid
                AND sourceref IS NOT NULL
                AND ({$likeexact} OR {$likeprefix})",
            $params
        );

        if (empty($refs)) {
            return;
        }

        try {
            $client = \local_minerva\api_client::from_link($link);
            $client->orphan_by_source_refs($link->minerva_course_id, $refs);
        } catch (\Throwable $t) {
            // Network / auth failure: don't break the user-facing
            // delete flow. The next scheduled sync's reconcile sweep
            // will pick this up.
            debugging(
                'local_minerva: orphan-on-delete failed for cm ' . $cmid . ': ' . $t->getMessage(),
                DEBUG_DEVELOPER
            );
        }

        // Local bookkeeping: drop the log rows so the next reconcile
        // sweep doesn't list these refs as "still present".
        $DB->delete_records_select(
            'local_minerva_sync_log',
            "courseid = :courseid AND sourceref IS NOT NULL AND ({$likeexact} OR {$likeprefix})",
            $params
        );
    }
}
