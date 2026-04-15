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
use local_minerva\observer;

/**
 * Scheduled task to sync Moodle enrolments to Minerva.
 *
 * Runs periodically to catch any enrolments missed by the event observer.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class sync_enrolments extends \core\task\scheduled_task {
    /**
     * Return the task's name.
     *
     * @return string
     */
    public function get_name(): string {
        return get_string('task_sync_enrolments', 'local_minerva');
    }

    /**
     * Execute the task.
     */
    public function execute(): void {
        global $DB;

        if (!get_config('local_minerva', 'autosync_enrolment')) {
            mtrace('Minerva enrolment sync is disabled.');
            return;
        }

        $links = $DB->get_records('local_minerva_links');
        if (empty($links)) {
            mtrace('No Minerva course links found.');
            return;
        }

        foreach ($links as $link) {
            try {
                $client = api_client::from_link($link);
                $this->sync_course($client, $link);
            } catch (\Exception $e) {
                mtrace("  Course {$link->courseid}: API not configured - " . $e->getMessage());
            }
        }
    }

    /**
     * Sync a single linked course.
     *
     * @param api_client $client
     * @param object $link
     */
    private function sync_course(api_client $client, object $link): void {
        $context = \context_course::instance($link->courseid, IGNORE_MISSING);
        if (!$context) {
            mtrace("  Course {$link->courseid} no longer exists, skipping.");
            return;
        }

        $result = self::reconcile_members($client, $link, $context, function (string $msg): void {
            mtrace('  ' . $msg);
        });

        mtrace("  Course {$link->courseid} -> Minerva {$link->minerva_course_id}: " .
            "added {$result->added}, removed {$result->removed}.");
    }

    /**
     * Reconcile Minerva student membership against Moodle enrolment for a single course.
     *
     * Adds every currently-enrolled Moodle user to Minerva, and removes every
     * Minerva student whose eppn is not in the Moodle enrolled set. Teachers and
     * TAs on the Minerva side are never removed (they may have been granted
     * access out-of-band).
     *
     * @param api_client $client
     * @param object $link
     * @param \context_course $context
     * @param callable|null $logger fn(string $msg): void for per-user failures
     * @return \stdClass {added: int, removed: int}
     */
    public static function reconcile_members(
        api_client $client,
        object $link,
        \context_course $context,
        ?callable $logger = null
    ): \stdClass {
        $enrolledusers = get_enrolled_users(
            $context,
            '',
            0,
            'u.id, u.username, u.firstname, u.lastname'
        );

        // Build lowercase eppn set from Moodle side.
        $moodleeppns = [];
        foreach ($enrolledusers as $user) {
            $moodleeppns[strtolower(observer::get_eppn($user))] = true;
        }

        $added = 0;
        foreach ($enrolledusers as $user) {
            $eppn = observer::get_eppn($user);
            $displayname = trim($user->firstname . ' ' . $user->lastname);
            try {
                $client->add_member($link->minerva_course_id, $eppn, $displayname);
                $added++;
            } catch (\Exception $e) {
                if ($logger) {
                    $logger("Failed to add {$user->username}: " . $e->getMessage());
                }
            }
        }

        $removed = 0;
        try {
            $members = $client->list_members($link->minerva_course_id);
        } catch (\Exception $e) {
            if ($logger) {
                $logger("Failed to list Minerva members: " . $e->getMessage());
            }
            return (object)['added' => $added, 'removed' => 0];
        }

        foreach ($members as $member) {
            $role = $member->role ?? '';
            $eppn = isset($member->eppn) ? strtolower((string) $member->eppn) : '';
            if ($role !== 'student' || $eppn === '') {
                // Never touch teachers/TAs; skip records without an eppn we can match on.
                continue;
            }
            if (isset($moodleeppns[$eppn])) {
                continue;
            }
            try {
                $client->remove_member($link->minerva_course_id, $eppn);
                $removed++;
            } catch (\Exception $e) {
                if ($logger) {
                    $logger("Failed to remove {$eppn}: " . $e->getMessage());
                }
            }
        }

        return (object)['added' => $added, 'removed' => $removed];
    }
}
