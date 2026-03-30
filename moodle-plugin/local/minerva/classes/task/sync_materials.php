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
 * Scheduled task to sync new course materials (PDFs) to Minerva.
 *
 * Runs periodically to find and upload any new PDF files that haven't
 * been synced yet for each linked course.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class sync_materials extends \core\task\scheduled_task {
    /**
     * Return the task's name.
     *
     * @return string
     */
    public function get_name(): string {
        return get_string('task_sync_materials', 'local_minerva');
    }

    /**
     * Execute the task.
     */
    public function execute(): void {
        global $DB;

        if (!get_config('local_minerva', 'autosync_materials')) {
            mtrace('Minerva materials sync is disabled.');
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
     * Sync materials for a single linked course.
     *
     * @param api_client $client
     * @param object $link
     */
    private function sync_course(api_client $client, object $link): void {
        global $DB;

        $course = get_course($link->courseid);
        if (!$course) {
            mtrace("  Course {$link->courseid} no longer exists, skipping.");
            return;
        }

        $newfiles = self::find_unsynced_pdfs($course, $link->courseid);

        if (empty($newfiles)) {
            mtrace("  Course {$link->courseid}: no new materials.");
            return;
        }

        $uploaded = 0;
        foreach ($newfiles as $file) {
            $tmpfile = tempnam(sys_get_temp_dir(), 'minerva_');
            $file->copy_content_to($tmpfile);

            try {
                $result = $client->upload_document(
                    $link->minerva_course_id,
                    $tmpfile,
                    $file->get_filename()
                );

                $record = new \stdClass();
                $record->courseid = $link->courseid;
                $record->contenthash = $file->get_contenthash();
                $record->filename = $file->get_filename();
                $record->minerva_doc_id = $result->id ?? '';
                $record->timecreated = time();
                $DB->insert_record('local_minerva_sync_log', $record);

                $uploaded++;
            } catch (\Exception $e) {
                mtrace("  Failed to upload {$file->get_filename()}: " . $e->getMessage());
            } finally {
                @unlink($tmpfile);
            }
        }

        mtrace("  Course {$link->courseid} -> Minerva {$link->minerva_course_id}: uploaded {$uploaded} new file(s).");
    }

    /**
     * Find PDF files in a course that haven't been synced yet.
     *
     * @param object $course Moodle course object.
     * @param int $courseid Moodle course ID.
     * @return \stored_file[] Array of unsynced PDF files.
     */
    public static function find_unsynced_pdfs(object $course, int $courseid): array {
        global $DB;

        $modinfo = get_fast_modinfo($course);
        $fs = get_file_storage();
        $pdffiles = [];

        foreach ($modinfo->get_cms() as $cm) {
            if (!$cm->visible || !$cm->available) {
                continue;
            }

            if (!in_array($cm->modname, ['resource', 'folder'])) {
                continue;
            }

            $component = 'mod_' . $cm->modname;
            $modcontext = \context_module::instance($cm->id);
            $files = $fs->get_area_files($modcontext->id, $component, 'content', false, 'filename', false);

            foreach ($files as $file) {
                if ($file->is_directory()) {
                    continue;
                }
                if ($file->get_mimetype() === 'application/pdf') {
                    $pdffiles[] = $file;
                }
            }
        }

        $alreadysynced = $DB->get_records_menu(
            'local_minerva_sync_log',
            ['courseid' => $courseid],
            '',
            'contenthash, id'
        );

        $newfiles = [];
        foreach ($pdffiles as $file) {
            if (!isset($alreadysynced[$file->get_contenthash()])) {
                $newfiles[] = $file;
            }
        }

        return $newfiles;
    }
}
