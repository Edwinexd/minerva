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
 * List all Minerva activity instances in a course.
 *
 * @package    mod_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

require_once(__DIR__ . '/../../config.php');

$id = required_param('id', PARAM_INT); // Course ID.

$course = get_course($id);
require_login($course);

$PAGE->set_url(new moodle_url('/mod/minerva/index.php', ['id' => $id]));
$PAGE->set_title(get_string('modulenameplural', 'mod_minerva'));
$PAGE->set_heading($course->fullname);
$PAGE->set_pagelayout('incourse');

echo $OUTPUT->header();

$instances = get_all_instances_in_course('minerva', $course);
if (!$instances) {
    notice(
        get_string('thereareno', 'moodle', get_string('modulenameplural', 'mod_minerva')),
        new moodle_url('/course/view.php', ['id' => $course->id])
    );
}

$table = new html_table();
$table->head = [get_string('name'), get_string('description')];
foreach ($instances as $instance) {
    $url = new moodle_url('/mod/minerva/view.php', ['id' => $instance->coursemodule]);
    $table->data[] = [
        html_writer::link($url, $instance->name),
        format_module_intro('minerva', $instance, $instance->coursemodule),
    ];
}

echo html_writer::table($table);
echo $OUTPUT->footer();
