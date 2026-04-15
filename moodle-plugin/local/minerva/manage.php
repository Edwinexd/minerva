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
 * Manage the link between a Moodle course and a Minerva course.
 *
 * Teachers configure the Minerva API URL, API key, and select a course.
 * Credentials are stored per course link, not globally.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

require_once(__DIR__ . '/../../config.php');

$courseid = required_param('id', PARAM_INT);
$action = optional_param('action', '', PARAM_ALPHA);

$course = get_course($courseid);
require_login($course);

$context = context_course::instance($courseid);
require_capability('local/minerva:manage', $context);

$pageurl = new moodle_url('/local/minerva/manage.php', ['id' => $courseid]);
$PAGE->set_url($pageurl);
$PAGE->set_context($context);
$PAGE->set_course($course);
$PAGE->set_title(get_string('manage_link', 'local_minerva'));
$PAGE->set_heading($course->fullname . ' - ' . get_string('manage_link', 'local_minerva'));
$PAGE->set_pagelayout('incourse');

// Mutating actions must be POST + sesskey.
if ($action !== '') {
    require_sesskey();
    if (!in_array($_SERVER['REQUEST_METHOD'] ?? '', ['POST'], true)) {
        throw new moodle_exception('invalidrequest');
    }
}

if ($action === 'unlink') {
    $DB->delete_records('local_minerva_links', ['courseid' => $courseid]);
    // Drop the per-course sync log too, so re-linking doesn't silently
    // assume content already lives in Minerva.
    $DB->delete_records('local_minerva_sync_log', ['courseid' => $courseid]);
    redirect(
        $pageurl,
        get_string('link_removed', 'local_minerva'),
        null,
        \core\output\notification::NOTIFY_SUCCESS
    );
}

if ($action === 'resetsync') {
    $count = $DB->count_records('local_minerva_sync_log', ['courseid' => $courseid]);
    $DB->delete_records('local_minerva_sync_log', ['courseid' => $courseid]);
    redirect(
        $pageurl,
        get_string('sync_log_reset_done', 'local_minerva', (object)['count' => $count]),
        null,
        \core\output\notification::NOTIFY_SUCCESS
    );
}

if ($action === 'sync') {
    $link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);
    if ($link) {
        try {
            $client = \local_minerva\api_client::from_link($link);
            $result = \local_minerva\task\sync_enrolments::reconcile_members($client, $link, $context);
            redirect(
                $pageurl,
                get_string('sync_enrolment_done', 'local_minerva', $result),
                null,
                \core\output\notification::NOTIFY_SUCCESS
            );
        } catch (\Exception $e) {
            redirect(
                $pageurl,
                $e->getMessage(),
                null,
                \core\output\notification::NOTIFY_ERROR
            );
        }
    }
    redirect($pageurl);
}

echo $OUTPUT->header();

// Get current link.
$link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);

if ($link) {
    // Show current link and management options.
    echo html_writer::tag(
        'div',
        html_writer::tag('strong', get_string('linked_course', 'local_minerva') . ': ') .
            s($link->minerva_course_name) .
            ' (' . s($link->minerva_course_id) . ')' .
            html_writer::empty_tag('br') .
            html_writer::tag(
                'small',
                get_string('settings_apiurl', 'local_minerva') . ': ' . s($link->minerva_api_url),
                ['class' => 'text-muted']
            ),
        ['class' => 'alert alert-info']
    );

    // Unlink (destructive: confirm + red).
    $unlinkbtn = new \core\output\single_button(
        new moodle_url($pageurl, ['action' => 'unlink']),
        get_string('unlink_course', 'local_minerva'),
        'post',
        \core\output\single_button::BUTTON_DANGER
    );
    $unlinkbtn->add_confirm_action(get_string('unlink_course_confirm', 'local_minerva'));
    echo $OUTPUT->render($unlinkbtn);

    // Sync enrolment (non-destructive).
    echo $OUTPUT->single_button(
        new moodle_url($pageurl, ['action' => 'sync']),
        get_string('sync_enrolment', 'local_minerva'),
        'post'
    );

    // Sync materials button.
    if (has_capability('local/minerva:syncmaterials', $context)) {
        $maturl = new moodle_url('/local/minerva/sync.php', ['id' => $courseid]);
        echo html_writer::link($maturl, get_string('sync_materials', 'local_minerva'), [
            'class' => 'btn btn-secondary',
        ]);

        $resetbtn = new \core\output\single_button(
            new moodle_url($pageurl, ['action' => 'resetsync']),
            get_string('reset_sync_log', 'local_minerva'),
            'post',
            \core\output\single_button::BUTTON_DANGER
        );
        $resetbtn->add_confirm_action(get_string('reset_sync_log_confirm', 'local_minerva'));
        echo $OUTPUT->render($resetbtn);
    }
} else {
    // Link form: just URL (if not locked) + API key.
    // The key is scoped to a single course, so we resolve it automatically.
    $form = new \local_minerva\form\link_course_form($pageurl);

    if ($form->is_cancelled()) {
        redirect(new moodle_url('/course/view.php', ['id' => $courseid]));
    }

    if ($data = $form->get_data()) {
        // Validation has already resolved the scoped course; reuse it instead
        // of calling the API a second time.
        $mc = $form->resolvedcourse;
        if ($mc === null) {
            // Defensive: should not happen (validation ran), but guard anyway.
            redirect(
                $pageurl,
                get_string('no_scoped_course', 'local_minerva'),
                null,
                \core\output\notification::NOTIFY_ERROR
            );
        }

        $record = new stdClass();
        $record->courseid = $courseid;
        $record->minerva_course_id = $mc->id;
        $record->minerva_course_name = $mc->name;
        $record->minerva_api_url = rtrim($data->minerva_api_url, '/');
        $record->minerva_api_key = $data->minerva_api_key;
        $record->timecreated = time();
        $record->timemodified = time();
        $DB->insert_record('local_minerva_links', $record);

        redirect(
            $pageurl,
            get_string('link_saved', 'local_minerva'),
            null,
            \core\output\notification::NOTIFY_SUCCESS
        );
    }

    $form->set_data(['courseid' => $courseid]);
    $form->display();
}

echo $OUTPUT->footer();
