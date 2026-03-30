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
$PAGE->set_pagelayout('admin');

// Handle unlink action.
if ($action === 'unlink' && confirm_sesskey()) {
    $DB->delete_records('local_minerva_links', ['courseid' => $courseid]);
    redirect($pageurl, get_string('link_removed', 'local_minerva'), null,
        \core\output\notification::NOTIFY_SUCCESS);
}

// Handle sync enrolment action.
if ($action === 'sync' && confirm_sesskey()) {
    $link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);
    if ($link) {
        try {
            $client = new \local_minerva\api_client();
            $enrolledusers = get_enrolled_users($context, '', 0,
                'u.id, u.username, u.firstname, u.lastname');
            $added = 0;
            foreach ($enrolledusers as $user) {
                $eppn = \local_minerva\observer::get_eppn($user);
                $displayname = trim($user->firstname . ' ' . $user->lastname);
                $client->add_member($link->minerva_course_id, $eppn, $displayname);
                $added++;
            }
            $a = (object)['added' => $added, 'removed' => 0];
            redirect($pageurl, get_string('sync_enrolment_done', 'local_minerva', $a), null,
                \core\output\notification::NOTIFY_SUCCESS);
        } catch (\Exception $e) {
            redirect($pageurl, $e->getMessage(), null,
                \core\output\notification::NOTIFY_ERROR);
        }
    }
}

// Check if API is configured.
$apiurl = get_config('local_minerva', 'apiurl');
$apikey = get_config('local_minerva', 'apikey');

echo $OUTPUT->header();

if (empty($apiurl) || empty($apikey)) {
    echo $OUTPUT->notification(get_string('no_api_configured', 'local_minerva'), 'warning');
    echo $OUTPUT->footer();
    exit;
}

// Get current link.
$link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);

if ($link) {
    // Show current link and management options.
    echo html_writer::tag('div',
        html_writer::tag('strong', get_string('linked_course', 'local_minerva') . ': ') .
        s($link->minerva_course_name) .
        ' (' . s($link->minerva_course_id) . ')',
        ['class' => 'alert alert-info']
    );

    // Unlink button.
    $unlinkurl = new moodle_url($pageurl, ['action' => 'unlink', 'sesskey' => sesskey()]);
    echo html_writer::link($unlinkurl, get_string('unlink_course', 'local_minerva'), [
        'class' => 'btn btn-danger mr-2',
    ]);

    // Sync enrolment button.
    $syncurl = new moodle_url($pageurl, ['action' => 'sync', 'sesskey' => sesskey()]);
    echo html_writer::link($syncurl, get_string('sync_enrolment', 'local_minerva'), [
        'class' => 'btn btn-secondary mr-2',
    ]);

    // Sync materials button.
    if (has_capability('local/minerva:syncmaterials', $context)) {
        $maturl = new moodle_url('/local/minerva/sync.php', ['id' => $courseid]);
        echo html_writer::link($maturl, get_string('sync_materials', 'local_minerva'), [
            'class' => 'btn btn-secondary',
        ]);
    }
} else {
    // Show form to link a course.
    try {
        $client = new \local_minerva\api_client();
        $minervacourses = $client->list_courses();
    } catch (\Exception $e) {
        echo $OUTPUT->notification($e->getMessage(), 'error');
        echo $OUTPUT->footer();
        exit;
    }

    $form = new \local_minerva\form\link_course_form($pageurl, [
        'minerva_courses' => $minervacourses,
    ]);

    if ($form->is_cancelled()) {
        redirect(new moodle_url('/course/view.php', ['id' => $courseid]));
    }

    if ($data = $form->get_data()) {
        // Find the selected course name.
        $coursename = '';
        foreach ($minervacourses as $mc) {
            if ($mc->id === $data->minerva_course_id) {
                $coursename = $mc->name;
                break;
            }
        }

        $record = new stdClass();
        $record->courseid = $courseid;
        $record->minerva_course_id = $data->minerva_course_id;
        $record->minerva_course_name = $coursename;
        $record->timecreated = time();
        $record->timemodified = time();
        $DB->insert_record('local_minerva_links', $record);

        redirect($pageurl, get_string('link_saved', 'local_minerva'), null,
            \core\output\notification::NOTIFY_SUCCESS);
    }

    $form->set_data(['courseid' => $courseid]);
    $form->display();
}

echo $OUTPUT->footer();
