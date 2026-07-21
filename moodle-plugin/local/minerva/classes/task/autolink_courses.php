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

namespace local_minerva\task;

use local_minerva\api_client;

/**
 * Scheduled task that auto-links Moodle courses to Minerva by external id.
 *
 * A course whose "Course ID number" (idnumber) holds one or more Daisy
 * offering ids is linked to the matching Minerva course without any teacher
 * action. A manual link always wins (linked courses are skipped) and an
 * explicit unlink records an opt-out (see local_minerva_autolink_optout) so
 * this sweep leaves it alone. Admins disable the whole behaviour via the
 * `autolink_by_external_id` setting.
 *
 * Because a cron run has no acting teacher, provisioning goes through
 * /api/integration/site/provision-by-offering, which authorises on the site
 * key plus the external-id match alone (setting a course idnumber is a
 * manager-only capability in Moodle, so the match is the trust anchor).
 *
 * @package    local_minerva
 * @copyright  2026 Edwin Sundberg
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class autolink_courses extends \core\task\scheduled_task {
    /**
     * Return the task's name.
     *
     * @return string
     */
    public function get_name(): string {
        return get_string('task_autolink_courses', 'local_minerva');
    }

    /**
     * Execute the task.
     */
    public function execute(): void {
        global $DB, $CFG;
        require_once($CFG->dirroot . '/local/minerva/lib.php');
        // Moodle's \curl (used by api_client) lives in filelib and isn't
        // autoloaded in the bare CLI/cron context.
        require_once($CFG->libdir . '/filelib.php');

        if (!get_config('local_minerva', 'autolink_by_external_id')) {
            mtrace('Minerva auto-link is disabled.');
            return;
        }
        if (!api_client::site_integration_available()) {
            mtrace('Minerva auto-link needs the site integration key; skipping.');
            return;
        }

        // Candidates: courses with an external id that are neither already
        // linked nor opted out. The joins do the "manual link wins" and
        // "respect explicit unlink" filtering in one query.
        $sql = "SELECT c.id, c.fullname, c.idnumber
                  FROM {course} c
             LEFT JOIN {local_minerva_links} l ON l.courseid = c.id
             LEFT JOIN {local_minerva_autolink_optout} o ON o.courseid = c.id
                 WHERE c.id <> :siteid
                   AND " . $DB->sql_isnotempty('course', 'c.idnumber', false, false) . "
                   AND l.id IS NULL
                   AND o.id IS NULL";
        $candidates = $DB->get_records_sql($sql, ['siteid' => SITEID]);
        if (empty($candidates)) {
            mtrace('Minerva auto-link: no candidate courses.');
            return;
        }

        $url = rtrim(get_config('local_minerva', 'minerva_url'), '/');
        $linked = 0;
        foreach ($candidates as $course) {
            $ids = local_minerva_parse_external_ids($course->idnumber);
            if (empty($ids)) {
                continue;
            }
            try {
                $client = api_client::from_site_config();
                $res = $client->site_provision_by_offering($ids, format_string($course->fullname));
            } catch (\Exception $e) {
                mtrace("  Course {$course->id}: auto-link failed: " . $e->getMessage());
                continue;
            }
            $status = $res->status ?? 'none';
            if ($status !== 'matched' || empty($res->course) || empty($res->key)) {
                continue;
            }
            if (!empty($res->multiple_matches)) {
                // Several offering ids resolved to different Minerva courses;
                // we linked the first. Log so a split mapping is visible (the
                // fix is usually merging the offerings on the Minerva side).
                mtrace(
                    "  Course {$course->id}: external id resolves to multiple Minerva courses; " .
                        "linked the first ({$res->course->id})."
                );
            }

            $record = new \stdClass();
            $record->courseid = $course->id;
            $record->minerva_course_id = $res->course->id;
            $record->minerva_course_name = $res->course->name;
            $record->minerva_api_url = $url;
            $record->minerva_api_key = $res->key;
            $record->auto_linked = 1;
            $record->timecreated = time();
            $record->timemodified = time();
            try {
                $DB->insert_record('local_minerva_links', $record);
                $linked++;
                mtrace("  Course {$course->id} auto-linked to Minerva {$res->course->id} ({$res->course->name}).");
            } catch (\dml_write_exception $e) {
                // Unique courseid: a manual link raced in since our query. The
                // minted key is harmless (unused); leave the existing link.
                mtrace("  Course {$course->id}: link already present, skipping.");
            }
        }
        mtrace("Minerva auto-link: linked {$linked} course(s).");
    }
}
