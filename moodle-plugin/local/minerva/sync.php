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

/**
 * Sync course materials (PDF files) to the linked Minerva course.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

require_once(__DIR__ . '/../../config.php');

$courseid = required_param('id', PARAM_INT);
$confirm = optional_param('confirm', 0, PARAM_BOOL);

$course = get_course($courseid);
require_login($course);

$context = context_course::instance($courseid);
require_capability('local/minerva:syncmaterials', $context);

$link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);
if (!$link) {
    throw new moodle_exception('no_link', 'local_minerva');
}

$pageurl = new moodle_url('/local/minerva/sync.php', ['id' => $courseid]);
$manageurl = new moodle_url('/local/minerva/manage.php', ['id' => $courseid]);

$PAGE->set_url($pageurl);
$PAGE->set_context($context);
$PAGE->set_course($course);
$PAGE->set_title(get_string('sync_materials', 'local_minerva'));
$PAGE->set_heading($course->fullname . ' - ' . get_string('sync_materials', 'local_minerva'));
$PAGE->set_pagelayout('admin');

// Find all PDF files in the course.
$fs = get_file_storage();
$files = $fs->get_area_files($context->id, 'mod_resource', 'content', false, 'filename', false);

// Also check mod_folder.
$folderfiles = $fs->get_area_files($context->id, 'mod_folder', 'content', false, 'filename', false);
$files = array_merge($files, $folderfiles);

// Filter to PDFs only.
$pdffiles = [];
foreach ($files as $file) {
    if ($file->is_directory()) {
        continue;
    }
    $mimetype = $file->get_mimetype();
    if ($mimetype === 'application/pdf') {
        $pdffiles[] = $file;
    }
}

// Check which files have already been synced.
$alreadysynced = $DB->get_records_menu('local_minerva_sync_log',
    ['courseid' => $courseid], '', 'contenthash, id');

$newfiles = [];
foreach ($pdffiles as $file) {
    if (!isset($alreadysynced[$file->get_contenthash()])) {
        $newfiles[] = $file;
    }
}

if ($confirm && confirm_sesskey()) {
    // Perform the sync.
    try {
        $client = new \local_minerva\api_client();
    } catch (\Exception $e) {
        redirect($manageurl, $e->getMessage(), null, \core\output\notification::NOTIFY_ERROR);
    }

    $uploaded = 0;
    foreach ($newfiles as $file) {
        // Copy file to a temp location for upload.
        $tmpfile = tempnam(sys_get_temp_dir(), 'minerva_');
        $file->copy_content_to($tmpfile);

        try {
            $result = $client->upload_document(
                $link->minerva_course_id,
                $tmpfile,
                $file->get_filename()
            );

            // Log the sync.
            $record = new stdClass();
            $record->courseid = $courseid;
            $record->contenthash = $file->get_contenthash();
            $record->filename = $file->get_filename();
            $record->minerva_doc_id = $result->id ?? '';
            $record->timecreated = time();
            $DB->insert_record('local_minerva_sync_log', $record);

            $uploaded++;
        } catch (\Exception $e) {
            debugging("Failed to upload {$file->get_filename()}: " . $e->getMessage(),
                DEBUG_NORMAL);
        } finally {
            @unlink($tmpfile);
        }
    }

    $a = (object)['uploaded' => $uploaded];
    redirect($manageurl, get_string('sync_materials_done', 'local_minerva', $a), null,
        \core\output\notification::NOTIFY_SUCCESS);
}

echo $OUTPUT->header();

echo html_writer::tag('p', get_string('sync_materials_desc', 'local_minerva'));

if (empty($newfiles)) {
    echo $OUTPUT->notification(get_string('sync_materials_none', 'local_minerva'), 'info');
    echo html_writer::link($manageurl, get_string('back'), ['class' => 'btn btn-secondary']);
} else {
    echo html_writer::tag('p', 'Found ' . count($newfiles) . ' new PDF file(s) to sync:');

    echo html_writer::start_tag('ul');
    foreach ($newfiles as $file) {
        $size = display_size($file->get_filesize());
        echo html_writer::tag('li', s($file->get_filename()) . " ({$size})");
    }
    echo html_writer::end_tag('ul');

    if (count($pdffiles) > count($newfiles)) {
        echo html_writer::tag('p',
            count($pdffiles) - count($newfiles) . ' file(s) already synced previously.',
            ['class' => 'text-muted']
        );
    }

    $confirmurl = new moodle_url($pageurl, ['confirm' => 1, 'sesskey' => sesskey()]);
    echo html_writer::link($confirmurl, get_string('sync_materials', 'local_minerva'), [
        'class' => 'btn btn-primary mr-2',
    ]);
    echo html_writer::link($manageurl, get_string('cancel'), ['class' => 'btn btn-secondary']);
}

echo $OUTPUT->footer();
