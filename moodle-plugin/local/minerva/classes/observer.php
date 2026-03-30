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
 * Event observer for enrolment changes.
 *
 * Syncs Moodle enrolments to the linked Minerva course in real time.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class observer {
    /**
     * Handle user enrolment created event.
     *
     * @param \core\event\user_enrolment_created $event
     */
    public static function user_enrolled(\core\event\user_enrolment_created $event): void {
        if (!get_config('local_minerva', 'autosync_enrolment')) {
            return;
        }

        $courseid = $event->courseid;
        $userid = $event->relateduserid;

        self::sync_user_to_minerva($courseid, $userid, 'add');
    }

    /**
     * Handle user enrolment deleted event.
     *
     * @param \core\event\user_enrolment_deleted $event
     */
    public static function user_unenrolled(\core\event\user_enrolment_deleted $event): void {
        if (!get_config('local_minerva', 'autosync_enrolment')) {
            return;
        }

        $courseid = $event->courseid;
        $userid = $event->relateduserid;

        self::sync_user_to_minerva($courseid, $userid, 'remove');
    }

    /**
     * Add or remove a user from the linked Minerva course.
     *
     * @param int $courseid Moodle course ID.
     * @param int $userid Moodle user ID.
     * @param string $action 'add' or 'remove'.
     */
    private static function sync_user_to_minerva(int $courseid, int $userid, string $action): void {
        global $DB;

        $link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);
        if (!$link) {
            return;
        }

        $user = $DB->get_record('user', ['id' => $userid], 'id, username, firstname, lastname');
        if (!$user) {
            return;
        }

        $eppn = self::get_eppn($user);
        $displayname = trim($user->firstname . ' ' . $user->lastname);

        try {
            $client = api_client::from_link($link);

            if ($action === 'add') {
                $client->add_member($link->minerva_course_id, $eppn, $displayname);
            } else {
                $client->remove_member($link->minerva_course_id, $eppn);
            }
        } catch (\Exception $e) {
            debugging("Minerva enrolment sync failed for user {$userid} " .
                "in course {$courseid}: " . $e->getMessage(), DEBUG_NORMAL);
        }
    }

    /**
     * Build the eppn for a Moodle user.
     *
     * @param object $user Moodle user record.
     * @return string The eppn (e.g. user1234@SU.SE).
     */
    public static function get_eppn(object $user): string {
        $suffix = get_config('local_minerva', 'eppn_suffix') ?: '@SU.SE';
        $username = $user->username;

        // If the username already contains @, use it as-is.
        if (strpos($username, '@') !== false) {
            return $username;
        }

        return $username . $suffix;
    }
}
